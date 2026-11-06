use dmz_core::install::{InstallConfig, InstallerEngine};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use tempfile::tempdir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// Helper that builds a valid PEP 427 Python wheel (.whl) as bytes
fn build_dummy_wheel(dist_name: &str, version: &str, module_name: &str) -> Vec<u8> {
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buffer);
        let options = SimpleFileOptions::default();

        // 1. Python package files
        zip.start_file(format!("{}/__init__.py", module_name), options)
            .unwrap();
        zip.write_all(format!("__version__ = '{}'\n", version).as_bytes())
            .unwrap();

        zip.start_file(format!("{}/core.py", module_name), options)
            .unwrap();
        zip.write_all(b"def run():\n    return 'success'\n")
            .unwrap();

        // 2. Wheel metadata (.dist-info)
        let dist_info = format!("{}-{}.dist-info", dist_name, version);
        zip.start_file(format!("{}/METADATA", dist_info), options)
            .unwrap();
        zip.write_all(
            format!(
                "Metadata-Version: 2.1\nName: {}\nVersion: {}\nSummary: Test Python package\n",
                dist_name, version
            )
            .as_bytes(),
        )
        .unwrap();

        // 3. Wheel declaration
        zip.start_file(format!("{}/WHEEL", dist_info), options)
            .unwrap();
        zip.write_all(
            b"Wheel-Version: 1.0\nGenerator: test\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        )
        .unwrap();

        // 4. Console script entry point
        zip.start_file(format!("{}/entry_points.txt", dist_info), options)
            .unwrap();
        zip.write_all(
            format!(
                "[console_scripts]\n{} = {}.core:run\n",
                module_name, module_name
            )
            .as_bytes(),
        )
        .unwrap();

        zip.finish().unwrap();
    }
    buffer.into_inner()
}

#[test]
fn test_python_wheel_offline_install_and_console_script_hydration() {
    let temp = tempdir().expect("Failed to create tempdir");
    let workspace = temp.path().join("workspace");
    let store = temp.path().join("store");
    fs::create_dir_all(&workspace).unwrap();
    fs::create_dir_all(&store).unwrap();

    let pkg_name = "Jinja2";
    let pkg_version = "3.1.2";
    let safe_name = pkg_name;

    // 1. Create a genuine .whl archive in the package store
    let wheel_bytes = build_dummy_wheel(pkg_name, pkg_version, "jinja2");
    let pkg_store_dir = store.join(safe_name);
    fs::create_dir_all(&pkg_store_dir).unwrap();
    let archive_path = pkg_store_dir.join(format!("{}-{}.archive", safe_name, pkg_version));
    fs::write(&archive_path, &wheel_bytes).unwrap();

    let mut hasher = Sha256::new();
    hasher.update(&wheel_bytes);
    let computed_sha256 = format!("{:x}", hasher.finalize());

    // 2. Create dmz.lock referencing this package
    let locked_pkg = LockedPackage {
        name: pkg_name.to_string(),
        version: pkg_version.to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: computed_sha256,
        source_url: None,
        dependencies: vec![],
        member_id: None,
    };

    let mut packages = BTreeMap::new();
    packages.insert(format!("{}:{}", pkg_name, pkg_version), locked_pkg);

    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        vcs: None,
        workspaces: BTreeMap::new(),
        closure_signature: DmzLockfile::compute_closure_hash(&packages),
        packages,
    };

    let lockfile_path = workspace.join("dmz.lock");
    lockfile.save(&lockfile_path).unwrap();

    // 3. Execute offline install via InstallerEngine
    let config = InstallConfig {
        workspace_path: workspace.clone(),
        lockfile_path,
        store_dir: store,
    };

    let installed_count =
        InstallerEngine::install(&config).expect("Offline install of wheel must succeed");
    assert_eq!(installed_count, 1);

    // 4. Verify site-packages extraction
    let site_packages = workspace
        .join(".dmz")
        .join("venv")
        .join("lib")
        .join("python3.11")
        .join("site-packages");

    let module_init = site_packages.join("jinja2").join("__init__.py");
    assert!(
        module_init.exists(),
        "jinja2/__init__.py must be extracted from wheel"
    );

    let module_core = site_packages.join("jinja2").join("core.py");
    assert!(
        module_core.exists(),
        "jinja2/core.py must be extracted from wheel"
    );

    // 5. Verify PEP 376 INSTALLER metadata attribution
    let installer_file = site_packages
        .join(format!("{}-{}.dist-info", pkg_name, pkg_version))
        .join("INSTALLER");
    assert!(
        installer_file.exists(),
        "INSTALLER file must be written into .dist-info"
    );
    let installer_content = fs::read_to_string(&installer_file).unwrap();
    assert_eq!(installer_content.trim(), "dmz", "INSTALLER must record dmz");

    // 6. Verify cross-platform CLI console script shims in .venv/bin/ and .dmz/venv/bin/
    let venv_bin = workspace.join(".venv").join("bin");
    #[cfg(unix)]
    {
        let posix_shim = venv_bin.join("jinja2");
        assert!(
            posix_shim.exists(),
            "POSIX console script shim .venv/bin/jinja2 must be created"
        );
        let shim_content = fs::read_to_string(&posix_shim).unwrap();
        assert!(
            shim_content.contains("jinja2.core"),
            "Shim must reference module"
        );
    }

    let cmd_shim = venv_bin.join("jinja2.cmd");
    assert!(
        cmd_shim.exists(),
        "Windows .cmd console script shim must be created"
    );

    let ps1_shim = venv_bin.join("jinja2.ps1");
    assert!(
        ps1_shim.exists(),
        "PowerShell .ps1 console script shim must be created"
    );

    // 7. Verify .pth linkage
    let pth_file = workspace
        .join(".venv")
        .join("lib")
        .join("python3.11")
        .join("site-packages")
        .join("dmz_store.pth");
    assert!(pth_file.exists(), "dmz_store.pth must exist");
}

#[test]
fn test_dual_repo_sidecar_member_dependency_isolation() {
    let temp = tempdir().expect("Failed to create tempdir");
    let umbrella_root = temp.path().join("umbrella");
    let primary_dir = umbrella_root.join("primary_app");
    let sidecar_dir = umbrella_root.join("workspaces/sidecars/ai-engine");
    let store = umbrella_root.join(".dmz/store");

    fs::create_dir_all(&primary_dir).unwrap();
    fs::create_dir_all(&sidecar_dir).unwrap();
    fs::create_dir_all(&store).unwrap();

    // 1. Setup multi-workspace manifest (dmz-workspace.toml)
    let ws_config = WorkspaceProjectConfig::new(
        "dual-app-umbrella",
        vec![
            WorkspaceMember::new("primary", "primary_app", MemberRole::Primary),
            WorkspaceMember::new(
                "ai-engine",
                "workspaces/sidecars/ai-engine",
                MemberRole::Sidecar,
            ),
        ],
    );
    ws_config
        .save_to_file(&umbrella_root.join("dmz-workspace.toml"))
        .unwrap();

    // 2. Create dummy Python wheel for Jinja2 in store
    let pkg_name = "Jinja2";
    let pkg_version = "3.1.2";
    let wheel_bytes = build_dummy_wheel(pkg_name, pkg_version, "jinja2");
    let pkg_store_dir = store.join(pkg_name);
    fs::create_dir_all(&pkg_store_dir).unwrap();
    let archive_path = pkg_store_dir.join(format!("{}-{}.archive", pkg_name, pkg_version));
    fs::write(&archive_path, &wheel_bytes).unwrap();

    let mut hasher = Sha256::new();
    hasher.update(&wheel_bytes);
    let computed_sha256 = format!("{:x}", hasher.finalize());

    // 3. Lockfile maps Jinja2 explicitly to the sidecar member
    let locked_pkg = LockedPackage {
        name: pkg_name.to_string(),
        version: pkg_version.to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: computed_sha256,
        source_url: None,
        dependencies: vec![],
        member_id: Some("ai-engine".to_string()),
    };

    let mut packages = BTreeMap::new();
    packages.insert(format!("{}:{}", pkg_name, pkg_version), locked_pkg);

    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        vcs: None,
        workspaces: BTreeMap::new(),
        closure_signature: DmzLockfile::compute_closure_hash(&packages),
        packages,
    };

    let lockfile_path = umbrella_root.join("dmz.lock");
    lockfile.save(&lockfile_path).unwrap();

    // 4. Run InstallerEngine from the umbrella root
    let config = InstallConfig {
        workspace_path: umbrella_root.clone(),
        lockfile_path,
        store_dir: store,
    };

    let installed_count =
        InstallerEngine::install(&config).expect("Sidecar offline install must succeed");
    assert_eq!(installed_count, 1);

    // 5. Jinja2 must be installed inside the sidecar directory:
    let sidecar_site_packages = sidecar_dir
        .join(".dmz")
        .join("venv")
        .join("lib")
        .join("python3.11")
        .join("site-packages");
    assert!(
        sidecar_site_packages.join("jinja2/__init__.py").exists(),
        "Jinja2 must be materialized in sidecar's .dmz/venv"
    );

    // 6. Jinja2 must NOT leak into the umbrella root or primary app:
    let root_site_packages = umbrella_root
        .join(".dmz")
        .join("venv")
        .join("lib")
        .join("python3.11")
        .join("site-packages");
    assert!(
        !root_site_packages.join("jinja2").exists(),
        "Sidecar dependencies must not be dumped into umbrella root"
    );

    let primary_site_packages = primary_dir
        .join(".dmz")
        .join("venv")
        .join("lib")
        .join("python3.11")
        .join("site-packages");
    assert!(
        !primary_site_packages.join("jinja2").exists(),
        "Sidecar dependencies must not leak into primary app directory"
    );
}
