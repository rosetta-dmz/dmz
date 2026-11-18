use super::*;

#[test]
fn test_multi_workspace_lockfile_compatibility() {
    let pkg1 = LockedPackage {
        name: "express".to_string(),
        version: "4.19.2".to_string(),
        ecosystem: Ecosystem::Npm,
        sha256: "a1b2c3d4".to_string(),
        source_url: None,
        dependencies: vec![],
        member_id: Some("frontend".to_string()),
    };

    let pkg2 = LockedPackage {
        name: "tokio".to_string(),
        version: "1.38.0".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: "e5f6a7b8".to_string(),
        source_url: None,
        dependencies: vec![],
        member_id: Some("sidecar".to_string()),
    };

    let mut lockfile = DmzLockfile::new(vec![pkg1, pkg2]);
    lockfile.add_workspace(WorkspaceMetadata {
        id: "frontend".to_string(),
        path: "./apps/web".to_string(),
        role: "primary".to_string(),
        ecosystem: Some("npm".to_string()),
        closure_signature: None,
    });
    lockfile.add_workspace(WorkspaceMetadata {
        id: "sidecar".to_string(),
        path: "../engine-sidecar".to_string(),
        role: "sidecar".to_string(),
        ecosystem: Some("cargo".to_string()),
        closure_signature: None,
    });

    assert_eq!(lockfile.workspaces.len(), 2);
    assert_eq!(lockfile.packages.len(), 2);

    let serialized = serde_json::to_string_pretty(&lockfile).unwrap();
    let deserialized: DmzLockfile = serde_json::from_str(&serialized).unwrap();

    assert_eq!(deserialized.workspaces.len(), 2);
    assert_eq!(deserialized.packages.len(), 2);
    assert_eq!(
        deserialized.closure_signature,
        DmzLockfile::compute_closure_hash(&deserialized.packages)
    );
}

#[test]
fn test_lockfile_save_load_and_integrity_verification() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("dmz.lock");

    let pkg = LockedPackage {
        name: "test-pkg".to_string(),
        version: "1.0.0".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: "fake-sha".to_string(),
        source_url: None,
        dependencies: vec![],
        member_id: None,
    };
    let lockfile = DmzLockfile::new(vec![pkg]);
    lockfile.save(&path).unwrap();

    let loaded = DmzLockfile::load_and_verify(&path).unwrap();
    assert_eq!(loaded, lockfile);

    let mut tampered = lockfile.clone();
    tampered.closure_signature = "bad-hash".to_string();
    tampered.save(&path).unwrap();
    let err = DmzLockfile::load_and_verify(&path).unwrap_err();
    match err {
        LockError::IntegrityMismatch { expected, .. } => {
            assert_eq!(expected, "bad-hash");
        }
        other => panic!("Expected IntegrityMismatch, got {:?}", other),
    }

    let dir_err = DmzLockfile::load_and_verify(temp.path()).unwrap_err();
    assert!(matches!(dir_err, LockError::Io(_)));
}

#[test]
fn test_lockfile_default_and_trait_conformance() {
    let lockfile = DmzLockfile::default();
    assert_eq!(lockfile.version, 1);
    assert!(lockfile.packages.is_empty());
    assert_eq!(
        lockfile.closure_signature,
        DmzLockfile::compute_closure_hash(&lockfile.packages)
    );
}

#[test]
fn test_lockfile_file_needs_refresh_detects_staleness() {
    use crate::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("dmz.lock");
    assert!(DmzLockfile::file_needs_refresh(&path, None, true, None));

    // Legacy lockfile with include_dev=None requires refresh when include_dev=true.
    let mut lockfile = DmzLockfile::with_platform(vec![], Some("darwin-arm64".into()));
    lockfile.include_dev = None;
    lockfile.save(&path).unwrap();
    assert!(DmzLockfile::file_needs_refresh(
        &path,
        Some("darwin-arm64"),
        true,
        None
    ));

    // Stamped lockfile matches identical target & include_dev=true.
    lockfile.include_dev = Some(true);
    lockfile.save(&path).unwrap();
    assert!(!DmzLockfile::file_needs_refresh(
        &path,
        Some("darwin-arm64"),
        true,
        None
    ));

    // Stale if requested include_dev or target platform differs.
    assert!(DmzLockfile::file_needs_refresh(
        &path,
        Some("darwin-arm64"),
        false,
        None
    ));
    assert!(DmzLockfile::file_needs_refresh(
        &path,
        Some("linux-x64"),
        true,
        None
    ));

    // Stale if multi-workspace manifest declares a member absent from dmz.lock.
    let mut ws_cfg = WorkspaceProjectConfig {
        name: "suite".to_string(),
        members: vec![WorkspaceMember::new(
            "tauri-ui",
            "../ui",
            MemberRole::Sidecar,
        )],
        ..Default::default()
    };
    ws_cfg.members[0].ecosystem = Some("node".to_string());
    assert!(DmzLockfile::file_needs_refresh(
        &path,
        Some("darwin-arm64"),
        true,
        Some(&ws_cfg)
    ));

    // Stale if workspace has Node manifests with deps yet dmz.lock holds 0 npm packages.
    let ws_dir = temp.path().join("node-ws");
    std::fs::create_dir_all(&ws_dir).unwrap();
    std::fs::write(
        ws_dir.join("package.json"),
        r#"{"name":"web","devDependencies":{"vite":"^6.0.0"}}"#,
    )
    .unwrap();
    assert!(DmzLockfile::file_needs_refresh_for_workspace(
        &path,
        &ws_dir,
        Some("darwin-arm64"),
        true,
        None
    ));
}

#[test]
fn test_lockfile_staleness_detects_new_cargo_crate_and_digest_drift() {
    use crate::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let sidecar = root.join("codebase-ui");
    let tauri_dir = sidecar.join("frontend/src-tauri");
    std::fs::create_dir_all(&tauri_dir).unwrap();
    std::fs::write(
        sidecar.join("frontend/package.json"),
        r#"{"name":"frontend","dependencies":{"vite":"^6.0.0"}}"#,
    )
    .unwrap();
    std::fs::write(
        tauri_dir.join("Cargo.toml"),
        "[package]\nname=\"frontend\"\nversion=\"0.1.0\"\n[dependencies]\ncfb = \"0.10.0\"\nrfd = \"0.15.0\"\n",
    )
    .unwrap();

    let project = WorkspaceProjectConfig::new(
        "dual",
        vec![
            WorkspaceMember::new("primary", ".", MemberRole::Primary),
            WorkspaceMember::new("sidecar", "codebase-ui", MemberRole::Sidecar),
        ],
    );

    // 1. Legacy lockfile without inputs_digest that has npm + cfb, missing rfd -> MUST detect stale!
    let mut legacy_lock = DmzLockfile::with_platform(
        vec![
            LockedPackage {
                name: "vite".to_string(),
                version: "6.0.0".to_string(),
                ecosystem: Ecosystem::Npm,
                sha256: "aaa".to_string(),
                source_url: None,
                dependencies: vec![],
                member_id: Some("sidecar".to_string()),
            },
            LockedPackage {
                name: "cfb".to_string(),
                version: "0.10.0".to_string(),
                ecosystem: Ecosystem::Cargo,
                sha256: "bbb".to_string(),
                source_url: None,
                dependencies: vec![],
                member_id: Some("sidecar".to_string()),
            },
        ],
        None,
    );
    legacy_lock.include_dev = Some(true);
    legacy_lock.add_workspace(WorkspaceMetadata {
        id: "primary".to_string(),
        path: ".".to_string(),
        role: "primary".to_string(),
        ecosystem: None,
        closure_signature: None,
    });
    legacy_lock.add_workspace(WorkspaceMetadata {
        id: "sidecar".to_string(),
        path: "codebase-ui".to_string(),
        role: "sidecar".to_string(),
        ecosystem: None,
        closure_signature: None,
    });
    assert!(legacy_lock.is_stale_for_workspace(root, None, true, Some(&project)));

    // 2. Digest-stamped lockfile matches current manifests until Cargo.toml changes.
    legacy_lock.inputs_digest = Some(DmzLockfile::compute_workspace_inputs_digest(
        root,
        None,
        true,
        Some(&project),
    ));
    assert!(!legacy_lock.is_stale_for_workspace(root, None, true, Some(&project)));

    std::fs::write(
        tauri_dir.join("Cargo.toml"),
        "[package]\nname=\"frontend\"\nversion=\"0.1.0\"\n[dependencies]\ncfb = \"0.10.0\"\nrfd = \"0.15.2\"\n",
    )
    .unwrap();
    assert!(legacy_lock.is_stale_for_workspace(root, None, true, Some(&project)));
}

#[test]
fn test_lockfile_staleness_detects_python_abi_drift() {
    let temp = tempfile::tempdir().unwrap();
    let ws = temp.path();
    std::fs::create_dir_all(ws.join(".venv")).unwrap();
    std::fs::write(ws.join("requirements.txt"), "PyYAML==6.0.3\n").unwrap();
    std::fs::write(
        ws.join(".venv/pyvenv.cfg"),
        "version_info = 3.14.2.final.0\n",
    )
    .unwrap();

    let mut lock = DmzLockfile::new(vec![LockedPackage {
        name: "PyYAML".to_string(),
        version: "6.0.3".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: "abc".to_string(),
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    }]);
    lock.include_dev = Some(true);

    // Legacy PyPI lockfile with python_abi = None must be treated as stale when 3.14 is resolved.
    assert!(lock.is_stale_for_workspace(ws, None, true, None));

    // Stamped cp314 lockfile with current digest is fresh.
    lock.python_abi = Some("cp314".to_string());
    lock.inputs_digest = Some(DmzLockfile::compute_workspace_inputs_digest(
        ws, None, true, None,
    ));
    assert!(!lock.is_stale_for_workspace(ws, None, true, None));

    // Mismatched cp312 python_abi must trigger staleness even if inputs_digest matched previously.
    let mut old_abi_lock = lock.clone();
    old_abi_lock.python_abi = Some("cp312".to_string());
    assert!(old_abi_lock.is_stale_for_workspace(ws, None, true, None));

    // Switching .venv/pyvenv.cfg to 3.13 invalidates inputs_digest cleanly.
    std::fs::write(
        ws.join(".venv/pyvenv.cfg"),
        "version_info = 3.13.1.final.0\n",
    )
    .unwrap();
    assert!(lock.is_stale_for_workspace(ws, None, true, None));
}
