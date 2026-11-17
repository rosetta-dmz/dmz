use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ExportMode, ImportConfig};
use dmz_core::install::{InstallConfig, InstallerEngine};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage, WorkspaceMetadata};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::workspace::{
    discover_tauri_cargo_dirs_excluding, MemberRole, WorkspaceMember, WorkspaceProjectConfig,
};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn test_dual_repo_sidecar_nested_tauri_cargo_vendor_staleness_and_roundtrip() {
    let temp = tempdir().unwrap();
    let base = temp.path();

    // Primary repo at umbrella root with >600 source files before `codebase-ui/`
    let ws_root = base.join("umbrella");
    fs::create_dir_all(ws_root.join("assets")).unwrap();
    for idx in 0..650 {
        fs::write(
            ws_root.join(format!("assets/item_{:04}.py", idx)),
            "x = 1\n",
        )
        .unwrap();
    }
    fs::write(
        ws_root.join("pyproject.toml"),
        "[project]\nname=\"api\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();

    // Sidecar nested inside umbrella with frontend/src-tauri declaring `cfb` and `rfd`
    let sidecar_dir = ws_root.join("codebase-ui");
    let fe_dir = sidecar_dir.join("frontend");
    let tauri_dir = fe_dir.join("src-tauri");
    fs::create_dir_all(tauri_dir.join("src")).unwrap();
    fs::write(fe_dir.join("package.json"), r#"{"name":"frontend"}"#).unwrap();
    fs::write(
        tauri_dir.join("Cargo.toml"),
        "[package]\nname = \"frontend\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\ncfb = \"0.10.0\"\nrfd = \"0.15.2\"\n",
    )
    .unwrap();
    fs::write(
        tauri_dir.join("Cargo.lock"),
        r#"version = 3

[[package]]
name = "cfb"
version = "0.10.0"
checksum = "cfb-lock-checksum-abc"

[[package]]
name = "rfd"
version = "0.15.2"
checksum = "rfd-lock-checksum-xyz"
"#,
    )
    .unwrap();
    fs::write(tauri_dir.join("src/main.rs"), "fn main() {}").unwrap();

    let project = WorkspaceProjectConfig::new(
        "dual-umbrella",
        vec![
            WorkspaceMember::new("primary", Path::new("."), MemberRole::Primary),
            WorkspaceMember::new("codebase-ui", Path::new("codebase-ui"), MemberRole::Sidecar),
        ],
    );

    // 1. Primary discovery must exclude sibling `codebase-ui`
    let primary_cargo =
        discover_tauri_cargo_dirs_excluding(&ws_root, std::slice::from_ref(&sidecar_dir));
    assert!(primary_cargo.is_empty());

    // 2. Store + legacy lockfile missing `rfd` is detected as stale
    let store_dir = ws_root.join(".dmz/store");
    let cfb_sha = crate::write_mock_cargo_store_pkg(&store_dir, "cfb", "0.10.0");
    let rfd_sha = crate::write_mock_cargo_store_pkg(&store_dir, "rfd", "0.15.2");

    let mut partial_lock = DmzLockfile::new(vec![LockedPackage {
        name: "cfb".to_string(),
        version: "0.10.0".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: cfb_sha.clone(),
        source_url: None,
        dependencies: vec![],
        member_id: Some("codebase-ui".to_string()),
    }]);
    partial_lock.include_dev = Some(true);
    partial_lock.add_workspace(WorkspaceMetadata {
        id: "primary".to_string(),
        path: ".".to_string(),
        role: "primary".to_string(),
        ecosystem: None,
        closure_signature: None,
    });
    partial_lock.add_workspace(WorkspaceMetadata {
        id: "codebase-ui".to_string(),
        path: "codebase-ui".to_string(),
        role: "sidecar".to_string(),
        ecosystem: None,
        closure_signature: None,
    });
    assert!(partial_lock.is_stale_for_workspace(&ws_root, None, true, Some(&project)));

    // 3. Complete lockfile with digest passes staleness, exports & imports offline
    let mut full_lock = DmzLockfile::new(vec![
        LockedPackage {
            name: "cfb".to_string(),
            version: "0.10.0".to_string(),
            ecosystem: Ecosystem::Cargo,
            sha256: cfb_sha,
            source_url: None,
            dependencies: vec![],
            member_id: Some("codebase-ui".to_string()),
        },
        LockedPackage {
            name: "rfd".to_string(),
            version: "0.15.2".to_string(),
            ecosystem: Ecosystem::Cargo,
            sha256: rfd_sha,
            source_url: None,
            dependencies: vec![],
            member_id: Some("codebase-ui".to_string()),
        },
    ]);
    full_lock.include_dev = Some(true);
    full_lock.workspaces = partial_lock.workspaces.clone();
    full_lock.inputs_digest = Some(DmzLockfile::compute_workspace_inputs_digest(
        &ws_root,
        None,
        true,
        Some(&project),
    ));
    assert!(!full_lock.is_stale_for_workspace(&ws_root, None, true, Some(&project)));

    let lockfile_path = ws_root.join("dmz.lock");
    full_lock.save(&lockfile_path).unwrap();

    let archive_path = ws_root.join("dist/dual_cargo.tar.zst");
    AirGapExporter::export(&ExportConfig {
        workspace_path: ws_root.clone(),
        lockfile_path: lockfile_path.clone(),
        store_dir: store_dir.clone(),
        output_path: archive_path.clone(),
        zstd_level: 1,
        mode: ExportMode::Unified,
        include_dev: true,
        multi_workspace: Some(project.clone()),
        ..Default::default()
    })
    .expect("export dual-repo cargo bundle");

    let target_ws = base.join("imported-dual");
    AirGapImporter::import(&ImportConfig {
        archive_path,
        target_store_dir: target_ws.join(".dmz/store"),
        target_workspace_dir: target_ws.clone(),
        verify_closure_hash: true,
        include_dev: true,
        install_dependencies: true,
    })
    .expect("import dual-repo cargo bundle");

    let imported_sidecar = target_ws.join("workspaces/codebase-ui");
    assert!(imported_sidecar
        .join(".cargo/vendor/rfd-0.15.2/Cargo.toml")
        .is_file());
    assert!(imported_sidecar
        .join(".cargo/vendor/cfb-0.10.0/Cargo.toml")
        .is_file());
    assert!(!target_ws.join("workspaces/umbrella/.cargo").exists());

    let rfd_checksum =
        fs::read_to_string(imported_sidecar.join(".cargo/vendor/rfd-0.15.2/.cargo-checksum.json"))
            .unwrap();
    assert!(
        rfd_checksum.contains("rfd-lock-checksum-xyz"),
        "Expected nested frontend/src-tauri/Cargo.lock checksum parity, got: {rfd_checksum}"
    );

    let cfg_toml =
        fs::read_to_string(imported_sidecar.join("frontend/src-tauri/.cargo/config.toml")).unwrap();
    assert!(cfg_toml.contains("../../.cargo/vendor"));

    // 4. Removing vendored `rfd` self-heals cleanly via re-install and passes completeness check
    fs::remove_dir_all(imported_sidecar.join(".cargo/vendor/rfd-0.15.2")).unwrap();
    let healed = InstallerEngine::install(&InstallConfig {
        workspace_path: target_ws.clone(),
        lockfile_path: target_ws.join("dmz.lock"),
        store_dir: target_ws.join(".dmz/store"),
    })
    .expect("expected re-install to self-heal rfd and pass completeness verification");
    assert_eq!(healed, 2);
    assert!(imported_sidecar
        .join(".cargo/vendor/rfd-0.15.2/Cargo.toml")
        .is_file());

    // 5. DepsOnly bundle retains embedded dmz-workspace.toml member routing on import
    let deps_archive = ws_root.join("dist/dual_deps_only.tar.zst");
    AirGapExporter::export(&ExportConfig {
        workspace_path: ws_root.clone(),
        lockfile_path,
        store_dir,
        output_path: deps_archive.clone(),
        zstd_level: 1,
        mode: ExportMode::DepsOnly,
        include_dev: true,
        multi_workspace: Some(project),
        ..Default::default()
    })
    .expect("export dual-repo DepsOnly bundle");

    let deps_target = base.join("imported-deps-only");
    fs::create_dir_all(deps_target.join("workspaces/codebase-ui/frontend/src-tauri")).unwrap();
    fs::write(
        deps_target.join("workspaces/codebase-ui/frontend/src-tauri/Cargo.toml"),
        "[package]\nname=\"frontend\"\nversion=\"0.1.0\"\n",
    )
    .unwrap();
    let deps_res = AirGapImporter::import(&ImportConfig {
        archive_path: deps_archive,
        target_store_dir: deps_target.join(".dmz/store"),
        target_workspace_dir: deps_target.clone(),
        verify_closure_hash: true,
        include_dev: true,
        install_dependencies: true,
    })
    .expect("import dual-repo DepsOnly bundle");
    assert!(deps_res.workspace_project.is_some());
    assert!(deps_target
        .join("workspaces/codebase-ui/.cargo/vendor/rfd-0.15.2/Cargo.toml")
        .is_file());

    // 6. AirGapExporter must reject exporting a stale dmz.lock missing `rfd` required by Cargo.toml
    let stale_lock_path = ws_root.join("stale_dmz.lock");
    partial_lock.save(&stale_lock_path).unwrap();
    let export_err = AirGapExporter::export(&ExportConfig {
        workspace_path: ws_root.clone(),
        lockfile_path: stale_lock_path,
        store_dir: ws_root.join(".dmz/store"),
        output_path: ws_root.join("dist/should_fail.tar.zst"),
        zstd_level: 1,
        mode: ExportMode::Unified,
        include_dev: true,
        multi_workspace: Some(WorkspaceProjectConfig::new(
            "dual-umbrella",
            vec![
                WorkspaceMember::new("primary", Path::new("."), MemberRole::Primary),
                WorkspaceMember::new("codebase-ui", Path::new("codebase-ui"), MemberRole::Sidecar),
            ],
        )),
        ..Default::default()
    })
    .expect_err("export must refuse incomplete Cargo lockfile missing rfd");
    assert!(export_err.to_string().contains("rfd"));
}
