use super::*;
use tempfile::tempdir;

#[test]
fn test_install_error_display() {
    let err = InstallError::IntegrityMismatch {
        expected: "abc".into(),
        actual: "def".into(),
    };
    assert!(err
        .to_string()
        .contains("Store Artifact Integrity Mismatch"));

    let err2 = InstallError::ArtifactMissing {
        package: "foo".into(),
        path: PathBuf::from("/store/foo.archive"),
    };
    assert!(err2.to_string().contains("Missing cached store artifact"));

    let err3 = InstallError::UnsupportedEcosystem("unknown_lang".into());
    assert!(err3.to_string().contains("Ecosystem not supported"));
}

#[test]
fn test_install_fails_fast_when_store_missing() {
    let temp = tempdir().unwrap();
    let ws = temp.path().join("workspace");
    fs::create_dir_all(&ws).unwrap();
    let lockfile_path = ws.join("dmz.lock");

    let packages = std::collections::BTreeMap::new();
    let closure_signature = DmzLockfile::compute_closure_hash(&packages);
    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        include_dev: None,
        inputs_digest: None,
        vcs: None,
        workspaces: std::collections::BTreeMap::new(),
        closure_signature,
        packages,
    };
    lockfile.save(&lockfile_path).unwrap();

    let config = InstallConfig {
        workspace_path: ws,
        lockfile_path,
        store_dir: temp.path().join("non_existent_store"),
    };

    let result = InstallerEngine::install(&config);
    assert!(matches!(result, Err(InstallError::IoError(_))));
}

#[test]
fn test_locate_store_artifact_missing() {
    let temp = tempdir().unwrap();
    let store = temp.path().join("store");
    fs::create_dir_all(&store).unwrap();

    let pkg = LockedPackage {
        name: "test-pkg".into(),
        version: "1.0.0".into(),
        ecosystem: Ecosystem::Cargo,
        sha256: "deadbeef".into(),
        source_url: None,
        dependencies: vec![],
        member_id: None,
    };

    let result = InstallerEngine::locate_store_artifact(&store, &pkg);
    assert!(matches!(result, Err(InstallError::ArtifactMissing { .. })));
}

#[test]
fn test_verify_artifact_integrity_mismatch() {
    let temp = tempdir().unwrap();
    let archive_path = temp.path().join("artifact.archive");
    fs::write(&archive_path, b"corrupted content").unwrap();

    let pkg = LockedPackage {
        name: "test-pkg".into(),
        version: "1.0.0".into(),
        ecosystem: Ecosystem::Cargo,
        sha256: "0000000000000000000000000000000000000000000000000000000000000000".into(),
        source_url: None,
        dependencies: vec![],
        member_id: None,
    };

    let result = InstallerEngine::verify_artifact_integrity(&archive_path, &pkg);
    assert!(matches!(
        result,
        Err(InstallError::IntegrityMismatch { .. })
    ));
}

#[test]
fn test_locate_store_artifact_rejects_traversal() {
    let temp = tempdir().unwrap();
    let store = temp.path().join("store");
    fs::create_dir_all(&store).unwrap();

    for traversal_name in &["..", "../escape", "../../etc/shadow", "safe/../../escape"] {
        let pkg = LockedPackage {
            name: traversal_name.to_string(),
            version: "1.0.0".into(),
            ecosystem: Ecosystem::Cargo,
            sha256: "deadbeef".into(),
            source_url: None,
            dependencies: vec![],
            member_id: None,
        };

        let result = InstallerEngine::locate_store_artifact(&store, &pkg);
        assert!(
            matches!(result, Err(InstallError::MapperError(_))),
            "Expected traversal rejection for {}",
            traversal_name
        );
    }
}

#[test]
fn test_resolve_member_path_containment() {
    let temp = tempdir().unwrap();
    let ws = temp.path().join("workspace");
    fs::create_dir_all(&ws).unwrap();
    let canonical_ws = fs::canonicalize(&ws).unwrap();

    let valid_member = ws.join("packages/app");
    fs::create_dir_all(&valid_member).unwrap();
    let resolved = InstallerEngine::resolve_member_path(&ws, Path::new("packages/app"));
    assert!(resolved.starts_with(&canonical_ws));

    let escaped_path = Path::new("../../../../etc");
    let resolved = InstallerEngine::resolve_member_path(&ws, escaped_path);
    assert_eq!(resolved, canonical_ws);
}

#[test]
fn test_verify_artifact_integrity_rejects_dummy_placeholders() {
    let temp = tempdir().unwrap();
    let archive_path = temp.path().join("artifact.archive");
    let bytes = b"real archive payload";
    fs::write(&archive_path, bytes).unwrap();
    let real_hash = {
        use sha2::Digest as _;
        crate::archive::format_sha256_hex(&sha2::Sha256::digest(bytes))
    };

    for dummy_hash in &["", "dummy", "pending", "0000000000000000"] {
        let pkg = LockedPackage {
            name: "test-pkg".into(),
            version: "1.0.0".into(),
            ecosystem: Ecosystem::Cargo,
            sha256: (*dummy_hash).to_string(),
            source_url: None,
            dependencies: vec![],
            member_id: None,
        };
        let res = InstallerEngine::verify_artifact_integrity(&archive_path, &pkg);
        assert!(
            matches!(res, Err(InstallError::IntegrityMismatch { .. })),
            "Expected dummy hash {:?} to be rejected",
            dummy_hash
        );
    }

    let valid_pkg = LockedPackage {
        name: "test-pkg".into(),
        version: "1.0.0".into(),
        ecosystem: Ecosystem::Cargo,
        sha256: real_hash,
        source_url: None,
        dependencies: vec![],
        member_id: None,
    };
    assert!(InstallerEngine::verify_artifact_integrity(&archive_path, &valid_pkg).is_ok());
}

#[test]
fn test_resolve_target_member_dir_bundler_and_maven_gradle_fallback() {
    let temp = tempdir().unwrap();
    let ws = temp.path();
    let ruby_dir = ws.join("ruby-service");
    let jvm_dir = ws.join("jvm-service");
    fs::create_dir_all(&ruby_dir).unwrap();
    fs::create_dir_all(&jvm_dir).unwrap();
    fs::write(ruby_dir.join("Gemfile"), "source 'https://rubygems.org'\n").unwrap();
    fs::write(jvm_dir.join("pom.xml"), "<project></project>\n").unwrap();

    let cfg = WorkspaceProjectConfig::new(
        "polyglot",
        vec![
            crate::workspace::WorkspaceMember::new(
                "ruby",
                Path::new("ruby-service"),
                crate::workspace::MemberRole::Sidecar,
            ),
            crate::workspace::WorkspaceMember::new(
                "jvm",
                Path::new("jvm-service"),
                crate::workspace::MemberRole::Service,
            ),
        ],
    );

    let bundler_pkg = LockedPackage {
        name: "rack".into(),
        version: "3.0.0".into(),
        ecosystem: Ecosystem::Bundler,
        sha256: "abc".into(),
        source_url: None,
        dependencies: vec![],
        member_id: None,
    };
    let resolved_ruby = InstallerEngine::resolve_target_workspace(ws, &bundler_pkg, Some(&cfg));
    assert_eq!(resolved_ruby, fs::canonicalize(&ruby_dir).unwrap());

    let maven_pkg = LockedPackage {
        name: "com.example:lib".into(),
        version: "1.0.0".into(),
        ecosystem: Ecosystem::MavenGradle,
        sha256: "def".into(),
        source_url: None,
        dependencies: vec![],
        member_id: None,
    };
    let resolved_jvm = InstallerEngine::resolve_target_workspace(ws, &maven_pkg, Some(&cfg));
    assert_eq!(resolved_jvm, fs::canonicalize(&jvm_dir).unwrap());
}

#[test]
fn test_cargo_config_toml_merge_updates_stale_vendor_dir_and_preserves_settings() {
    let temp = tempdir().unwrap();
    let cfg_path = temp.path().join(".cargo/config.toml");
    fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
    fs::write(
        &cfg_path,
        "[build]\njobs = 4\n\n[source.crates-io]\nreplace-with = \"vendored-sources\"\n\n[source.vendored-sources]\ndirectory = \"../stale/vendor\"\n",
    )
    .unwrap();

    mappers::CargoMapper::ensure_vendored_cargo_config(&cfg_path, "../../.cargo/vendor").unwrap();

    assert_eq!(
        mappers::CargoMapper::read_vendored_source_directory(&cfg_path).as_deref(),
        Some("../../.cargo/vendor")
    );
    let updated = fs::read_to_string(&cfg_path).unwrap();
    assert!(updated.contains("jobs = 4"));
}

#[test]
fn test_verify_cargo_vendor_completeness_catches_missing_crate_and_passes_when_populated() {
    let temp = tempdir().unwrap();
    let ws = temp.path();
    let sidecar = ws.join("workspaces/codebase-ui");
    let tauri_dir = sidecar.join("frontend/src-tauri");
    fs::create_dir_all(tauri_dir.join(".cargo")).unwrap();
    fs::write(
        tauri_dir.join("Cargo.toml"),
        "[package]\nname=\"frontend\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();
    mappers::CargoMapper::ensure_vendored_cargo_config(
        &tauri_dir.join(".cargo/config.toml"),
        "../../.cargo/vendor",
    )
    .unwrap();

    let cfg = WorkspaceProjectConfig::new(
        "dual",
        vec![crate::workspace::WorkspaceMember::new(
            "sidecar",
            Path::new("workspaces/codebase-ui"),
            crate::workspace::MemberRole::Sidecar,
        )],
    );
    let lock = DmzLockfile::new(vec![LockedPackage {
        name: "rfd".to_string(),
        version: "0.15.2".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: "abc".to_string(),
        source_url: None,
        dependencies: vec![],
        member_id: Some("sidecar".to_string()),
    }]);

    let missing_err = InstallerEngine::verify_cargo_vendor_completeness(ws, &lock, Some(&cfg))
        .expect_err("missing vendored rfd must fail completeness check");
    assert!(missing_err.to_string().contains("rfd@0.15.2"));

    let vendored_crate = sidecar.join(".cargo/vendor/rfd-0.15.2");
    fs::create_dir_all(&vendored_crate).unwrap();
    fs::write(
        vendored_crate.join("Cargo.toml"),
        "[package]\nname=\"rfd\"\nversion=\"0.15.2\"\n",
    )
    .unwrap();

    InstallerEngine::verify_cargo_vendor_completeness(ws, &lock, Some(&cfg))
        .expect("populated vendor dir must pass completeness check");

    // Adding a new direct dependency (`serde`) in Cargo.toml or Cargo.lock without updating dmz.lock
    // MUST still fail completeness verification (zero circular self-check).
    fs::write(
        tauri_dir.join("Cargo.toml"),
        "[package]\nname=\"frontend\"\nversion=\"0.1.0\"\n\n[dependencies]\nserde = \"1.0.197\"\n",
    )
    .unwrap();
    let direct_missing = InstallerEngine::verify_cargo_vendor_completeness(ws, &lock, Some(&cfg))
        .expect_err(
        "missing Cargo.toml direct dep must fail completeness check even if absent from dmz.lock",
    );
    assert!(direct_missing.to_string().contains("serde"));
}
