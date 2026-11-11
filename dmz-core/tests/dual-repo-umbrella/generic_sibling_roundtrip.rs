use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ExportMode, ImportConfig};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};
use std::fs;
use std::path::Path;
use tar::Archive as TarArchive;
use tempfile::tempdir;
use zstd::stream::read::Decoder as ZstdDecoder;

#[test]
fn test_dual_repo_airgap_export_and_import_roundtrip() {
    let temp = tempdir().expect("Failed to create tempdir");
    let base = temp.path();

    // 1. Setup host layout: Primary Workspace + External Sibling Sidecar
    let primary_dir = base.join("primary_app");
    fs::create_dir_all(primary_dir.join("src")).unwrap();
    fs::write(
        primary_dir.join("Cargo.toml"),
        "[package]\nname = \"primary_app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        primary_dir.join("src/main.rs"),
        "fn main() { println!(\"Primary app\"); }",
    )
    .unwrap();

    let sidecar_dir = base.join("sibling_sidecar");
    fs::create_dir_all(sidecar_dir.join("src")).unwrap();
    fs::write(
        sidecar_dir.join("pyproject.toml"),
        "[project]\nname = \"ai-engine\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(sidecar_dir.join("requirements.txt"), "fastapi==0.100.0\n").unwrap();
    fs::write(
        sidecar_dir.join("src/main.py"),
        "import os\nprint('Sidecar socket:', os.environ.get('DMZ_SIDECAR_SOCK'))\n",
    )
    .unwrap();

    // 2. Multi-workspace manifest pointing to sibling sidecar
    let manifest = WorkspaceProjectConfig::new(
        "dual-airgap-project",
        vec![
            WorkspaceMember::new("primary", Path::new("."), MemberRole::Primary),
            WorkspaceMember::new(
                "ai-engine",
                Path::new("../sibling_sidecar"),
                MemberRole::Sidecar,
            ),
        ],
    );
    manifest
        .save_to_file(&primary_dir.join("dmz-workspace.toml"))
        .expect("Failed to write dmz-workspace.toml");

    // 3. Create dummy locked package and store artifact
    let store_dir = primary_dir.join(".dmz/store");
    fs::create_dir_all(&store_dir).unwrap();

    let dummy_pkg_name = "test-pkg";
    let pkg_dir = store_dir.join(dummy_pkg_name);
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(pkg_dir.join("content.txt"), "dummy package artifact").unwrap();

    let pkg = LockedPackage {
        name: dummy_pkg_name.to_string(),
        version: "1.0.0".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: "dummy-sha".to_string(),
        source_url: None,
        dependencies: vec![],
        member_id: Some("primary".to_string()),
    };
    let lockfile = DmzLockfile::new(vec![pkg]);
    let lockfile_path = primary_dir.join("dmz.lock");
    lockfile.save(&lockfile_path).unwrap();

    // 4. Export archive using AirGapExporter
    let archive_path = primary_dir.join("dist/dual_bundle.tar.zst");
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

    // 5. Inspect archive entries to verify Umbrella Master Structure with preserved folder names
    let file = fs::File::open(&archive_path).expect("Failed to open archive");
    let decoder = ZstdDecoder::new(file).expect("Zstd decode error");
    let mut tar_archive = TarArchive::new(decoder);

    let mut entry_names = Vec::new();
    let mut normalized_manifest_str = String::new();

    for entry_res in tar_archive.entries().expect("Failed to read tar entries") {
        let mut entry = entry_res.expect("Tar entry error");
        let path = entry.path().expect("Entry path error").to_path_buf();
        let path_str = path.to_string_lossy().replace('\\', "/");
        entry_names.push(path_str.clone());

        if path_str == "dmz-workspace.toml" {
            use std::io::Read;
            entry.read_to_string(&mut normalized_manifest_str).unwrap();
        }
    }

    // Verify preserved folder names in archive entries under workspaces/ namespace:
    assert!(
        entry_names
            .iter()
            .any(|p| p.contains("workspaces/primary_app/Cargo.toml")),
        "Expected workspaces/primary_app/Cargo.toml in archive, got: {:?}",
        entry_names
    );
    assert!(
        entry_names
            .iter()
            .any(|p| p.contains("workspaces/ai-engine/src/main.py")),
        "Expected workspaces/ai-engine/src/main.py in archive, got: {:?}",
        entry_names
    );
    assert!(
        entry_names.contains(&"dmz-workspace.toml".to_string()),
        "Expected root dmz-workspace.toml in archive"
    );

    // Verify normalized dmz-workspace.toml references the workspaces/ namespace
    let (normalized_config, _) = WorkspaceProjectConfig::from_toml_str(&normalized_manifest_str)
        .map(|c| (c, ()))
        .unwrap();
    let sidecar_member = normalized_config
        .find_member("ai-engine")
        .expect("ai-engine member");
    assert_eq!(
        sidecar_member.path.to_str().unwrap().replace('\\', "/"),
        "workspaces/ai-engine"
    );

    let primary_member = normalized_config
        .find_member("primary")
        .expect("primary member");
    assert_eq!(
        primary_member.path.to_str().unwrap().replace('\\', "/"),
        "workspaces/primary_app"
    );

    // 6. Import archive into a clean destination directory
    let target_dir = base.join("extracted-dual-project");
    let import_config = ImportConfig {
        archive_path: archive_path.clone(),
        target_store_dir: target_dir.join(".dmz/store"),
        target_workspace_dir: target_dir.clone(),
        verify_closure_hash: true,
    };

    let import_res = AirGapImporter::import(&import_config).expect("Import failed");
    assert_eq!(import_res.imported_packages, 1);
    assert!(import_res.workspace_project.is_some());

    // 7. Verify unpacked file tree in target directory under workspaces/
    assert!(target_dir
        .join("workspaces/primary_app/Cargo.toml")
        .is_file());
    assert!(target_dir
        .join("workspaces/ai-engine/src/main.py")
        .is_file());
    assert!(target_dir
        .join("workspaces/ai-engine/pyproject.toml")
        .is_file());
    assert!(target_dir.join("dmz-workspace.toml").is_file());

    // Verify normalized dmz-workspace.toml is discovered and points to restored folders
    let (restored_cfg, _) =
        WorkspaceProjectConfig::discover_workspace_manifest(&target_dir).expect("Manifest found");
    let restored_sidecar = restored_cfg.find_member("ai-engine").unwrap();
    assert_eq!(
        restored_sidecar.path.to_str().unwrap(),
        "workspaces/ai-engine"
    );
    assert!(target_dir
        .join(&restored_sidecar.path)
        .join("src/main.py")
        .is_file());
}
