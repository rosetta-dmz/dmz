use dmz_core::install::{InstallConfig, InstallerEngine};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use tempfile::tempdir;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

/// Helper that builds a valid PEP 427 Python wheel (.whl) as bytes
pub fn build_dummy_wheel(dist_name: &str, version: &str, module_name: &str) -> Vec<u8> {
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
fn test_airgap_offline_install_python_workflow() -> Result<(), Box<dyn std::error::Error>> {
    let tmp_dir = tempdir()?;
    let workspace_path = tmp_dir.path().join("workspace");
    let store_path = tmp_dir.path().join(".dmz/store");

    fs::create_dir_all(&workspace_path)?;
    fs::create_dir_all(&store_path)?;

    let pkg_name = "requests";
    let pkg_version = "2.31.0";
    let safe_name = pkg_name.replace('/', "__");

    let pkg_store_dir = store_path.join(&safe_name);
    fs::create_dir_all(&pkg_store_dir)?;

    let mut tar_builder = tar::Builder::new(Vec::new());
    let dummy_py_content = b"# requests module";
    let mut header = tar::Header::new_gnu();
    header.set_size(dummy_py_content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar_builder.append_data(&mut header, "requests/__init__.py", &dummy_py_content[..])?;
    let tar_bytes = tar_builder.into_inner()?;

    let mut gz_encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut gz_encoder, &tar_bytes)?;
    let payload = gz_encoder.finish()?;

    let archive_filename = format!("{}-{}.archive", safe_name, pkg_version);
    let archive_path = pkg_store_dir.join(&archive_filename);
    fs::write(&archive_path, &payload)?;

    let mut hasher = Sha256::new();
    hasher.update(&payload);
    let computed_sha256 = format!("{:x}", hasher.finalize());

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

    let closure_sig = DmzLockfile::compute_closure_hash(&packages);
    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        vcs: None,
        workspaces: BTreeMap::new(),
        closure_signature: closure_sig,
        packages,
    };

    let lockfile_path = workspace_path.join("dmz.lock");
    lockfile.save(&lockfile_path)?;

    let config = InstallConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path,
        store_dir: store_path,
    };

    let installed_count = InstallerEngine::install(&config)?;
    assert_eq!(installed_count, 1);

    let site_packages = workspace_path
        .join(".dmz")
        .join("venv")
        .join("lib")
        .join("python3.11")
        .join("site-packages")
        .join("requests")
        .join("__init__.py");
    assert!(
        site_packages.exists(),
        "Materialized dependency file should exist in site-packages"
    );

    Ok(())
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
