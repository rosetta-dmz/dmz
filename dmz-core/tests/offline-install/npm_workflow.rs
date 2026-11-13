use dmz_core::install::{InstallConfig, InstallerEngine};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use flate2::write::GzEncoder;
use flate2::Compression;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_airgap_offline_install_workflow() -> Result<(), Box<dyn std::error::Error>> {
    let tmp_dir = tempdir()?;
    let workspace_path = tmp_dir.path().join("workspace");
    let store_path = tmp_dir.path().join(".dmz/store");

    fs::create_dir_all(&workspace_path)?;
    fs::create_dir_all(&store_path)?;

    let pkg_name = "lodash";
    let pkg_version = "4.17.21";
    let safe_name = pkg_name.replace('/', "__");

    let pkg_store_dir = store_path.join(&safe_name);
    fs::create_dir_all(&pkg_store_dir)?;

    // Construct a valid minimal gzipped tar archive (.tgz) for testing
    let mut tar_builder = tar::Builder::new(Vec::new());
    let dummy_content = b"{\"name\": \"lodash\"}";
    let mut header = tar::Header::new_gnu();
    header.set_size(dummy_content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();

    tar_builder.append_data(&mut header, "package.json", &dummy_content[..])?;
    let tar_bytes = tar_builder.into_inner()?;

    let mut gz_encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut gz_encoder, &tar_bytes)?;
    let payload = gz_encoder.finish()?;

    // Save valid archive to store
    let archive_filename = format!("{}-{}.archive", safe_name, pkg_version);
    let archive_path = pkg_store_dir.join(&archive_filename);
    fs::write(&archive_path, &payload)?;

    // Compute accurate SHA-256 hash of the valid archive
    let computed_sha256 = dmz_core::archive::format_sha256_hex(&Sha256::digest(&payload));

    let locked_pkg = LockedPackage {
        name: pkg_name.to_string(),
        version: pkg_version.to_string(),
        ecosystem: Ecosystem::Npm,
        sha256: computed_sha256,
        source_url: None,
        dependencies: vec![],
        member_id: None,
    };

    let mut packages = BTreeMap::new();
    packages.insert(format!("{}:{}", pkg_name, pkg_version), locked_pkg);

    let closure_sig = DmzLockfile::compute_closure_hash(&packages);
    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        vcs: None,
        workspaces: BTreeMap::new(),
        closure_signature: closure_sig,
        packages,
    };

    let lockfile_path = workspace_path.join("dmz.lock");
    lockfile.save(&lockfile_path)?;

    // Execute DMZ native offline install
    let config = InstallConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path,
        store_dir: store_path,
    };

    let installed_count = InstallerEngine::install(&config)?;
    assert_eq!(
        installed_count, 1,
        "Expected exactly 1 package to be installed offline"
    );

    // Verify package was correctly unpacked into node_modules layout
    let expected_installed_path = workspace_path
        .join("node_modules")
        .join(pkg_name)
        .join("package.json");
    assert!(
        expected_installed_path.exists(),
        "Materialized dependency file should exist in node_modules"
    );

    Ok(())
}

#[test]
fn test_airgap_offline_install_nested_npm_package_flattening(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp_dir = tempdir()?;
    let workspace_path = tmp_dir.path().join("workspace");
    let store_path = tmp_dir.path().join(".dmz/store");

    fs::create_dir_all(&workspace_path)?;
    fs::create_dir_all(&store_path)?;

    let pkg_name = "@scope/my-lib";
    let pkg_version = "1.0.0";
    let safe_name = pkg_name.replace('/', "__");

    let pkg_store_dir = store_path.join(&safe_name);
    fs::create_dir_all(&pkg_store_dir)?;

    // Standard npm registry tarballs nest everything under a "package/" folder
    let mut tar_builder = tar::Builder::new(Vec::new());
    let dummy_content = b"{\"name\": \"@scope/my-lib\"}";
    let mut header = tar::Header::new_gnu();
    header.set_size(dummy_content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();

    tar_builder.append_data(&mut header, "package/package.json", &dummy_content[..])?;
    let tar_bytes = tar_builder.into_inner()?;

    let mut gz_encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut gz_encoder, &tar_bytes)?;
    let payload = gz_encoder.finish()?;

    let archive_path = pkg_store_dir.join(format!("{}-{}.archive", safe_name, pkg_version));
    fs::write(&archive_path, &payload)?;

    let computed_sha256 = dmz_core::archive::format_sha256_hex(&Sha256::digest(&payload));

    let locked_pkg = LockedPackage {
        name: pkg_name.to_string(),
        version: pkg_version.to_string(),
        ecosystem: Ecosystem::Npm,
        sha256: computed_sha256,
        source_url: None,
        dependencies: vec![],
        member_id: None,
    };

    let mut packages = BTreeMap::new();
    packages.insert(format!("{}:{}", pkg_name, pkg_version), locked_pkg);

    let closure_sig = DmzLockfile::compute_closure_hash(&packages);
    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        vcs: None,
        workspaces: BTreeMap::new(),
        closure_signature: closure_sig,
        packages,
    };

    let lockfile_path = workspace_path.join("dmz.lock");
    lockfile.save(&lockfile_path)?;

    let config = InstallConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path,
        store_dir: store_path,
    };

    let installed_count = InstallerEngine::install(&config)?;
    assert_eq!(installed_count, 1);

    // Verify package prefix is stripped and installed properly into node_modules/@scope/my-lib/package.json
    let expected_installed_path = workspace_path
        .join("node_modules")
        .join("@scope")
        .join("my-lib")
        .join("package.json");
    assert!(
        expected_installed_path.exists(),
        "Stripped package directory should land directly at node_modules/@scope/my-lib/package.json"
    );

    // Ensure it didn't create node_modules/@scope/my-lib/package/package.json
    let unstripped_path = workspace_path
        .join("node_modules")
        .join("@scope")
        .join("my-lib")
        .join("package")
        .join("package.json");
    assert!(
        !unstripped_path.exists(),
        "Did not expect unstripped 'package/' folder inside destination"
    );

    Ok(())
}

#[test]
fn test_airgap_offline_install_npm_bin_links() -> Result<(), Box<dyn std::error::Error>> {
    let tmp_dir = tempdir()?;
    let workspace_path = tmp_dir.path().join("workspace");
    let store_path = tmp_dir.path().join(".dmz/store");

    fs::create_dir_all(&workspace_path)?;
    fs::create_dir_all(&store_path)?;

    let pkg_name = "cli-tool";
    let pkg_version = "2.0.0";
    let safe_name = pkg_name;

    let pkg_store_dir = store_path.join(safe_name);
    fs::create_dir_all(&pkg_store_dir)?;

    let mut tar_builder = tar::Builder::new(Vec::new());

    // Write package.json declaring bin
    let pkg_json = br#"{"name": "cli-tool", "bin": {"my-cli": "./bin/run.js"}}"#;
    let mut h1 = tar::Header::new_gnu();
    h1.set_size(pkg_json.len() as u64);
    h1.set_mode(0o644);
    h1.set_cksum();
    tar_builder.append_data(&mut h1, "package/package.json", &pkg_json[..])?;

    // Write executable script
    let run_js = b"#!/usr/bin/env node\nconsole.log('running');";
    let mut h2 = tar::Header::new_gnu();
    h2.set_size(run_js.len() as u64);
    h2.set_mode(0o755);
    h2.set_cksum();
    tar_builder.append_data(&mut h2, "package/bin/run.js", &run_js[..])?;

    let tar_bytes = tar_builder.into_inner()?;
    let mut gz_encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut gz_encoder, &tar_bytes)?;
    let payload = gz_encoder.finish()?;

    let archive_path = pkg_store_dir.join(format!("{}-{}.archive", safe_name, pkg_version));
    fs::write(&archive_path, &payload)?;

    let computed_sha256 = dmz_core::archive::format_sha256_hex(&Sha256::digest(&payload));

    let locked_pkg = LockedPackage {
        name: pkg_name.to_string(),
        version: pkg_version.to_string(),
        ecosystem: Ecosystem::Npm,
        sha256: computed_sha256,
        source_url: None,
        dependencies: vec![],
        member_id: None,
    };

    let mut packages = BTreeMap::new();
    packages.insert(format!("{}:{}", pkg_name, pkg_version), locked_pkg);

    let closure_sig = DmzLockfile::compute_closure_hash(&packages);
    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        vcs: None,
        workspaces: BTreeMap::new(),
        closure_signature: closure_sig,
        packages,
    };

    let lockfile_path = workspace_path.join("dmz.lock");
    lockfile.save(&lockfile_path)?;

    let config = InstallConfig {
        workspace_path: workspace_path.clone(),
        lockfile_path,
        store_dir: store_path,
    };

    let installed_count = InstallerEngine::install(&config)?;
    assert_eq!(installed_count, 1);

    // Verify node_modules/.bin symlink/wrapper exists
    let bin_dir = workspace_path.join("node_modules").join(".bin");
    assert!(bin_dir.exists(), "node_modules/.bin must exist");

    #[cfg(unix)]
    {
        let symlink_path = bin_dir.join("my-cli");
        assert!(
            symlink_path.exists(),
            "POSIX symlink for my-cli must exist in node_modules/.bin"
        );
    }

    #[cfg(windows)]
    {
        let cmd_path = bin_dir.join("my-cli.cmd");
        assert!(
            cmd_path.exists(),
            "Windows .cmd wrapper for my-cli must exist in node_modules/.bin"
        );
    }

    Ok(())
}
