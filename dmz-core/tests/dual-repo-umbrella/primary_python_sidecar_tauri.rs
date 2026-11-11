use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ExportMode, ImportConfig};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};
use std::fs;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn test_dual_repo_umbrella_export_and_import_reversed_roles_python_primary_node_sidecar() {
    let temp = tempdir().expect("Failed to create tempdir");
    let base = temp.path();

    // Setup: Primary is Python Backend, Sidecar is Node / Tauri GUI
    let primary_dir = base.join("ai-backend-service");
    fs::create_dir_all(primary_dir.join("src")).unwrap();
    fs::write(
        primary_dir.join("pyproject.toml"),
        "[project]\nname = \"ai-backend-service\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        primary_dir.join("requirements.txt"),
        "fastapi==0.100.0\nuvicorn==0.22.0\n",
    )
    .unwrap();
    fs::write(
        primary_dir.join("src/main.py"),
        "print('AI Backend Primary Running')\n",
    )
    .unwrap();

    let sidecar_dir = base.join("desktop-ui");
    fs::create_dir_all(sidecar_dir.join("src-tauri/src")).unwrap();
    fs::write(
        sidecar_dir.join("package.json"),
        r#"{"name": "desktop-ui", "version": "0.1.0", "scripts": {"tauri": "tauri"}, "devDependencies": {"@tauri-apps/cli": "^2.0.0"}}"#,
    )
    .unwrap();
    fs::write(
        sidecar_dir.join("src-tauri/Cargo.toml"),
        "[package]\nname = \"desktop-ui-core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        sidecar_dir.join("src-tauri/src/main.rs"),
        "fn main() { println!(\"Tauri sidecar\"); }",
    )
    .unwrap();

    // Multi-workspace manifest
    let manifest = WorkspaceProjectConfig::new(
        "ai-desktop-system",
        vec![
            WorkspaceMember::new("backend", Path::new("."), MemberRole::Primary),
            WorkspaceMember::new("ui-client", Path::new("../desktop-ui"), MemberRole::Sidecar),
        ],
    );
    manifest
        .save_to_file(&primary_dir.join("dmz-workspace.toml"))
        .expect("Failed to write dmz-workspace.toml");

    let store_dir = primary_dir.join(".dmz/store");
    fs::create_dir_all(&store_dir).unwrap();
    let dummy_pkg_name = "test-pkg";
    let pkg_dir = store_dir.join(dummy_pkg_name);
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(pkg_dir.join("data.txt"), "dummy").unwrap();

    let pkg = LockedPackage {
        name: dummy_pkg_name.to_string(),
        version: "1.0.0".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: "dummy-sha".to_string(),
        source_url: None,
        dependencies: vec![],
        member_id: Some("ui-client".to_string()),
    };
    let lockfile = DmzLockfile::new(vec![pkg]);
    let lockfile_path = primary_dir.join("dmz.lock");
    lockfile.save(&lockfile_path).unwrap();

    // Export archive
    let archive_path = primary_dir.join("dist/ai_system.tar.zst");
    let export_config = ExportConfig {
        workspace_path: primary_dir.clone(),
        lockfile_path: lockfile_path.clone(),
        store_dir: store_dir.clone(),
        output_path: archive_path.clone(),
        zstd_level: 1,
        mode: ExportMode::Unified,
        target_platform: None,
        with_git_history: false,
        since: None,
        with_toolchain: false,
        multi_workspace: Some(manifest),
    };

    AirGapExporter::export(&export_config).expect("Export failed");
    assert!(archive_path.exists());

    // Import into target
    let target_dir = base.join("extracted-ai-system");
    let import_config = ImportConfig {
        archive_path: archive_path.clone(),
        target_store_dir: target_dir.join(".dmz/store"),
        target_workspace_dir: target_dir.clone(),
        verify_closure_hash: true,
    };

    let import_res = AirGapImporter::import(&import_config).expect("Import failed");
    assert_eq!(import_res.imported_packages, 1);

    // Verify file tree with preserved names under workspaces/ namespace
    assert!(target_dir.join("workspaces/backend/src/main.py").is_file());
    assert!(target_dir
        .join("workspaces/ui-client/package.json")
        .is_file());
    assert!(target_dir
        .join("workspaces/ui-client/src-tauri/Cargo.toml")
        .is_file());
    assert!(target_dir.join("dmz-workspace.toml").is_file());
    assert!(target_dir
        .join("ai-desktop-system.code-workspace")
        .is_file());

    // Verify offline hydration in reversed configuration:
    // a) Python settings written to primary Python folder
    let primary_py_settings = target_dir.join("workspaces/backend/.vscode/settings.json");
    assert!(primary_py_settings.is_file());
    let py_settings_content = fs::read_to_string(&primary_py_settings).unwrap();
    assert!(py_settings_content.contains("python.terminal.activateEnvironment"));

    // b) Tauri CLI shim materialized in sidecar Node/GUI folder
    let sidecar_tauri_shim = target_dir.join("workspaces/ui-client/node_modules/.bin/tauri");
    assert!(
        sidecar_tauri_shim.is_file(),
        "Expected Tauri CLI shim in sidecar desktop-ui directory"
    );
    if cfg!(windows) {
        assert!(
            target_dir
                .join("workspaces/ui-client/node_modules/.bin/tauri.cmd")
                .is_file(),
            "Expected tauri.cmd on Windows"
        );
        assert!(
            target_dir
                .join("workspaces/ui-client/node_modules/.bin/tauri.ps1")
                .is_file(),
            "Expected tauri.ps1 on Windows"
        );
    }

    // c) Cargo vendoring configured in sidecar Tauri crate
    let sidecar_cargo_conf = target_dir.join("workspaces/ui-client/src-tauri/.cargo/config.toml");
    assert!(sidecar_cargo_conf.is_file());
    let cargo_conf_str = fs::read_to_string(&sidecar_cargo_conf).unwrap();
    assert!(cargo_conf_str.contains("replace-with = \"vendored-sources\""));

    // d) Member parity symlinks exist (.dmz, dmz-workspace.toml, dmz.lock)
    assert!(
        target_dir.join("workspaces/backend/.dmz").exists(),
        "Backend member should have .dmz symlink"
    );
    assert!(
        target_dir.join("workspaces/ui-client/.dmz").exists(),
        "UI member should have .dmz symlink"
    );
}
