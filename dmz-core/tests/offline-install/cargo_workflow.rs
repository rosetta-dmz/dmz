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
fn test_airgap_offline_install_cargo_workflow() -> Result<(), Box<dyn std::error::Error>> {
    let tmp_dir = tempdir()?;
    let workspace_path = tmp_dir.path().join("workspace");
    let store_path = tmp_dir.path().join(".dmz/store");

    fs::create_dir_all(&workspace_path)?;
    fs::create_dir_all(&store_path)?;

    let pkg_name = "serde";
    let pkg_version = "1.0.197";
    let safe_name = pkg_name;

    let pkg_store_dir = store_path.join(safe_name);
    fs::create_dir_all(&pkg_store_dir)?;

    let mut tar_builder = tar::Builder::new(Vec::new());

    // Write crate Cargo.toml
    let toml_data = br#"[package]
name = "serde"
version = "1.0.197"
edition = "2021"
"#;
    let mut h1 = tar::Header::new_gnu();
    h1.set_size(toml_data.len() as u64);
    h1.set_mode(0o644);
    h1.set_cksum();
    tar_builder.append_data(&mut h1, "Cargo.toml", &toml_data[..])?;

    // Write src/lib.rs
    let lib_rs = b"pub fn serialize() -> bool { true }";
    let mut h2 = tar::Header::new_gnu();
    h2.set_size(lib_rs.len() as u64);
    h2.set_mode(0o644);
    h2.set_cksum();
    tar_builder.append_data(&mut h2, "src/lib.rs", &lib_rs[..])?;

    let tar_bytes = tar_builder.into_inner()?;
    let mut gz_encoder = GzEncoder::new(Vec::new(), Compression::default());
    std::io::Write::write_all(&mut gz_encoder, &tar_bytes)?;
    let payload = gz_encoder.finish()?;

    let archive_path = pkg_store_dir.join(format!("{}-{}.archive", safe_name, pkg_version));
    fs::write(&archive_path, &payload)?;

    let mut hasher = Sha256::new();
    hasher.update(&payload);
    let computed_sha256 = format!("{:x}", hasher.finalize());

    let locked_pkg = LockedPackage {
        name: pkg_name.to_string(),
        version: pkg_version.to_string(),
        ecosystem: Ecosystem::Cargo,
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

    // Verify .cargo/config.toml was generated to point to vendored-sources
    let cargo_config = workspace_path.join(".cargo").join("config.toml");
    assert!(
        cargo_config.exists(),
        ".cargo/config.toml must be generated"
    );
    let content = fs::read_to_string(&cargo_config)?;
    assert!(
        content.contains("replace-with = \"vendored-sources\""),
        "cargo config must contain replacement source"
    );

    // Verify package unpacked into .cargo/vendor/serde-1.0.197/
    let vendor_dir = workspace_path
        .join(".cargo")
        .join("vendor")
        .join("serde-1.0.197");
    assert!(
        vendor_dir.exists(),
        ".cargo/vendor/serde-1.0.197 directory must exist"
    );
    assert!(
        vendor_dir.join(".cargo-checksum.json").exists(),
        ".cargo-checksum.json must be materialized for vendored crate"
    );

    Ok(())
}
