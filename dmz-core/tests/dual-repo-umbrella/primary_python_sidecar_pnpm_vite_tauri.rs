use dmz_core::archive::{AirGapExporter, AirGapImporter, ExportConfig, ExportMode, ImportConfig};
use dmz_core::resolver::{ResolveOptions, ResolverEngine};
use dmz_core::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};
use flate2::write::GzEncoder;
use flate2::Compression;
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::tempdir;

/// Builds a valid `.tar.gz` npm package archive in `<store>/<safe>/<safe>-<ver>.archive`
/// with a `package/package.json` and optional `package/<bin_rel>` script, returning its SHA-256.
fn write_mock_npm_store_pkg(
    store_dir: &Path,
    pkg_name: &str,
    version: &str,
    bin_entry: Option<(&str, &str)>,
) -> String {
    let mut tar_builder = tar::Builder::new(Vec::new());

    let pkg_json = match bin_entry {
        Some((cmd, rel)) => serde_json::json!({
            "name": pkg_name,
            "version": version,
            "bin": { cmd: rel }
        })
        .to_string(),
        None => serde_json::json!({
            "name": pkg_name,
            "version": version
        })
        .to_string(),
    };

    let mut hdr = tar::Header::new_gnu();
    hdr.set_size(pkg_json.len() as u64);
    hdr.set_mode(0o644);
    hdr.set_cksum();
    tar_builder
        .append_data(&mut hdr, "package/package.json", pkg_json.as_bytes())
        .unwrap();

    if let Some((cmd, rel)) = bin_entry {
        let script = format!("#!/usr/bin/env node\nconsole.log('{cmd}');\n");
        let mut bin_hdr = tar::Header::new_gnu();
        bin_hdr.set_size(script.len() as u64);
        bin_hdr.set_mode(0o755);
        bin_hdr.set_cksum();
        let rel_clean = rel.trim_start_matches("./");
        tar_builder
            .append_data(
                &mut bin_hdr,
                format!("package/{}", rel_clean),
                script.as_bytes(),
            )
            .unwrap();
    }

    let tar_bytes = tar_builder.into_inner().unwrap();
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar_bytes).unwrap();
    let payload = gz.finish().unwrap();

    let safe_name = pkg_name.replace('/', "__");
    let pkg_dir = store_dir.join(&safe_name);
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join(format!("{}-{}.archive", safe_name, version)),
        &payload,
    )
    .unwrap();
    dmz_core::resolver::lockfile::DmzLockfile::hash_bytes(&payload)
}

/// Builds a minimal pure-Python `.whl` archive in `<store>/<pkg>/<pkg>-<ver>.archive`,
/// returning its SHA-256.
fn write_mock_pypi_wheel(store_dir: &Path, pkg_name: &str, version: &str) -> String {
    let mut buf = Vec::new();
    {
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let opts = zip::write::SimpleFileOptions::default();
        zip.start_file(format!("{}/__init__.py", pkg_name), opts)
            .unwrap();
        zip.write_all(b"VERSION = '1.0.0'\n").unwrap();
        zip.start_file(format!("{}-{}.dist-info/METADATA", pkg_name, version), opts)
            .unwrap();
        zip.write_all(
            format!(
                "Metadata-Version: 2.1\nName: {}\nVersion: {}\n",
                pkg_name, version
            )
            .as_bytes(),
        )
        .unwrap();
        zip.finish().unwrap();
    }
    let pkg_dir = store_dir.join(pkg_name);
    fs::create_dir_all(&pkg_dir).unwrap();
    fs::write(
        pkg_dir.join(format!("{}-{}.archive", pkg_name, version)),
        &buf,
    )
    .unwrap();
    dmz_core::resolver::lockfile::DmzLockfile::hash_bytes(&buf)
}

/// Populates all non-local locked packages with verifiable mock store archives so `AirGapExporter`
/// and `AirGapImporter` execute the full real pipeline end-to-end offline.
fn populate_store_for_lockfile(
    store_dir: &Path,
    lockfile: &mut dmz_core::resolver::lockfile::DmzLockfile,
) {
    for pkg in lockfile.packages.values_mut() {
        if pkg.is_local_path() {
            continue;
        }
        pkg.sha256 = match pkg.ecosystem {
            dmz_core::resolver::manifest::Ecosystem::Npm => match pkg.name.as_str() {
                "vite" => write_mock_npm_store_pkg(
                    store_dir,
                    &pkg.name,
                    &pkg.version,
                    Some(("vite", "./bin/vite.js")),
                ),
                "@tauri-apps/cli" => write_mock_npm_store_pkg(
                    store_dir,
                    &pkg.name,
                    &pkg.version,
                    Some(("tauri", "./bin/tauri.js")),
                ),
                _ => write_mock_npm_store_pkg(store_dir, &pkg.name, &pkg.version, None),
            },
            dmz_core::resolver::manifest::Ecosystem::PyPI => {
                write_mock_pypi_wheel(store_dir, &pkg.name, &pkg.version)
            }
            dmz_core::resolver::manifest::Ecosystem::Cargo => {
                crate::write_mock_cargo_store_pkg(store_dir, &pkg.name, &pkg.version)
            }
            _ => continue,
        };
    }
    lockfile.closure_signature =
        dmz_core::resolver::lockfile::DmzLockfile::compute_closure_hash(&lockfile.packages);
}

#[test]
fn test_dual_repo_python_primary_pnpm_vite_tauri_sidecar_roundtrip() {
    let temp = tempdir().expect("tempdir");
    let base = temp.path();

    // Primary repo: Python backend with stale source `.vscode/settings.json`
    let primary_dir = base.join("ai-backend");
    fs::create_dir_all(primary_dir.join("src")).unwrap();
    fs::create_dir_all(primary_dir.join(".vscode")).unwrap();
    fs::write(
        primary_dir.join("pyproject.toml"),
        "[project]\nname = \"ai-backend\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(primary_dir.join("requirements.txt"), "fastapi==0.115.0\n").unwrap();
    fs::write(primary_dir.join("src/main.py"), "print('ok')\n").unwrap();
    fs::write(
        primary_dir.join(".vscode/settings.json"),
        r#"{"python.defaultInterpreterPath": "${workspaceFolder}/.venv/bin/python"}"#,
    )
    .unwrap();

    // Sidecar repo: Tauri + Vite + pnpm (including underscore npm dep + v9 lockfile)
    let sidecar_dir = base.join("desktop-ui");
    fs::create_dir_all(sidecar_dir.join("src-tauri/src")).unwrap();
    fs::write(
        sidecar_dir.join("package.json"),
        r#"{
  "name": "desktop-ui",
  "version": "0.1.0",
  "packageManager": "pnpm@10.22.0",
  "scripts": {
    "dev": "vite",
    "tauri": "tauri"
  },
  "dependencies": {
    "lodash_es": "^4.17.0"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2.2.0",
    "vite": "^6.0.0"
  }
}"#,
    )
    .unwrap();
    fs::write(
        sidecar_dir.join("pnpm-lock.yaml"),
        r#"lockfileVersion: '9.0'

importers:

  .:
    dependencies:
      lodash_es:
        specifier: ^4.17.21
        version: 4.17.21
    devDependencies:
      '@tauri-apps/cli':
        specifier: ^2.2.0
        version: 2.2.7
      vite:
        specifier: ^6.0.0
        version: 6.0.11

packages:

  '@tauri-apps/cli@2.2.7':
    resolution: {integrity: sha512-aaa}
    hasBin: true

  lodash_es@4.17.21:
    resolution: {integrity: sha512-bbb}

  esbuild@0.24.2:
    resolution: {integrity: sha512-ccc}

  vite@6.0.11:
    resolution: {integrity: sha512-ddd}
    hasBin: true

snapshots:

  '@tauri-apps/cli@2.2.7': {}

  lodash_es@4.17.21: {}

  esbuild@0.24.2: {}

  vite@6.0.11(@types/node@22.0.0):
    dependencies:
      esbuild: 0.24.2
"#,
    )
    .unwrap();
    fs::write(
        sidecar_dir.join("src-tauri/Cargo.toml"),
        "[package]\nname = \"desktop-ui-core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        sidecar_dir.join("src-tauri/tauri.conf.json"),
        r#"{"build": {"beforeDevCommand": "pnpm dev"}}"#,
    )
    .unwrap();
    fs::write(sidecar_dir.join("src-tauri/src/main.rs"), "fn main() {}").unwrap();

    let manifest = WorkspaceProjectConfig::new(
        "dual-tauri-system",
        vec![
            WorkspaceMember::new("backend", Path::new("."), MemberRole::Primary),
            WorkspaceMember::new("ui", Path::new("../desktop-ui"), MemberRole::Sidecar),
        ],
    );
    manifest
        .save_to_file(&primary_dir.join("dmz-workspace.toml"))
        .unwrap();

    let mut lockfile = ResolverEngine::resolve_multi_workspace_with_options(
        &primary_dir,
        &manifest,
        None,
        &ResolveOptions {
            target_platform: None,
            include_dev: true,
        },
    )
    .expect("resolve_multi_workspace");

    // Underscore-named package from pnpm-lock.yaml must not be truncated/dropped and must upgrade to 4.17.21
    let lodash_es = lockfile
        .packages
        .values()
        .find(|p| p.name == "lodash_es")
        .unwrap_or_else(|| {
            panic!(
                "Expected lodash_es to be preserved in dmz.lock, got: {:?}",
                lockfile.packages.keys().collect::<Vec<_>>()
            )
        });
    assert_eq!(
        lodash_es.version, "4.17.21",
        "Expected pnpm-lock.yaml to upgrade lodash_es ^4.17.0 -> 4.17.21"
    );
    assert!(
        lockfile.packages.values().any(|p| p.name == "vite"),
        "Expected vite in dmz.lock"
    );
    assert!(
        lockfile.packages.values().any(|p| p.name == "esbuild"),
        "Expected transitive esbuild in dmz.lock"
    );

    let store_dir = primary_dir.join(".dmz/store");
    fs::create_dir_all(&store_dir).unwrap();
    populate_store_for_lockfile(&store_dir, &mut lockfile);

    let lockfile_path = primary_dir.join("dmz.lock");
    lockfile.save(&lockfile_path).unwrap();

    let archive_path = primary_dir.join("dist/bundle.tar.zst");
    AirGapExporter::export(&ExportConfig {
        workspace_path: primary_dir.clone(),
        lockfile_path,
        store_dir,
        output_path: archive_path.clone(),
        zstd_level: 1,
        mode: ExportMode::Unified,
        include_dev: true,
        multi_workspace: Some(manifest),
        ..Default::default()
    })
    .expect("export");

    let target_dir = base.join("imported-umbrella");
    AirGapImporter::import(&ImportConfig {
        archive_path,
        target_store_dir: target_dir.join(".dmz/store"),
        target_workspace_dir: target_dir.clone(),
        verify_closure_hash: true,
        include_dev: true,
        install_dependencies: true,
    })
    .expect("import");

    // 1. Verify Node / pnpm / Vite / Tauri artifacts in sidecar
    let imported_ui = target_dir.join("workspaces/ui");
    assert!(
        imported_ui.join("node_modules/.bin/vite").is_file(),
        "Expected node_modules/.bin/vite shim"
    );
    assert!(
        imported_ui.join("node_modules/.bin/tauri").is_file(),
        "Expected node_modules/.bin/tauri shim"
    );
    assert!(
        imported_ui.join("node_modules/.modules.yaml").is_file(),
        "Expected pnpm .modules.yaml"
    );
    assert!(
        imported_ui
            .join("node_modules/.pnpm/vite@6.0.11/node_modules/esbuild")
            .exists(),
        "Expected pnpm virtual-store transitive link for esbuild"
    );

    // 2. Verify Python member `.vscode/settings.json` does NOT emit bare `${workspaceFolder}/.venv/...`
    //    when inside a multi-root `.code-workspace` umbrella!
    let backend_settings_raw =
        fs::read_to_string(target_dir.join("workspaces/backend/.vscode/settings.json")).unwrap();
    let backend_settings: serde_json::Value = serde_json::from_str(&backend_settings_raw).unwrap();
    if let Some(interp) = backend_settings
        .get("python.defaultInterpreterPath")
        .and_then(|v| v.as_str())
    {
        assert!(
            !interp.contains("${workspaceFolder}/"),
            "Umbrella member .vscode/settings.json must not emit unlabeled ${{workspaceFolder}}/ path, got: {}",
            interp
        );
    }

    // 3. Verify `<project>.code-workspace` scopes Python interpreter with folder label if `.venv/bin/python` exists
    let cw_raw = fs::read_to_string(target_dir.join("dual-tauri-system.code-workspace")).unwrap();
    let cw: serde_json::Value = serde_json::from_str(&cw_raw).unwrap();
    let folders = cw["folders"].as_array().unwrap();
    let backend_folder = folders.iter().find(|f| f["name"] == "backend").unwrap();
    if target_dir
        .join("workspaces/backend/.venv/bin/python")
        .is_file()
        || target_dir
            .join("workspaces/backend/.venv/Scripts/python.exe")
            .is_file()
    {
        let interp = backend_folder["settings"]["python.defaultInterpreterPath"]
            .as_str()
            .expect("Expected labeled defaultInterpreterPath in .code-workspace");
        assert!(
            interp.starts_with("${workspaceFolder:backend}/.venv/"),
            "Expected ${{workspaceFolder:backend}}/.venv/... in .code-workspace, got: {}",
            interp
        );
    }
}

#[test]
fn test_dual_repo_pnpm_workspace_monorepo_vite_sidecar_roundtrip() {
    let temp = tempdir().expect("tempdir");
    let base = temp.path();

    // Primary repo: stdlib-only Python backend (0 external PyPI packages)
    let primary_dir = base.join("py-service");
    fs::create_dir_all(primary_dir.join("src")).unwrap();
    fs::write(
        primary_dir.join("pyproject.toml"),
        "[project]\nname = \"py-service\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    fs::write(primary_dir.join("src/main.py"), "print('stdlib ok')\n").unwrap();

    // Sidecar repo: pnpm monorepo using `pnpm-workspace.yaml` with `apps/desktop/package.json`
    let sidecar_dir = base.join("ui-monorepo");
    fs::create_dir_all(sidecar_dir.join("apps/desktop/src-tauri/src")).unwrap();
    fs::write(
        sidecar_dir.join("pnpm-workspace.yaml"),
        "packages:\n  - 'apps/*'\n",
    )
    .unwrap();
    fs::write(
        sidecar_dir.join("package.json"),
        r#"{
  "name": "ui-monorepo",
  "private": true,
  "packageManager": "pnpm@10.22.0",
  "scripts": {
    "tauri": "pnpm --filter desktop tauri"
  }
}"#,
    )
    .unwrap();
    fs::write(
        sidecar_dir.join("apps/desktop/package.json"),
        r#"{
  "name": "desktop",
  "version": "0.1.0",
  "scripts": {
    "dev": "vite",
    "tauri": "tauri"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2.2.0",
    "vite": "^6.0.0"
  }
}"#,
    )
    .unwrap();
    fs::write(
        sidecar_dir.join("pnpm-lock.yaml"),
        r#"lockfileVersion: '9.0'

importers:

  .: {}

  apps/desktop:
    devDependencies:
      '@tauri-apps/cli':
        specifier: ^2.2.0
        version: 2.2.7
      vite:
        specifier: ^6.0.0
        version: 6.0.11

packages:

  '@tauri-apps/cli@2.2.7':
    resolution: {integrity: sha512-aaa}
    hasBin: true

  vite@6.0.11:
    resolution: {integrity: sha512-ddd}
    hasBin: true

snapshots:

  '@tauri-apps/cli@2.2.7': {}
  vite@6.0.11: {}
"#,
    )
    .unwrap();
    fs::write(
        sidecar_dir.join("apps/desktop/src-tauri/Cargo.toml"),
        "[package]\nname = \"desktop-core\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(
        sidecar_dir.join("apps/desktop/src-tauri/tauri.conf.json"),
        r#"{"build": {"beforeDevCommand": "pnpm dev"}}"#,
    )
    .unwrap();
    fs::write(
        sidecar_dir.join("apps/desktop/src-tauri/src/main.rs"),
        "fn main() {}",
    )
    .unwrap();

    let manifest = WorkspaceProjectConfig::new(
        "mono-tauri-system",
        vec![
            WorkspaceMember::new("backend", Path::new("."), MemberRole::Primary),
            WorkspaceMember::new("ui", Path::new("../ui-monorepo"), MemberRole::Sidecar),
        ],
    );
    manifest
        .save_to_file(&primary_dir.join("dmz-workspace.toml"))
        .unwrap();

    let mut lockfile = ResolverEngine::resolve_multi_workspace_with_options(
        &primary_dir,
        &manifest,
        None,
        &ResolveOptions {
            target_platform: None,
            include_dev: true,
        },
    )
    .expect("resolve_multi_workspace");

    assert!(
        lockfile.packages.values().any(|p| p.name == "vite"),
        "Expected vite from pnpm-workspace.yaml subpackage to be resolved"
    );
    assert!(
        lockfile
            .packages
            .values()
            .any(|p| p.name == "@tauri-apps/cli"),
        "Expected @tauri-apps/cli from pnpm-workspace.yaml subpackage to be resolved"
    );

    let store_dir = primary_dir.join(".dmz/store");
    fs::create_dir_all(&store_dir).unwrap();
    populate_store_for_lockfile(&store_dir, &mut lockfile);

    let lockfile_path = primary_dir.join("dmz.lock");
    lockfile.save(&lockfile_path).unwrap();

    let archive_path = primary_dir.join("dist/bundle.tar.zst");
    AirGapExporter::export(&ExportConfig {
        workspace_path: primary_dir.clone(),
        lockfile_path,
        store_dir,
        output_path: archive_path.clone(),
        zstd_level: 1,
        mode: ExportMode::Unified,
        include_dev: true,
        multi_workspace: Some(manifest),
        ..Default::default()
    })
    .expect("export");

    let target_dir = base.join("imported-monorepo");
    AirGapImporter::import(&ImportConfig {
        archive_path,
        target_store_dir: target_dir.join(".dmz/store"),
        target_workspace_dir: target_dir.clone(),
        verify_closure_hash: true,
        include_dev: true,
        install_dependencies: true,
    })
    .expect("import");

    // Subpackage `apps/desktop` must ALSO have node_modules, .bin/vite, .bin/tauri, and .modules.yaml!
    let desktop_dir = target_dir.join("workspaces/ui/apps/desktop");
    assert!(
        desktop_dir.join("node_modules").is_dir(),
        "Expected apps/desktop/node_modules to be materialized for pnpm workspace member"
    );
    assert!(
        desktop_dir.join("node_modules/.bin/vite").is_file(),
        "Expected apps/desktop/node_modules/.bin/vite shim"
    );
    assert!(
        desktop_dir.join("node_modules/.bin/tauri").is_file(),
        "Expected apps/desktop/node_modules/.bin/tauri shim"
    );
    assert!(
        desktop_dir.join("node_modules/.modules.yaml").is_file(),
        "Expected apps/desktop/node_modules/.modules.yaml"
    );

    // Even when Python backend had 0 wheels, `.venv/bin/python` should be hydrated if host python3 exists
    let host_has_python = which::which("python3").is_ok() || which::which("python").is_ok();
    if host_has_python {
        assert!(
            target_dir
                .join("workspaces/backend/.venv/bin/python")
                .is_file()
                || target_dir
                    .join("workspaces/backend/.venv/Scripts/python.exe")
                    .is_file(),
            "Expected .venv python interpreter to be hydrated even with 0 PyPI wheels"
        );
    }
}

#[test]
fn test_dual_repo_nested_frontend_and_toolchain_minimal_pnpm_bundle() {
    let temp = tempdir().unwrap();
    let base = temp.path();

    // Primary: Python Repo
    let primary_dir = base.join("api-primary");
    fs::create_dir_all(&primary_dir).unwrap();
    fs::write(
        primary_dir.join("pyproject.toml"),
        "[project]\nname = \"api-primary\"\nversion = \"0.1.0\"\ndependencies = []\n",
    )
    .unwrap();

    // Sidecar: Nested `frontend/` + `frontend/src-tauri/` container repo
    let sidecar_dir = base.join("desktop-shell");
    let frontend_dir = sidecar_dir.join("frontend");
    let tauri_dir = frontend_dir.join("src-tauri/src");
    fs::create_dir_all(&tauri_dir).unwrap();
    fs::write(
        frontend_dir.join("package.json"),
        r#"{
  "name": "desktop-frontend",
  "version": "1.0.0",
  "scripts": { "dev": "vite", "tauri": "tauri" },
  "devDependencies": { "vite": "^6.0.11" }
}"#,
    )
    .unwrap();
    fs::write(
        frontend_dir.join("pnpm-lock.yaml"),
        r#"lockfileVersion: '9.0'
importers:
  .:
    devDependencies:
      vite:
        specifier: ^6.0.11
        version: 6.0.11
packages:
  vite@6.0.11:
    resolution: {integrity: sha512-ddd}
    hasBin: true
snapshots:
  vite@6.0.11: {}
"#,
    )
    .unwrap();
    fs::write(
        frontend_dir.join("src-tauri/Cargo.toml"),
        "[package]\nname = \"nested-tauri\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(tauri_dir.join("main.rs"), "fn main() {}").unwrap();

    let manifest = WorkspaceProjectConfig::new(
        "nested-dual",
        vec![
            WorkspaceMember::new("api", Path::new("."), MemberRole::Primary),
            WorkspaceMember::new("shell", Path::new("../desktop-shell"), MemberRole::Sidecar),
        ],
    );

    // 1. Minimal profile toolchain discovery must still detect Node/pnpm from sidecar member root!
    let discovered_tc =
        dmz_core::toolchain::ToolchainProvider::discover_for_workspace_with_project_and_options(
            &primary_dir,
            None,
            Some(&manifest),
            dmz_core::toolchain::ToolchainDetectOptions { include_dev: true },
            dmz_core::toolchain::ToolchainProfile::Minimal,
            None,
        );
    if let Some(node_tc) = discovered_tc
        .iter()
        .find(|tc| tc.ecosystem == dmz_core::toolchain::ToolchainEcosystem::Node)
    {
        if which::which("pnpm").is_ok() {
            assert!(
                node_tc.bin_files.iter().any(|(_, rel)| rel.contains("pnpm")),
                "Expected Minimal Node toolchain for dual-repo to include pnpm when host has pnpm: {:?}",
                node_tc.bin_files
            );
        }
    }

    // 2. Multi-workspace resolve + export + import roundtrip with nested frontend/
    let mut lockfile = ResolverEngine::resolve_multi_workspace_with_options(
        &primary_dir,
        &manifest,
        None,
        &ResolveOptions {
            target_platform: None,
            include_dev: true,
        },
    )
    .expect("resolve nested dual-repo");
    assert!(
        lockfile.packages.values().any(|p| p.name == "vite"),
        "Expected vite inside nested frontend/ to be resolved"
    );

    let store_dir = primary_dir.join(".dmz/store");
    fs::create_dir_all(&store_dir).unwrap();
    populate_store_for_lockfile(&store_dir, &mut lockfile);

    let lockfile_path = primary_dir.join("dmz.lock");
    lockfile.save(&lockfile_path).unwrap();

    let archive_path = primary_dir.join("dist/nested.tar.zst");
    AirGapExporter::export(&ExportConfig {
        workspace_path: primary_dir.clone(),
        lockfile_path,
        store_dir,
        output_path: archive_path.clone(),
        zstd_level: 1,
        mode: ExportMode::Unified,
        include_dev: true,
        multi_workspace: Some(manifest),
        ..Default::default()
    })
    .expect("export nested dual-repo");

    let target_dir = base.join("imported-nested");
    AirGapImporter::import(&ImportConfig {
        archive_path,
        target_store_dir: target_dir.join(".dmz/store"),
        target_workspace_dir: target_dir.clone(),
        verify_closure_hash: true,
        include_dev: true,
        install_dependencies: true,
    })
    .expect("import nested dual-repo");

    let imported_frontend = target_dir.join("workspaces/shell/frontend");
    assert!(
        imported_frontend.join("node_modules/.bin/vite").is_file(),
        "Expected nested workspaces/shell/frontend/node_modules/.bin/vite shim"
    );
    assert!(
        imported_frontend
            .join("node_modules/.modules.yaml")
            .is_file(),
        "Expected nested workspaces/shell/frontend/node_modules/.modules.yaml"
    );

    // Execute the offline `vite` shim via node if host has `node` available
    if which::which("node").is_ok() {
        let out = std::process::Command::new(imported_frontend.join("node_modules/.bin/vite"))
            .current_dir(&imported_frontend)
            .output()
            .expect("run offline vite shim");
        assert!(
            out.status.success(),
            "Expected offline vite shim to run cleanly: stderr={}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            stdout.contains("vite"),
            "Expected offline vite shim stdout to contain 'vite', got '{}'",
            stdout
        );
    }

    // 3. Test InstallerEngine fallback when member_id is None on lockfile packages
    let no_member_packages: Vec<_> = lockfile
        .packages
        .values()
        .cloned()
        .map(|mut pkg| {
            pkg.member_id = None;
            pkg
        })
        .collect();
    let no_member_lock = dmz_core::resolver::lockfile::DmzLockfile::new(no_member_packages);
    let fallback_ws = base.join("fallback-install");
    let fallback_frontend = fallback_ws.join("frontend");
    fs::create_dir_all(&fallback_frontend).unwrap();
    fs::write(
        fallback_frontend.join("package.json"),
        r#"{"name":"fb","devDependencies":{"vite":"^6.0.11"}}"#,
    )
    .unwrap();
    fs::write(
        fallback_frontend.join("pnpm-lock.yaml"),
        "lockfileVersion: '9.0'\n",
    )
    .unwrap();
    let no_member_lock_path = fallback_ws.join("dmz.lock");
    no_member_lock.save(&no_member_lock_path).unwrap();

    dmz_core::install::InstallerEngine::install(&dmz_core::install::InstallConfig {
        workspace_path: fallback_ws,
        lockfile_path: no_member_lock_path,
        store_dir: target_dir.join(".dmz/store"),
    })
    .expect("fallback install with member_id=None");
    assert!(
        fallback_frontend.join("node_modules/.bin/vite").is_file(),
        "Expected nested fallback install to materialize node_modules/.bin/vite when member_id is None"
    );
}

#[test]
fn test_dual_repo_root_plus_nested_frontend_and_code_only_store_reuse_import() {
    let temp = tempdir().unwrap();
    let base = temp.path();

    let primary_dir = base.join("api-primary");
    fs::create_dir_all(&primary_dir).unwrap();
    fs::write(
        primary_dir.join("pyproject.toml"),
        "[project]\nname = \"api-primary\"\nversion = \"0.1.0\"\ndependencies = []\n",
    )
    .unwrap();

    // Sidecar has BOTH root wrapper package.json (without `"workspaces"`) AND nested `frontend/`
    let sidecar_dir = base.join("hybrid-ui");
    let frontend_dir = sidecar_dir.join("frontend");
    fs::create_dir_all(frontend_dir.join("src-tauri/src")).unwrap();
    fs::write(
        sidecar_dir.join("package.json"),
        r#"{"name":"root-wrapper","scripts":{"dev":"pnpm --dir frontend dev"}}"#,
    )
    .unwrap();
    fs::write(
        frontend_dir.join("package.json"),
        r#"{"name":"frontend-app","devDependencies":{"vite":"^6.0.11"}}"#,
    )
    .unwrap();
    fs::write(
        frontend_dir.join("pnpm-lock.yaml"),
        r#"lockfileVersion: '9.0'
importers:
  .:
    devDependencies:
      vite:
        specifier: ^6.0.11
        version: 6.0.11
packages:
  vite@6.0.11:
    resolution: {integrity: sha512-ddd}
    hasBin: true
snapshots:
  vite@6.0.11: {}
"#,
    )
    .unwrap();
    fs::write(
        frontend_dir.join("src-tauri/Cargo.toml"),
        "[package]\nname = \"hybrid-tauri\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    fs::write(frontend_dir.join("src-tauri/src/main.rs"), "fn main() {}").unwrap();

    let manifest = WorkspaceProjectConfig::new(
        "wrapper-plus-frontend",
        vec![
            WorkspaceMember::new("api", Path::new("."), MemberRole::Primary),
            WorkspaceMember::new("ui", Path::new("../hybrid-ui"), MemberRole::Sidecar),
        ],
    );

    let mut lockfile = ResolverEngine::resolve_multi_workspace_with_options(
        &primary_dir,
        &manifest,
        None,
        &ResolveOptions {
            target_platform: None,
            include_dev: true,
        },
    )
    .expect("resolve wrapper + nested frontend");
    assert!(
        lockfile.packages.values().any(|p| p.name == "vite"),
        "Expected vite inside undeclared nested frontend/ to resolve even when root package.json exists"
    );

    let store_dir = primary_dir.join(".dmz/store");
    fs::create_dir_all(&store_dir).unwrap();
    populate_store_for_lockfile(&store_dir, &mut lockfile);
    let lockfile_path = primary_dir.join("dmz.lock");
    lockfile.save(&lockfile_path).unwrap();

    // First import Unified bundle to populate target store
    let unified_archive = primary_dir.join("dist/unified.tar.zst");
    AirGapExporter::export(&ExportConfig {
        workspace_path: primary_dir.clone(),
        lockfile_path: lockfile_path.clone(),
        store_dir: store_dir.clone(),
        output_path: unified_archive.clone(),
        zstd_level: 1,
        mode: ExportMode::Unified,
        include_dev: true,
        multi_workspace: Some(manifest.clone()),
        ..Default::default()
    })
    .expect("export unified");

    let target_dir = base.join("imported-wrapper");
    AirGapImporter::import(&ImportConfig {
        archive_path: unified_archive,
        target_store_dir: target_dir.join(".dmz/store"),
        target_workspace_dir: target_dir.clone(),
        verify_closure_hash: true,
        include_dev: true,
        install_dependencies: true,
    })
    .expect("import unified");

    let imported_fe = target_dir.join("workspaces/ui/frontend");
    assert!(
        imported_fe.join("node_modules/.bin/vite").is_file(),
        "Expected undeclared nested frontend/node_modules/.bin/vite"
    );

    // Re-importing a CodeOnly bundle when `.dmz/store` already exists must still install node_modules
    fs::remove_dir_all(imported_fe.join("node_modules")).unwrap();
    let code_archive = primary_dir.join("dist/code_only.tar.zst");
    AirGapExporter::export(&ExportConfig {
        workspace_path: primary_dir,
        lockfile_path,
        store_dir,
        output_path: code_archive.clone(),
        zstd_level: 1,
        mode: ExportMode::CodeOnly,
        include_dev: true,
        multi_workspace: Some(manifest),
        ..Default::default()
    })
    .expect("export code_only");

    AirGapImporter::import(&ImportConfig {
        archive_path: code_archive,
        target_store_dir: target_dir.join(".dmz/store"),
        target_workspace_dir: target_dir.clone(),
        verify_closure_hash: true,
        include_dev: true,
        install_dependencies: true,
    })
    .expect("import code_only with existing store");

    assert!(
        imported_fe.join("node_modules/.bin/vite").is_file(),
        "Expected CodeOnly import with cached store to materialize frontend/node_modules/.bin/vite"
    );

    // Generated env.sh must guard /usr/bin/xcrun behind filesystem CLT checks so it never triggers GUI popups
    dmz_core::toolchain::ToolchainSetup::generate_activator_scripts(&target_dir).unwrap();
    let env_sh = fs::read_to_string(target_dir.join(".dmz/env.sh")).unwrap();
    if env_sh.contains("/usr/bin/xcrun") {
        assert!(
            env_sh.contains("/var/db/xcode_select_link"),
            "Expected env.sh to guard /usr/bin/xcrun with filesystem CLT check"
        );
    }
}
