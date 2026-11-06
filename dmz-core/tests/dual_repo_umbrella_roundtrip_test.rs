use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ExportMode, ImportConfig};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};
use std::fs;
use std::io::Read;
use std::path::Path;
use tar::Archive as TarArchive;
use tempfile::tempdir;
use zstd::stream::read::Decoder as ZstdDecoder;

#[test]
fn test_dual_repo_umbrella_export_and_import_roundtrip() {
    let temp = tempdir().expect("Failed to create tempdir");
    let base = temp.path();

    // Setup host layout: Primary Tauri App + External Sibling Python Backend
    let primary_dir = base.join("my-tauri-app");
    fs::create_dir_all(primary_dir.join("src")).unwrap();
    fs::create_dir_all(primary_dir.join("src-tauri/src")).unwrap();
    fs::create_dir_all(primary_dir.join("documentation")).unwrap();

    fs::write(
        primary_dir.join("package.json"),
        r#"{"name": "my-tauri-app", "version": "0.1.0", "scripts": {"tauri": "tauri"}, "devDependencies": {"@tauri-apps/cli": "^2.0.0"}}"#,
    )
    .unwrap();

    fs::write(
        primary_dir.join("src-tauri/Cargo.toml"),
        "[package]\nname = \"my-tauri-app-core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        primary_dir.join("src-tauri/src/main.rs"),
        "fn main() { println!(\"Tauri primary\"); }",
    )
    .unwrap();

    fs::write(
        primary_dir.join("documentation/architecture.md"),
        "# System Architecture\nFrontend connects to sidecar via DMZ_SIDECAR_SOCK.\n",
    )
    .unwrap();

    let sidecar_dir = base.join("auth-py-backend");
    fs::create_dir_all(sidecar_dir.join("auth")).unwrap();
    fs::write(
        sidecar_dir.join("pyproject.toml"),
        "[project]\nname = \"auth-py-backend\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(
        sidecar_dir.join("requirements.txt"),
        "fastapi==0.100.0\nuvicorn==0.22.0\n",
    )
    .unwrap();
    fs::write(
        sidecar_dir.join("auth/main.py"),
        "import os\nprint('Auth sidecar running on socket', os.environ.get('DMZ_SIDECAR_SOCK'))\n",
    )
    .unwrap();

    // Multi-workspace manifest pointing to sibling sidecar
    let manifest = WorkspaceProjectConfig::new(
        "my-desktop-suite",
        vec![
            WorkspaceMember::new("frontend", Path::new("."), MemberRole::Primary),
            WorkspaceMember::new(
                "auth-service",
                Path::new("../auth-py-backend"),
                MemberRole::Sidecar,
            ),
        ],
    );
    manifest
        .save_to_file(&primary_dir.join("dmz-workspace.toml"))
        .expect("Failed to write dmz-workspace.toml");

    // Create dummy locked package and store artifact
    let store_dir = primary_dir.join(".dmz/store");
    fs::create_dir_all(&store_dir).unwrap();

    let dummy_pkg_name = "test-dep";
    let pkg_dir = store_dir.join(dummy_pkg_name);
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(pkg_dir.join("content.txt"), "dummy artifact").unwrap();

    let pkg = LockedPackage {
        name: dummy_pkg_name.to_string(),
        version: "1.0.0".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: "dummy-sha".to_string(),
        source_url: None,
        dependencies: vec![],
        member_id: Some("frontend".to_string()),
    };
    let lockfile = DmzLockfile::new(vec![pkg]);
    let lockfile_path = primary_dir.join("dmz.lock");
    lockfile.save(&lockfile_path).unwrap();

    // Export archive using AirGapExporter
    let archive_path = primary_dir.join("dist/bundle.tar.zst");
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

    let manifest_result = AirGapExporter::export(&export_config).expect("Export failed");
    assert!(archive_path.exists());
    assert_eq!(manifest_result.package_count, 1);

    // Inspect archive entries to verify Umbrella Master Structure
    let file = fs::File::open(&archive_path).expect("Failed to open archive");
    let decoder = ZstdDecoder::new(file).expect("Zstd decode error");
    let mut tar_archive = TarArchive::new(decoder);

    let mut entry_names = Vec::new();
    let mut normalized_manifest_str = String::new();
    let mut code_workspace_str = String::new();
    let mut readme_str = String::new();

    for entry_res in tar_archive.entries().expect("Failed to read tar entries") {
        let mut entry = entry_res.expect("Tar entry error");
        let path = entry.path().expect("Entry path error").to_path_buf();
        let path_str = path.to_string_lossy().replace('\\', "/");
        entry_names.push(path_str.clone());

        if path_str == "dmz-workspace.toml" {
            entry.read_to_string(&mut normalized_manifest_str).unwrap();
        } else if path_str.ends_with(".code-workspace") {
            entry.read_to_string(&mut code_workspace_str).unwrap();
        } else if path_str == "README.md" {
            entry.read_to_string(&mut readme_str).unwrap();
        }
    }

    // Verify preserved folder names in archive entries under workspaces/ namespace:
    assert!(
        entry_names
            .iter()
            .any(|p| p.contains("workspaces/frontend/package.json")),
        "Expected workspaces/frontend/package.json in archive, got: {:?}",
        entry_names
    );
    assert!(
        entry_names
            .iter()
            .any(|p| p.contains("workspaces/frontend/src-tauri/Cargo.toml")),
        "Expected workspaces/frontend/src-tauri/Cargo.toml in archive, got: {:?}",
        entry_names
    );
    assert!(
        entry_names
            .iter()
            .any(|p| p.contains("workspaces/auth-service/auth/main.py")),
        "Expected workspaces/auth-service/auth/main.py in archive, got: {:?}",
        entry_names
    );
    assert!(
        !entry_names
            .iter()
            .any(|p| p == "documentation" || p.starts_with("documentation/")),
        "Documentation must NOT be synthetically copied to master root level"
    );
    assert!(
        entry_names.contains(&"dmz-workspace.toml".to_string()),
        "Expected root dmz-workspace.toml in archive"
    );
    assert!(
        entry_names.contains(&"my-desktop-suite.code-workspace".to_string()),
        "Expected root my-desktop-suite.code-workspace in archive"
    );
    assert!(
        entry_names.contains(&"README.md".to_string()),
        "Expected root README.md in archive"
    );

    // Verify normalized dmz-workspace.toml content:
    assert!(
        normalized_manifest_str.contains("workspaces/frontend"),
        "Manifest should reference primary folder 'workspaces/frontend':\n{}",
        normalized_manifest_str
    );
    assert!(
        normalized_manifest_str.contains("workspaces/auth-service"),
        "Manifest should reference sidecar folder 'workspaces/auth-service':\n{}",
        normalized_manifest_str
    );

    // Verify .code-workspace schema and folder settings:
    let code_workspace_json: serde_json::Value =
        serde_json::from_str(&code_workspace_str).expect("Valid JSON code-workspace");
    let folders = code_workspace_json["folders"]
        .as_array()
        .expect("folders array");
    assert_eq!(folders.len(), 2); // primary and sidecar under workspaces/ (no synthetic root docs)
    assert!(code_workspace_str.contains("workspaces/frontend"));
    assert!(code_workspace_str.contains("workspaces/auth-service"));
    assert!(code_workspace_str.contains("python.defaultInterpreterPath"));
    assert!(code_workspace_str.contains("node_modules/.bin"));

    // Verify README.md quickstart instructions:
    assert!(readme_str.contains("my-desktop-suite.code-workspace"));
    assert!(readme_str.contains("dmz run --workspace ."));
    assert!(readme_str.contains("DMZ_SIDECAR_SOCK"));
    assert!(readme_str.contains("workspaces/frontend"));

    // 6. Import archive into a clean destination directory
    let target_dir = base.join("extracted-umbrella-suite");
    let import_config = ImportConfig {
        archive_path: archive_path.clone(),
        target_store_dir: target_dir.join(".dmz/store"),
        target_workspace_dir: target_dir.clone(),
        verify_closure_hash: true,
    };

    let import_res = AirGapImporter::import(&import_config).expect("Import failed");
    assert_eq!(import_res.imported_packages, 1);
    assert!(import_res.workspace_project.is_some());

    // Verify unpacked file tree in target directory: clean workspaces/ namespace
    assert!(target_dir
        .join("workspaces/frontend/package.json")
        .is_file());
    assert!(target_dir
        .join("workspaces/frontend/src-tauri/Cargo.toml")
        .is_file());
    assert!(target_dir
        .join("workspaces/auth-service/auth/main.py")
        .is_file());
    assert!(
        !target_dir.join("documentation").exists(),
        "Synthetic documentation must not exist at master root"
    );
    assert!(target_dir.join("dmz-workspace.toml").is_file());
    assert!(target_dir.join("my-desktop-suite.code-workspace").is_file());
    assert!(target_dir.join("README.md").is_file());

    // Verify offline dependency hydration:
    // a) Tauri CLI shim was materialized
    let tauri_shim = target_dir.join("workspaces/frontend/node_modules/.bin/tauri");
    assert!(
        tauri_shim.is_file(),
        "Expected tauri CLI shim at {:?}",
        tauri_shim
    );
    if cfg!(windows) {
        assert!(
            target_dir
                .join("workspaces/frontend/node_modules/.bin/tauri.cmd")
                .is_file(),
            "Expected tauri.cmd on Windows"
        );
        assert!(
            target_dir
                .join("workspaces/frontend/node_modules/.bin/tauri.ps1")
                .is_file(),
            "Expected tauri.ps1 on Windows"
        );
    }

    // b) Cargo vendoring was configured
    let tauri_cargo_config = target_dir.join("workspaces/frontend/src-tauri/.cargo/config.toml");
    assert!(tauri_cargo_config.is_file());
    let cargo_conf = fs::read_to_string(&tauri_cargo_config).unwrap();
    assert!(cargo_conf.contains("replace-with = \"vendored-sources\""));

    // c) Scoped Python settings were written to sidecar folder (and not at top level)
    let sidecar_vscode_settings = target_dir.join("workspaces/auth-service/.vscode/settings.json");
    assert!(sidecar_vscode_settings.is_file());
    let sidecar_settings_str = fs::read_to_string(&sidecar_vscode_settings).unwrap();
    assert!(sidecar_settings_str.contains("python.terminal.activateEnvironment"));

    let root_vscode_settings = target_dir.join(".vscode/settings.json");
    if root_vscode_settings.exists() {
        let root_settings_str = fs::read_to_string(&root_vscode_settings).unwrap();
        assert!(
            !root_settings_str.contains("python.defaultInterpreterPath"),
            "Root .vscode/settings.json must NOT contain python.defaultInterpreterPath for umbrella workspace"
        );
    }
}

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
}
