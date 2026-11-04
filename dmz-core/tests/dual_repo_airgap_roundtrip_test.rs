use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ImportConfig};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_dual_repo_airgap_export_and_import_roundtrip() {
    let temp = tempdir().unwrap();
    let root = temp.path();

    // 1. Primary workspace: Rust frontend
    let primary_dir = root.join("primary_app");
    fs::create_dir_all(primary_dir.join("src")).unwrap();
    fs::write(
        primary_dir.join("Cargo.toml"),
        "[package]\nname = \"primary_app\"\nversion = \"1.0.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        primary_dir.join("src/main.rs"),
        "fn main() { println!(\"Primary\"); }\n",
    )
    .unwrap();

    // 2. External sibling companion sidecar: Python backend
    let sidecar_dir = root.join("sibling_sidecar");
    fs::create_dir_all(&sidecar_dir).unwrap();
    fs::write(sidecar_dir.join("requirements.txt"), "flask>=2.0\n").unwrap();
    fs::write(
        sidecar_dir.join("server.py"),
        "import sys\nprint('Sidecar alive')\n",
    )
    .unwrap();

    // 3. Project configuration: dmz-workspace.toml in primary pointing to external sidecar
    let primary_m = WorkspaceMember::new("primary", ".", MemberRole::Primary);
    let sidecar_m = WorkspaceMember::new("ai-engine", "../sibling_sidecar", MemberRole::Sidecar);
    let project_cfg = WorkspaceProjectConfig::new("my-dual-app", vec![primary_m, sidecar_m]);
    project_cfg
        .save_to_file(&primary_dir.join("dmz-workspace.toml"))
        .unwrap();

    // 4. Lockfile with packages from both members
    let lockfile = DmzLockfile::new(vec![
        LockedPackage {
            name: "serde".into(),
            version: "1.0.200".into(),
            ecosystem: Ecosystem::Cargo,
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
            source_url: None,
            dependencies: vec![],
            member_id: Some("primary".into()),
        },
        LockedPackage {
            name: "flask".into(),
            version: "2.3.0".into(),
            ecosystem: Ecosystem::PyPI,
            sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
            source_url: None,
            dependencies: vec![],
            member_id: Some("ai-engine".into()),
        },
    ]);
    lockfile.save(&primary_dir.join("dmz.lock")).unwrap();

    // Create mock store cache for both packages
    let store_dir = primary_dir.join(".dmz/store");
    fs::create_dir_all(store_dir.join("serde/1.0.200")).unwrap();
    fs::write(
        store_dir.join("serde/1.0.200/serde.crate"),
        b"mock serde crate",
    )
    .unwrap();
    fs::create_dir_all(store_dir.join("flask/2.3.0")).unwrap();
    fs::write(store_dir.join("flask/2.3.0/flask.whl"), b"mock flask wheel").unwrap();

    // 5. Export bundle via AirGapExporter
    let bundle_path = root.join("dist/dual_bundle.tar.zst");
    let export_cfg = ExportConfig {
        workspace_path: primary_dir.clone(),
        lockfile_path: primary_dir.join("dmz.lock"),
        store_dir: store_dir.clone(),
        output_path: bundle_path.clone(),
        ..Default::default()
    };
    let manifest = AirGapExporter::export(&export_cfg).expect("Dual-repo export should succeed");

    assert!(bundle_path.exists());
    assert_eq!(manifest.package_count, 2);

    let exported_proj = manifest
        .workspace_project
        .expect("Manifest must contain workspace_project");
    assert_eq!(exported_proj.name, "my-dual-app");
    assert_eq!(exported_proj.members.len(), 2);

    let sc_member = exported_proj
        .find_member("ai-engine")
        .expect("ai-engine member must exist");
    assert_eq!(
        sc_member.path,
        std::path::PathBuf::from("sidecars/ai-engine")
    );

    // 6. Import into a clean target machine workspace
    let target_temp = tempdir().unwrap();
    let target_ws = target_temp.path().join("extracted_project");
    let target_store = target_ws.join(".dmz/store");

    let import_cfg = ImportConfig {
        archive_path: bundle_path,
        target_store_dir: target_store.clone(),
        target_workspace_dir: target_ws.clone(),
        verify_closure_hash: true,
    };
    let import_res = AirGapImporter::import(&import_cfg).expect("Dual-repo import should succeed");

    assert_eq!(import_res.imported_packages, 2);
    let imported_proj = import_res
        .workspace_project
        .expect("Import result must contain workspace_project");
    assert_eq!(imported_proj.members.len(), 2);

    // 7. Verify PRIMARY files unpacked
    assert!(
        target_ws.join("Cargo.toml").exists(),
        "Primary Cargo.toml must exist"
    );
    assert!(
        target_ws.join("src/main.rs").exists(),
        "Primary src/main.rs must exist"
    );
    assert_eq!(
        fs::read_to_string(target_ws.join("src/main.rs")).unwrap(),
        "fn main() { println!(\"Primary\"); }\n"
    );

    // 8. CRITICAL: Verify SIDECAR files unpacked into sidecars/ai-engine
    assert!(
        target_ws
            .join("sidecars/ai-engine/requirements.txt")
            .exists(),
        "Sidecar requirements.txt must be unpacked into sidecars/ai-engine"
    );
    assert!(
        target_ws.join("sidecars/ai-engine/server.py").exists(),
        "Sidecar server.py must be unpacked into sidecars/ai-engine"
    );
    assert_eq!(
        fs::read_to_string(target_ws.join("sidecars/ai-engine/server.py")).unwrap(),
        "import sys\nprint('Sidecar alive')\n"
    );

    // 9. Verify normalized dmz-workspace.toml in target workspace loads and points to extracted sidecar
    let target_manifest = target_ws.join("dmz-workspace.toml");
    assert!(
        target_manifest.exists(),
        "Target dmz-workspace.toml must exist"
    );
    let reloaded_cfg = WorkspaceProjectConfig::load_from_file(&target_manifest).unwrap();
    assert_eq!(reloaded_cfg.name, "my-dual-app");

    let target_sc = reloaded_cfg.find_member("ai-engine").unwrap();
    assert_eq!(
        target_sc.path,
        std::path::PathBuf::from("sidecars/ai-engine")
    );

    // Verify sidecar path resolves directly relative to target workspace root
    let target_sc_full = target_ws.join(&target_sc.path);
    assert!(
        target_sc_full.join("server.py").exists(),
        "Resolved sidecar path must point to valid directory"
    );
}
