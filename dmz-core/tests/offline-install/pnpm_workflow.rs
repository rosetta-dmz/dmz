use dmz_core::install::{InstallConfig, InstallerEngine};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::{Ecosystem, ManifestScanner};
use dmz_core::resolver::ResolverEngine;
use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use tempfile::tempdir;

fn build_npm_tgz(pkg_name: &str, version: &str, extra_json: &str) -> Vec<u8> {
    let mut tar_builder = tar::Builder::new(Vec::new());
    let pkg_json = format!(
        "{{\"name\":\"{}\",\"version\":\"{}\"{}}}",
        pkg_name, version, extra_json
    );
    let index_js = format!("module.exports = {{ version: '{}' }};\n", version);

    for (rel, bytes) in &[
        ("package/package.json", pkg_json.as_bytes()),
        ("package/index.js", index_js.as_bytes()),
    ] {
        let mut hdr = tar::Header::new_gnu();
        hdr.set_size(bytes.len() as u64);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        tar_builder.append_data(&mut hdr, rel, *bytes).unwrap();
    }
    let tar_bytes = tar_builder.into_inner().unwrap();
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar_bytes).unwrap();
    gz.finish().unwrap()
}

#[test]
fn test_airgap_offline_install_pnpm_multi_version_workflow(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp_dir = tempdir()?;
    let workspace_path = tmp_dir.path().join("workspace");
    let store_path = tmp_dir.path().join(".dmz/store");
    fs::create_dir_all(&workspace_path)?;
    fs::create_dir_all(&store_path)?;

    // Write package.json + pnpm-lock.yaml (v9 snapshots + packages) with multi-version `ms`
    fs::write(
        workspace_path.join("package.json"),
        r#"{
  "name": "pnpm-multi-ver-app",
  "version": "1.0.0",
  "dependencies": {
    "debug": "^4.3.4",
    "@acme/logger": "^1.0.0",
    "ms": "2.1.3"
  }
}"#,
    )?;
    fs::write(
        workspace_path.join("pnpm-lock.yaml"),
        r#"lockfileVersion: '9.0'
packages:
  debug@4.3.4:
    resolution: {integrity: sha512-debug}
  '@acme/logger@1.0.0':
    resolution: {integrity: sha512-logger}
  ms@2.1.3:
    resolution: {integrity: sha512-ms213}
  ms@2.0.0:
    resolution: {integrity: sha512-ms200}
snapshots:
  debug@4.3.4:
    dependencies:
      ms: 2.1.3
  '@acme/logger@1.0.0':
    dependencies:
      ms: 2.0.0
"#,
    )?;

    // 1. Manifest scan + resolution must preserve both `ms@2.1.3` and `ms@2.0.0` and dep edges
    let manifests = ManifestScanner::scan_dir(&workspace_path)?;
    assert_eq!(manifests.len(), 1);
    let pnpm_deps = &manifests[0].dependencies;
    assert!(
        pnpm_deps
            .iter()
            .any(|d| d.name == "ms" && d.version_req == "2.1.3"),
        "Expected ms@2.1.3 in scanned pnpm deps: {:?}",
        pnpm_deps
    );
    assert!(
        pnpm_deps
            .iter()
            .any(|d| d.name == "ms" && d.version_req == "2.0.0"),
        "Expected ms@2.0.0 in scanned pnpm deps: {:?}",
        pnpm_deps
    );

    let resolved_lock = ResolverEngine::resolve_workspace(&workspace_path)?;
    let ms_versions: Vec<&str> = resolved_lock
        .packages
        .values()
        .filter(|p| p.name == "ms")
        .map(|p| p.version.as_str())
        .collect();
    assert!(
        ms_versions.contains(&"2.1.3") && ms_versions.contains(&"2.0.0"),
        "Resolver collapsed multi-version ms: {:?}",
        ms_versions
    );

    // 2. Populate store archives with real SHA-256 hashes for all locked packages
    let mut final_packages = BTreeMap::new();
    for (key, pkg) in resolved_lock.packages {
        let safe_name = pkg.name.replace('/', "__");
        let payload = build_npm_tgz(&pkg.name, &pkg.version, "");
        let pkg_dir = store_path.join(&safe_name);
        fs::create_dir_all(&pkg_dir)?;
        let archive_path = pkg_dir.join(format!("{}-{}.archive", safe_name, pkg.version));
        fs::write(&archive_path, &payload)?;
        let sha256 = dmz_core::archive::format_sha256_hex(&Sha256::digest(&payload));
        final_packages.insert(
            key,
            LockedPackage {
                sha256,
                ecosystem: Ecosystem::Npm,
                ..pkg
            },
        );
    }

    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        vcs: None,
        workspaces: BTreeMap::new(),
        closure_signature: DmzLockfile::compute_closure_hash(&final_packages),
        packages: final_packages,
    };
    let lockfile_path = workspace_path.join("dmz.lock");
    lockfile.save(&lockfile_path)?;

    // 3. Offline install
    let config = InstallConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path,
        store_dir: store_path,
    };
    let installed_count = InstallerEngine::install(&config)?;
    assert_eq!(installed_count, 4);

    // 4. Verify `.pnpm` virtual store isolation and sibling dependency symlinks
    assert!(workspace_path
        .join("node_modules/.pnpm/ms@2.1.3/node_modules/ms/package.json")
        .is_file());
    assert!(workspace_path
        .join("node_modules/.pnpm/ms@2.0.0/node_modules/ms/package.json")
        .is_file());
    assert!(workspace_path.join("node_modules/.modules.yaml").is_file());
    assert!(workspace_path.join("node_modules/ms/package.json").exists());

    let debug_ms_pkg = fs::read_to_string(
        workspace_path.join("node_modules/.pnpm/debug@4.3.4/node_modules/ms/package.json"),
    )?;
    assert!(
        debug_ms_pkg.contains("\"2.1.3\""),
        "debug must resolve ms@2.1.3 via .pnpm sibling link, got: {}",
        debug_ms_pkg
    );

    let logger_ms_pkg = fs::read_to_string(
        workspace_path.join("node_modules/.pnpm/@acme+logger@1.0.0/node_modules/ms/package.json"),
    )?;
    assert!(
        logger_ms_pkg.contains("\"2.0.0\""),
        "@acme/logger must resolve ms@2.0.0 via .pnpm sibling link, got: {}",
        logger_ms_pkg
    );

    Ok(())
}
