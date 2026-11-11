use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ExportMode, ImportConfig};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn test_host_venv_excluded_and_target_venv_and_vscode_settings_materialized_offline() {
    let temp = tempdir().expect("Failed to create tempdir");
    let base = temp.path();

    // 1. Primary Python repo with a poisoned host .venv (simulating source machine)
    let py_dir = base.join("py-service");
    fs::create_dir_all(py_dir.join(".venv/bin")).unwrap();
    fs::write(
        py_dir.join(".venv/pyvenv.cfg"),
        "home = /opt/homebrew/opt/python@3.14/bin\n",
    )
    .unwrap();
    fs::write(
        py_dir.join(".venv/bin/poisoned_host_marker"),
        "must not be exported",
    )
    .unwrap();
    fs::write(
        py_dir.join("pyproject.toml"),
        "[project]\nname = \"py-service\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(py_dir.join("requirements.txt"), "dummy-pkg==1.0.0\n").unwrap();
    fs::write(py_dir.join("main.py"), "import dummy_pkg\n").unwrap();

    // 2. Companion Tauri repo with pnpm-lock.yaml and src-tauri
    let tauri_dir = base.join("tauri-ui");
    fs::create_dir_all(tauri_dir.join("src-tauri/src")).unwrap();
    fs::write(
        tauri_dir.join("package.json"),
        r#"{"name": "tauri-ui", "version": "0.1.0", "scripts": {"tauri": "tauri"}}"#,
    )
    .unwrap();
    fs::write(
        tauri_dir.join("pnpm-lock.yaml"),
        "lockfileVersion: '9.0'\npackages:\n  '@tauri-apps/cli@2.0.0':\n    resolution: {integrity: sha512-xyz}\n",
    )
    .unwrap();
    fs::write(
        tauri_dir.join("src-tauri/Cargo.toml"),
        "[package]\nname = \"tauri-ui\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(tauri_dir.join("src-tauri/src/main.rs"), "fn main() {}\n").unwrap();

    let manifest = WorkspaceProjectConfig::new(
        "hermetic-dual-suite",
        vec![
            WorkspaceMember::new("py-service", Path::new("."), MemberRole::Primary),
            WorkspaceMember::new("tauri-ui", Path::new("../tauri-ui"), MemberRole::Sidecar),
        ],
    );
    manifest
        .save_to_file(&py_dir.join("dmz-workspace.toml"))
        .unwrap();

    // Create a minimal valid wheel (.whl is a zip archive, stored as .archive in .dmz/store)
    let store_dir = py_dir.join(".dmz/store");
    let wheel_pkg_dir = store_dir.join("dummy-pkg");
    fs::create_dir_all(&wheel_pkg_dir).unwrap();
    let wheel_path = wheel_pkg_dir.join("dummy-pkg-1.0.0.archive");
    {
        let file = fs::File::create(&wheel_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("dummy_pkg/__init__.py", options).unwrap();
        zip.write_all(b"VALUE = 42\n").unwrap();
        zip.start_file("dummy_pkg-1.0.0.dist-info/WHEEL", options)
            .unwrap();
        zip.write_all(
            b"Wheel-Version: 1.0\nGenerator: dmz-test\nRoot-Is-Purelib: true\nTag: py3-none-any\n",
        )
        .unwrap();
        zip.start_file("dummy_pkg-1.0.0.dist-info/METADATA", options)
            .unwrap();
        zip.write_all(b"Metadata-Version: 2.1\nName: dummy-pkg\nVersion: 1.0.0\n")
            .unwrap();
        zip.finish().unwrap();
    }

    let pkg_pypi = LockedPackage {
        name: "dummy-pkg".to_string(),
        version: "1.0.0".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: DmzLockfile::hash_bytes(b"dummy-pkg:1.0.0:PyPI"),
        source_url: None,
        dependencies: vec![],
        member_id: Some("py-service".to_string()),
    };
    let lockfile = DmzLockfile::new(vec![pkg_pypi]);
    let lockfile_path = py_dir.join("dmz.lock");
    lockfile.save(&lockfile_path).unwrap();

    let archive_path = py_dir.join("dist/hermetic.tar.zst");
    let export_config = ExportConfig {
        workspace_path: py_dir.clone(),
        lockfile_path,
        store_dir,
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

    let target_dir = base.join("airgap-target");
    let import_config = ImportConfig {
        archive_path,
        target_store_dir: target_dir.join(".dmz/store"),
        target_workspace_dir: target_dir.clone(),
        verify_closure_hash: true,
    };
    AirGapImporter::import(&import_config).expect("Import failed");

    // 1. Poisoned host .venv file must NOT exist in imported workspace
    assert!(
        !target_dir
            .join("workspaces/py-service/.venv/bin/poisoned_host_marker")
            .exists(),
        "Host .venv must never be exported into air-gap bundle"
    );

    // 2. Fresh relocatable .venv and unpacked wheel must exist in py-service
    assert!(
        target_dir
            .join("workspaces/py-service/.venv/pyvenv.cfg")
            .is_file(),
        "Fresh .venv/pyvenv.cfg must be generated on import"
    );
    assert!(
        target_dir
            .join("workspaces/py-service/.venv/lib/python3/site-packages/dummy_pkg/__init__.py")
            .is_file(),
        "Bundled wheel must be unpacked offline into .venv site-packages"
    );

    // 3. Python member .vscode/settings.json has ${workspaceFolder}/.venv/bin/python
    let py_vscode =
        fs::read_to_string(target_dir.join("workspaces/py-service/.vscode/settings.json"))
            .expect("py-service .vscode/settings.json should exist");
    assert!(
        py_vscode.contains("${workspaceFolder}/.venv/bin/python"),
        "Expected scoped .venv interpreter path in py-service settings, got:\n{}",
        py_vscode
    );

    // 4. Tauri companion member must NOT have python.defaultInterpreterPath in its .vscode/settings.json
    let tauri_vscode_path = target_dir.join("workspaces/tauri-ui/.vscode/settings.json");
    if tauri_vscode_path.exists() {
        let tauri_vscode = fs::read_to_string(&tauri_vscode_path).unwrap();
        assert!(
            !tauri_vscode.contains("python.defaultInterpreterPath"),
            "Non-Python Tauri member must NOT have python.defaultInterpreterPath:\n{}",
            tauri_vscode
        );
    }

    // 5. Bogus per-member .code-workspace files must NOT be generated inside workspaces/<member>
    assert!(
        !target_dir
            .join("workspaces/py-service/py-service.code-workspace")
            .exists(),
        "Member folder must not have its own bogus .code-workspace file"
    );
    assert!(
        !target_dir
            .join("workspaces/tauri-ui/tauri-ui.code-workspace")
            .exists(),
        "Member folder must not have its own bogus .code-workspace file"
    );
}
