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
    let mut hasher = Sha256::new();
    hasher.update(&payload);
    let computed_sha256 = format!("{:x}", hasher.finalize());

    let locked_pkg = LockedPackage {
        name: pkg_name.to_string(),
        version: pkg_version.to_string(),
        ecosystem: Ecosystem::Npm,
        sha256: computed_sha256,
        source_url: None,
        dependencies: vec![],
    };

    let mut packages = BTreeMap::new();
    packages.insert(format!("{}:{}", pkg_name, pkg_version), locked_pkg);

    let closure_sig = DmzLockfile::compute_closure_hash(&packages);
    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        vcs: None,
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
fn test_airgap_offline_install_cargo_workflow() -> Result<(), Box<dyn std::error::Error>> {
    let tmp_dir = tempdir()?;
    let workspace_path = tmp_dir.path().join("workspace");
    let store_path = tmp_dir.path().join(".dmz/store");

    fs::create_dir_all(&workspace_path)?;
    fs::create_dir_all(&store_path)?;

    let pkg_name = "serde";
    let pkg_version = "1.0.104";
    let safe_name = pkg_name.replace('/', "__");

    let pkg_store_dir = store_path.join(&safe_name);
    fs::create_dir_all(&pkg_store_dir)?;

    // Construct a valid minimal gzipped tar archive (.crate) with Cargo.toml and src/lib.rs inside
    let mut tar_builder = tar::Builder::new(Vec::new());
    let cargo_toml_content =
        b"[package]\nname = \"serde\"\nversion = \"1.0.104\"\nedition = \"2021\"\n";
    let mut header = tar::Header::new_gnu();
    header.set_size(cargo_toml_content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar_builder.append_data(&mut header, "Cargo.toml", &cargo_toml_content[..])?;

    let lib_rs_content = b"pub fn dummy() {}\n";
    let mut lib_header = tar::Header::new_gnu();
    lib_header.set_size(lib_rs_content.len() as u64);
    lib_header.set_mode(0o644);
    lib_header.set_cksum();
    tar_builder.append_data(&mut lib_header, "src/lib.rs", &lib_rs_content[..])?;

    let tar_bytes = tar_builder.into_inner()?;

    let mut gz_encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut gz_encoder, &tar_bytes)?;
    let payload = gz_encoder.finish()?;

    // Save archive to store
    let archive_filename = format!("{}-{}.archive", safe_name, pkg_version);
    let archive_path = pkg_store_dir.join(&archive_filename);
    fs::write(&archive_path, &payload)?;

    let mut hasher = Sha256::new();
    hasher.update(&payload);
    let computed_sha256 = format!("{:x}", hasher.finalize());

    // Construct the TOML lockfile matching RustVendorMaterializer expectations
    let lockfile_content = format!(
        r#"[[crates]]
name = "{}"
version = "{}"
checksum = "{}"
store_path = "{}/{}"
"#,
        pkg_name, pkg_version, computed_sha256, safe_name, archive_filename
    );
    let lockfile_path = workspace_path.join("dmz.lock");
    fs::write(&lockfile_path, lockfile_content)?;

    // Create Cargo.toml and src/main.rs in the workspace to trigger the Rust ecosystem handler detection
    fs::create_dir_all(workspace_path.join("src"))?;
    fs::write(workspace_path.join("src/main.rs"), "fn main() {}")?;
    fs::write(
        workspace_path.join("Cargo.toml"),
        "[package]\nname = \"test-app\"\nversion = \"1.0.0\"\nedition = \"2021\"\n\n[dependencies]\nserde = \"1.0.104\"\n",
    )?;

    // Execute DMZ multi-ecosystem workspace installation via the registry trait engine
    let registry = dmz_core::ecosystem::EcosystemRegistry::new();
    registry.install_workspace(&workspace_path, &store_path, &lockfile_path)?;

    // Verify cargo vendor artifacts & directory source config manifest
    let vendor_cargo_toml = workspace_path
        .join(".cargo")
        .join("vendor")
        .join(format!("{}-{}", pkg_name, pkg_version))
        .join("Cargo.toml");
    assert!(
        vendor_cargo_toml.exists(),
        "Cargo.toml should exist in .cargo/vendor"
    );

    let config_json_path = workspace_path
        .join(".cargo")
        .join("vendor")
        .join("config.json");
    assert!(
        config_json_path.exists(),
        "Cargo directory source config.json manifest must be created"
    );

    let cargo_config = workspace_path.join(".cargo").join("config.toml");
    assert!(cargo_config.exists(), ".cargo/config.toml should exist");
    let config_content = fs::read_to_string(&cargo_config)?;
    assert!(config_content.contains("vendored-sources"));

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

    let pkg_name = "express";
    let pkg_version = "4.18.2";
    let safe_name = pkg_name.replace('/', "__");

    let pkg_store_dir = store_path.join(&safe_name);
    fs::create_dir_all(&pkg_store_dir)?;

    // Construct an npm tarball with standard nested `package/` folder
    let mut tar_builder = tar::Builder::new(Vec::new());
    let pkg_json_content = b"{\"name\": \"express\", \"version\": \"4.18.2\"}";
    let mut header = tar::Header::new_gnu();
    header.set_size(pkg_json_content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar_builder.append_data(&mut header, "package/package.json", &pkg_json_content[..])?;
    let tar_bytes = tar_builder.into_inner()?;

    let mut gz_encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut gz_encoder, &tar_bytes)?;
    let payload = gz_encoder.finish()?;

    let archive_filename = format!("{}-{}.archive", safe_name, pkg_version);
    let archive_path = pkg_store_dir.join(&archive_filename);
    fs::write(&archive_path, &payload)?;

    let mut hasher = Sha256::new();
    hasher.update(&payload);
    let computed_sha256 = format!("{:x}", hasher.finalize());

    let locked_pkg = LockedPackage {
        name: pkg_name.to_string(),
        version: pkg_version.to_string(),
        ecosystem: Ecosystem::Npm,
        sha256: computed_sha256,
        source_url: None,
        dependencies: vec![],
    };

    let mut packages = BTreeMap::new();
    packages.insert(format!("{}:{}", pkg_name, pkg_version), locked_pkg);

    let closure_sig = DmzLockfile::compute_closure_hash(&packages);
    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        vcs: None,
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

    // Verify package was correctly flattened directly into node_modules/express/package.json
    let direct_pkg_json = workspace_path
        .join("node_modules")
        .join(pkg_name)
        .join("package.json");
    assert!(direct_pkg_json.exists(), "Nested package/package.json must be flattened directly into node_modules/express/package.json");

    Ok(())
}

#[test]
fn test_airgap_offline_install_python_workflow() -> Result<(), Box<dyn std::error::Error>> {
    let tmp_dir = tempdir()?;
    let workspace_path = tmp_dir.path().join("workspace");
    let store_path = tmp_dir.path().join(".dmz/store");

    fs::create_dir_all(&workspace_path)?;
    fs::create_dir_all(&store_path)?;

    let pkg_name = "requests";
    let pkg_version = "2.31.0";
    let safe_name = pkg_name.replace('/', "__");

    let pkg_store_dir = store_path.join(&safe_name);
    fs::create_dir_all(&pkg_store_dir)?;

    let mut tar_builder = tar::Builder::new(Vec::new());
    let dummy_py_content = b"# requests module";
    let mut header = tar::Header::new_gnu();
    header.set_size(dummy_py_content.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar_builder.append_data(&mut header, "requests/__init__.py", &dummy_py_content[..])?;
    let tar_bytes = tar_builder.into_inner()?;

    let mut gz_encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut gz_encoder, &tar_bytes)?;
    let payload = gz_encoder.finish()?;

    let archive_filename = format!("{}-{}.archive", safe_name, pkg_version);
    let archive_path = pkg_store_dir.join(&archive_filename);
    fs::write(&archive_path, &payload)?;

    let mut hasher = Sha256::new();
    hasher.update(&payload);
    let computed_sha256 = format!("{:x}", hasher.finalize());

    let locked_pkg = LockedPackage {
        name: pkg_name.to_string(),
        version: pkg_version.to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: computed_sha256,
        source_url: None,
        dependencies: vec![],
    };

    let mut packages = BTreeMap::new();
    packages.insert(format!("{}:{}", pkg_name, pkg_version), locked_pkg);

    let closure_sig = DmzLockfile::compute_closure_hash(&packages);
    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        vcs: None,
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

    let site_packages = workspace_path
        .join(".dmz")
        .join("venv")
        .join("lib")
        .join("python3.11")
        .join("site-packages")
        .join("requests")
        .join("__init__.py");
    assert!(
        site_packages.exists(),
        "requests/__init__.py should exist in python site-packages"
    );

    // Verify .pth bridge was written into workspace .venv
    let pth_bridge = workspace_path
        .join(".venv")
        .join("lib")
        .join("python3.11")
        .join("site-packages")
        .join("dmz_store.pth");
    assert!(
        pth_bridge.exists(),
        "dmz_store.pth bridge should exist in .venv"
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

    let pkg_name = "eslint";
    let pkg_version = "8.0.0";
    let safe_name = pkg_name.replace('/', "__");

    let pkg_store_dir = store_path.join(&safe_name);
    fs::create_dir_all(&pkg_store_dir)?;

    let mut tar_builder = tar::Builder::new(Vec::new());
    let pkg_json = br#"{"name":"eslint","version":"8.0.0","bin":{"eslint":"bin/eslint.js"}}"#;
    let mut header = tar::Header::new_gnu();
    header.set_size(pkg_json.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar_builder.append_data(&mut header, "package.json", &pkg_json[..])?;

    let bin_script = b"#!/usr/bin/env node\nconsole.log('eslint');\n";
    let mut bin_header = tar::Header::new_gnu();
    bin_header.set_size(bin_script.len() as u64);
    bin_header.set_mode(0o755);
    bin_header.set_cksum();
    tar_builder.append_data(&mut bin_header, "bin/eslint.js", &bin_script[..])?;

    let tar_bytes = tar_builder.into_inner()?;
    let mut gz_encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut gz_encoder, &tar_bytes)?;
    let payload = gz_encoder.finish()?;

    let archive_filename = format!("{}-{}.archive", safe_name, pkg_version);
    let archive_path = pkg_store_dir.join(&archive_filename);
    fs::write(&archive_path, &payload)?;

    let mut hasher = Sha256::new();
    hasher.update(&payload);
    let computed_sha256 = format!("{:x}", hasher.finalize());

    let locked_pkg = LockedPackage {
        name: pkg_name.to_string(),
        version: pkg_version.to_string(),
        ecosystem: Ecosystem::Npm,
        sha256: computed_sha256,
        source_url: None,
        dependencies: vec![],
    };

    let mut packages = BTreeMap::new();
    packages.insert(format!("{}:{}", pkg_name, pkg_version), locked_pkg);

    let closure_sig = DmzLockfile::compute_closure_hash(&packages);
    let lockfile = DmzLockfile {
        version: 1,
        target_platform: None,
        vcs: None,
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

    let bin_link = if cfg!(windows) {
        workspace_path
            .join("node_modules")
            .join(".bin")
            .join("eslint.cmd")
    } else {
        workspace_path
            .join("node_modules")
            .join(".bin")
            .join("eslint")
    };
    assert!(
        bin_link.exists(),
        "eslint executable should exist in node_modules/.bin"
    );

    Ok(())
}
