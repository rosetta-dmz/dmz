use dmz_core::archive::{
    AirGapExporter, AirGapImporter, AirGapManifest, ExportConfig, ExportMode, ImportConfig,
};
use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::toolchain::{ToolchainEcosystem, ToolchainManifestEntry, ToolchainProfile};
use dmz_core::workspace::WorkspaceProjectConfig;
use std::fs;
use std::io::Write;
use std::path::Path;
use tar::{Archive, Builder};
use tempfile::tempdir;
use zstd::stream::read::Decoder;
use zstd::stream::write::Encoder;

fn write_tar_bytes<W: Write>(
    builder: &mut Builder<W>,
    rel_path: &str,
    data: &[u8],
) -> Result<(), Box<dyn std::error::Error>> {
    let mut header = tar::Header::new_gnu();
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    builder.append_data(&mut header, rel_path, data)?;
    Ok(())
}

fn list_archive_entries(archive_path: &Path) -> Result<Vec<String>, Box<dyn std::error::Error>> {
    let file = fs::File::open(archive_path)?;
    let decoder = Decoder::new(file)?;
    let mut tar_archive = Archive::new(decoder);
    let mut entries = Vec::new();
    for entry_res in tar_archive.entries()? {
        let entry = entry_res?;
        entries.push(entry.path()?.to_string_lossy().into_owned());
    }
    Ok(entries)
}

#[test]
fn test_export_prunes_unreferenced_store_versions_metadata_and_stale_dirs(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let ws = tmp.path().join("ws");
    let store = tmp.path().join("store");
    let out = tmp.path().join("dist/bundle.tar.zst");

    fs::create_dir_all(&ws)?;
    fs::create_dir_all(store.join("requests"))?;
    fs::create_dir_all(store.join("stale-pkg"))?;
    fs::write(ws.join("requirements.txt"), "requests==2.31.0\n")?;

    let valid_payload = b"requests-2.31.0-wheel-bytes";
    let valid_sha = DmzLockfile::hash_bytes(valid_payload);
    fs::write(
        store.join("requests/requests-2.31.0.archive"),
        valid_payload,
    )?;
    // Unreferenced extra files & directories that must NOT be bundled
    fs::write(
        store.join("requests/requests-2.30.0.archive"),
        b"old-version",
    )?;
    fs::write(store.join("requests/metadata.json"), b"{}")?;
    fs::write(store.join("requests/extra.whl"), b"stray-file")?;
    fs::write(
        store.join("stale-pkg/stale-pkg-1.0.0.archive"),
        b"stale-package",
    )?;

    let lockfile = DmzLockfile::new(vec![LockedPackage {
        name: "requests".to_string(),
        version: "2.31.0".to_string(),
        ecosystem: Ecosystem::PyPI,
        sha256: valid_sha,
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    }]);
    let lock_path = ws.join("dmz.lock");
    lockfile.save(&lock_path)?;

    let cfg = ExportConfig {
        workspace_path: ws,
        lockfile_path: lock_path,
        store_dir: store,
        output_path: out.clone(),
        zstd_level: 1,
        mode: ExportMode::Unified,
        target_platform: None,
        with_git_history: false,
        since: None,
        with_toolchain: false,
        multi_workspace: None,
        include_dev: false,
        toolchain_profile: ToolchainProfile::Minimal,
        allow_missing_toolchains: false,
    };

    AirGapExporter::export(&cfg)?;

    let entries = list_archive_entries(&out)?;
    let store_entries: Vec<&str> = entries
        .iter()
        .map(String::as_str)
        .filter(|e| e.starts_with("store/"))
        .collect();

    assert_eq!(
        store_entries,
        vec!["store/requests/requests-2.31.0.archive"]
    );
    assert!(!entries
        .iter()
        .any(|e| e.contains("requests-2.30.0.archive")));
    assert!(!entries.iter().any(|e| e.contains("metadata.json")));
    assert!(!entries.iter().any(|e| e.contains("stale-pkg")));

    Ok(())
}

#[test]
fn test_import_prunes_unreferenced_store_files_and_empty_dirs(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let bundle_path = tmp.path().join("bloated.tar.zst");
    let target_ws = tmp.path().join("imported_ws");
    let target_store = tmp.path().join("imported_store");

    let valid_payload = b"lodash-4.17.21-tgz";
    let valid_sha = DmzLockfile::hash_bytes(valid_payload);
    let lockfile = DmzLockfile::new(vec![LockedPackage {
        name: "lodash".to_string(),
        version: "4.17.21".to_string(),
        ecosystem: Ecosystem::Npm,
        sha256: valid_sha,
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    }]);
    let lock_json = serde_json::to_vec_pretty(&lockfile)?;

    let manifest = AirGapManifest {
        dmz_version: "1.4.5".to_string(),
        target_platform: None,
        vcs: None,
        delta_base: None,
        has_git_history: false,
        toolchains: Vec::new(),
        workspace_project: None,
        closure_signature: lockfile.closure_signature.clone(),
        package_count: 1,
        created_at_utc: 1_700_000_000,
        archive_sha256: String::new(),
    };
    let manifest_json = serde_json::to_vec_pretty(&manifest)?;

    {
        let out_file = fs::File::create(&bundle_path)?;
        let zstd_enc = Encoder::new(out_file, 1)?;
        let mut tar_builder = Builder::new(zstd_enc);

        write_tar_bytes(&mut tar_builder, "airgap_manifest.json", &manifest_json)?;
        write_tar_bytes(&mut tar_builder, "dmz.lock", &lock_json)?;
        write_tar_bytes(
            &mut tar_builder,
            "workspace/package.json",
            br#"{"name":"app","dependencies":{"lodash":"4.17.21"}}"#,
        )?;
        write_tar_bytes(
            &mut tar_builder,
            "store/lodash/lodash-4.17.21.archive",
            valid_payload,
        )?;
        // Unreferenced artifacts injected into store/
        write_tar_bytes(
            &mut tar_builder,
            "store/lodash/lodash-4.17.20.archive",
            b"stale-lodash-version",
        )?;
        write_tar_bytes(
            &mut tar_builder,
            "store/unreferenced-pkg/unreferenced-pkg-9.9.9.archive",
            b"rogue-pkg",
        )?;

        let zstd_enc = tar_builder.into_inner()?;
        zstd_enc.finish()?;
    }

    let import_cfg = ImportConfig {
        archive_path: bundle_path,
        target_store_dir: target_store.clone(),
        target_workspace_dir: target_ws,
        verify_closure_hash: false,
        include_dev: false,
        ..Default::default()
    };

    let result = AirGapImporter::import(&import_cfg)?;
    assert_eq!(result.imported_packages, 1);
    assert!(target_store.join("lodash/lodash-4.17.21.archive").is_file());
    assert!(!target_store.join("lodash/lodash-4.17.20.archive").exists());
    assert!(!target_store.join("unreferenced-pkg").exists());

    Ok(())
}

#[test]
fn test_import_root_manifest_dominance_purges_stray_python_toolchain_on_pure_rust_workspace(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let bundle_path = tmp.path().join("stray_toolchain.tar.zst");
    let target_ws = tmp.path().join("rust_ws");
    let target_store = tmp.path().join("rust_store");

    let crate_payload = b"serde-1.0.200-crate";
    let crate_sha = DmzLockfile::hash_bytes(crate_payload);
    let lockfile = DmzLockfile::new(vec![LockedPackage {
        name: "serde".to_string(),
        version: "1.0.200".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: crate_sha,
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    }]);
    let lock_json = serde_json::to_vec_pretty(&lockfile)?;

    // Manifest claims a Python toolchain even though the workspace is pure Rust!
    let manifest = AirGapManifest {
        dmz_version: "1.4.5".to_string(),
        target_platform: None,
        vcs: None,
        delta_base: None,
        has_git_history: false,
        toolchains: vec![ToolchainManifestEntry {
            ecosystem: ToolchainEcosystem::Python,
            version: "Python 3.12.0".to_string(),
            path_in_archive: "toolchains/python".to_string(),
            binaries: vec!["bin/python3".to_string()],
        }],
        workspace_project: None,
        closure_signature: lockfile.closure_signature.clone(),
        package_count: 1,
        created_at_utc: 1_700_000_000,
        archive_sha256: String::new(),
    };
    let manifest_json = serde_json::to_vec_pretty(&manifest)?;

    {
        let out_file = fs::File::create(&bundle_path)?;
        let zstd_enc = Encoder::new(out_file, 1)?;
        let mut tar_builder = Builder::new(zstd_enc);

        write_tar_bytes(&mut tar_builder, "airgap_manifest.json", &manifest_json)?;
        write_tar_bytes(&mut tar_builder, "dmz.lock", &lock_json)?;
        write_tar_bytes(
            &mut tar_builder,
            "workspace/Cargo.toml",
            br#"[package]
name = "pure-rust"
version = "0.1.0"
edition = "2021"
[dependencies]
serde = "1.0"
"#,
        )?;
        write_tar_bytes(&mut tar_builder, "workspace/src/lib.rs", b"")?;
        write_tar_bytes(
            &mut tar_builder,
            "store/serde/serde-1.0.200.archive",
            crate_payload,
        )?;
        write_tar_bytes(
            &mut tar_builder,
            "toolchains/python/bin/python3",
            b"#!/bin/sh\necho stray python\n",
        )?;

        let zstd_enc = tar_builder.into_inner()?;
        zstd_enc.finish()?;
    }

    let import_cfg = ImportConfig {
        archive_path: bundle_path,
        target_store_dir: target_store,
        target_workspace_dir: target_ws.clone(),
        verify_closure_hash: false,
        include_dev: false,
        ..Default::default()
    };

    let result = AirGapImporter::import(&import_cfg)?;
    assert!(!target_ws.join(".dmz/toolchains/python").exists());
    assert!(!result.has_toolchains);

    Ok(())
}

#[test]
fn test_export_scoped_manifest_ignores_unrelated_ancestor_dmz_workspace_toml(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = tempdir()?;
    let ancestor = tmp.path();
    let child_ws = ancestor.join("isolated-rust-service");
    let store = tmp.path().join("store");
    let out = tmp.path().join("dist/scoped.tar.zst");

    // Ancestor holds an unrelated dmz-workspace.toml referencing Python
    fs::write(
        ancestor.join("dmz-workspace.toml"),
        r#"
name = "unrelated-umbrella"
[[members]]
id = "unrelated-py"
path = "py-svc"
role = "sidecar"
ecosystem = "python"
"#,
    )?;

    fs::create_dir_all(child_ws.join("src"))?;
    fs::create_dir_all(store.join("anyhow"))?;
    fs::write(
        child_ws.join("Cargo.toml"),
        "[package]\nname = \"isolated-rust\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(child_ws.join("src/lib.rs"), "")?;

    let crate_payload = b"anyhow-1.0.86-crate";
    let crate_sha = DmzLockfile::hash_bytes(crate_payload);
    fs::write(store.join("anyhow/anyhow-1.0.86.archive"), crate_payload)?;

    let lockfile = DmzLockfile::new(vec![LockedPackage {
        name: "anyhow".to_string(),
        version: "1.0.86".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: crate_sha,
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    }]);
    let lock_path = child_ws.join("dmz.lock");
    lockfile.save(&lock_path)?;

    // Scoped discovery inside child_ws must NOT walk up to ancestor dmz-workspace.toml
    assert!(
        WorkspaceProjectConfig::discover_or_from_lockfile_scoped(&child_ws, &lock_path).is_none()
    );

    let cfg = ExportConfig {
        workspace_path: child_ws,
        lockfile_path: lock_path,
        store_dir: store,
        output_path: out.clone(),
        zstd_level: 1,
        mode: ExportMode::Unified,
        target_platform: None,
        with_git_history: false,
        since: None,
        with_toolchain: false,
        multi_workspace: None,
        include_dev: false,
        toolchain_profile: ToolchainProfile::Minimal,
        allow_missing_toolchains: false,
    };
    let manifest = AirGapExporter::export(&cfg)?;
    assert!(manifest.workspace_project.is_none());

    Ok(())
}
