use dmz_core::install::{InstallConfig, InstallerEngine};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use tempfile::tempdir;

use super::python_workflow::build_dummy_wheel;

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

