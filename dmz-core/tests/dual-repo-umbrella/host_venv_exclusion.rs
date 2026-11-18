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

    let wheel_bytes = fs::read(&wheel_path).unwrap();
    let pkg_pypi = LockedPackage {
        name: "dummy-pkg".to_string(),
        version: "1.0.0".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: DmzLockfile::hash_bytes(&wheel_bytes),
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
        ..Default::default()
    };
    AirGapExporter::export(&export_config).expect("Export failed");

    let target_dir = base.join("airgap-target");
    let import_config = ImportConfig {
        archive_path,
        target_store_dir: target_dir.join(".dmz/store"),
        target_workspace_dir: target_dir.clone(),
        verify_closure_hash: true,
        ..Default::default()
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
    let py_service_dir = target_dir.join("workspaces/py-service");
    let single_copy_unpacked = fs::read_dir(py_service_dir.join(".dmz/venv/lib"))
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            entry
                .path()
                .join("site-packages/dummy_pkg/__init__.py")
                .is_file()
        });
    assert!(
        single_copy_unpacked,
        "Bundled wheel must be unpacked offline into single-copy .dmz/venv site-packages"
    );

    let has_pth = fs::read_dir(py_service_dir.join(".venv/lib"))
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.path().join("site-packages/dmz_store.pth").is_file())
        || py_service_dir
            .join(".venv/Lib/site-packages/dmz_store.pth")
            .is_file();
    assert!(
        has_pth,
        ".venv site-packages must link to .dmz/venv via dmz_store.pth"
    );

    let has_duplicate_copy = fs::read_dir(py_service_dir.join(".venv/lib"))
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.path().join("site-packages/dummy_pkg").exists());
    assert!(
        !has_duplicate_copy,
        "Single-copy .venv layout must not duplicate unpacked packages under .venv/lib/"
    );

    // 3. Python member .vscode/settings.json must NOT emit unlabeled ${workspaceFolder}/.venv/...
    // (which causes VS Code multi-root ambiguity errors); instead the root `.code-workspace` scopes
    // the interpreter with `${workspaceFolder:py-service}/.venv/...`.
    let py_vscode =
        fs::read_to_string(target_dir.join("workspaces/py-service/.vscode/settings.json"))
            .expect("py-service .vscode/settings.json should exist");
    let py_vscode_json: serde_json::Value = serde_json::from_str(&py_vscode).unwrap();
    assert!(
        py_vscode_json.get("python.defaultInterpreterPath").is_none(),
        "Umbrella member .vscode/settings.json must not emit unlabeled python.defaultInterpreterPath, got:\n{}",
        py_vscode
    );
    assert_eq!(py_vscode_json["python.terminal.activateEnvironment"], true);

    let cw_raw = fs::read_to_string(target_dir.join("hermetic-dual-suite.code-workspace")).unwrap();
    assert!(
        cw_raw.contains("${workspaceFolder:py-service}/.venv/bin/python")
            || cw_raw.contains("${workspaceFolder:py-service}/.venv/Scripts/python.exe"),
        "Expected label-scoped .venv interpreter path in .code-workspace, got:\n{}",
        cw_raw
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

/// Regression: a Python member with locked PyPI packages but NO host `.venv` must never be
/// pointed at a non-existent interpreter (VS Code: "Default interpreter path ... could not be
/// resolved"), neither in the exported `.code-workspace` nor after import.
#[test]
fn test_locked_pypi_without_host_venv_never_emits_dangling_interpreter_path() {
    use std::io::Read;

    let temp = tempdir().expect("Failed to create tempdir");
    let base = temp.path();

    let py_dir = base.join("py-service");
    fs::create_dir_all(&py_dir).unwrap();
    fs::write(
        py_dir.join("pyproject.toml"),
        "[project]\nname = \"py-service\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(py_dir.join("requirements.txt"), "dummy-pkg==1.0.0\n").unwrap();
    fs::write(py_dir.join("main.py"), "import dummy_pkg\n").unwrap();

    let manifest = WorkspaceProjectConfig::new(
        "no-venv-suite",
        vec![
            WorkspaceMember::new("py-service", Path::new("."), MemberRole::Primary),
            WorkspaceMember::new("worker", Path::new("../worker"), MemberRole::Sidecar),
        ],
    );
    let worker_dir = base.join("worker");
    fs::create_dir_all(&worker_dir).unwrap();
    fs::write(worker_dir.join("worker.py"), "print('ok')\n").unwrap();
    manifest
        .save_to_file(&py_dir.join("dmz-workspace.toml"))
        .unwrap();

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
        zip.finish().unwrap();
    }
    let wheel_bytes = fs::read(&wheel_path).unwrap();
    let lockfile = DmzLockfile::new(vec![LockedPackage {
        name: "dummy-pkg".to_string(),
        version: "1.0.0".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: DmzLockfile::hash_bytes(&wheel_bytes),
        source_url: None,
        dependencies: vec![],
        member_id: Some("py-service".to_string()),
    }]);
    let lockfile_path = py_dir.join("dmz.lock");
    lockfile.save(&lockfile_path).unwrap();

    let archive_path = py_dir.join("dist/no-venv.tar.zst");
    AirGapExporter::export(&ExportConfig {
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
        ..Default::default()
    })
    .expect("Export failed");

    // 1. The exported .code-workspace must not reference a .venv that was never created
    let mut code_workspace = String::new();
    let decoder = zstd::stream::read::Decoder::new(fs::File::open(&archive_path).unwrap()).unwrap();
    for entry in tar::Archive::new(decoder).entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry
            .path()
            .unwrap()
            .to_string_lossy()
            .ends_with(".code-workspace")
        {
            entry.read_to_string(&mut code_workspace).unwrap();
        }
    }
    assert!(
        !code_workspace.is_empty(),
        "archive must embed .code-workspace"
    );
    assert!(
        !code_workspace.contains(".venv/bin/python")
            && !code_workspace.contains(".venv/Scripts/python.exe"),
        "exported .code-workspace must not reference a non-existent .venv:\n{}",
        code_workspace
    );

    // 2. After import every emitted interpreter path must resolve to a real file
    let target_dir = base.join("airgap-target");
    AirGapImporter::import(&ImportConfig {
        archive_path,
        target_store_dir: target_dir.join(".dmz/store"),
        target_workspace_dir: target_dir.clone(),
        verify_closure_hash: true,
        ..Default::default()
    })
    .expect("Import failed");

    let assert_resolves = |settings_json: &str,
                           folder: &Path,
                           label: Option<&str>,
                           origin: &str| {
        let v: serde_json::Value = serde_json::from_str(settings_json).unwrap();
        if let Some(p) = v
            .get("python.defaultInterpreterPath")
            .and_then(|p| p.as_str())
        {
            if let Some(lbl) = label {
                assert!(
                    p.starts_with(&format!("${{workspaceFolder:{}}}", lbl)),
                    "{origin}: .code-workspace interpreter path '{p}' should be scoped with ${{workspaceFolder:{lbl}}}"
                );
            }
            let dir_str = folder.to_string_lossy();
            let resolved = match label {
                Some(lbl) => p.replace(&format!("${{workspaceFolder:{}}}", lbl), &dir_str),
                None => p.to_string(),
            }
            .replace("${workspaceFolder}", &dir_str);
            assert!(
                Path::new(&resolved).is_file(),
                "{origin}: interpreter path '{p}' does not resolve to a file ({resolved})"
            );
        }
    };

    for entry in fs::read_dir(&target_dir).unwrap().flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "code-workspace") {
            let doc: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            for folder in doc["folders"].as_array().unwrap() {
                let folder_dir = target_dir.join(folder["path"].as_str().unwrap());
                let label = folder["name"].as_str();
                if let Some(settings) = folder.get("settings") {
                    assert_resolves(&settings.to_string(), &folder_dir, label, ".code-workspace");
                }
            }
        }
    }
    for member_dir in ["workspaces/py-service", "workspaces/worker"] {
        let settings_path = target_dir.join(member_dir).join(".vscode/settings.json");
        if settings_path.is_file() {
            assert_resolves(
                &fs::read_to_string(&settings_path).unwrap(),
                &target_dir.join(member_dir),
                None,
                member_dir,
            );
        }
    }
}
