use dmz_core::install::fast_clone::FastClone;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_fast_clone_reflink_and_dir_recursive() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let src_dir = temp.path().join("source_pkg");
    let dst_dir = temp.path().join("destination_pkg");

    fs::create_dir_all(src_dir.join("sub"))?;
    fs::write(
        src_dir.join("package.json"),
        "{\"name\":\"fast-clone-test\"}",
    )?;
    fs::write(src_dir.join("sub/index.js"), "console.log('fast clone');")?;

    FastClone::clone_dir_recursive(&src_dir, &dst_dir)?;

    assert!(dst_dir.join("package.json").exists());
    assert!(dst_dir.join("sub/index.js").exists());
    assert_eq!(
        fs::read_to_string(dst_dir.join("package.json"))?,
        "{\"name\":\"fast-clone-test\"}"
    );
    assert_eq!(
        fs::read_to_string(dst_dir.join("sub/index.js"))?,
        "console.log('fast clone');"
    );

    // Verify fast-clone file overwrite safety
    let single_src = src_dir.join("single.txt");
    let single_dst = dst_dir.join("single.txt");
    fs::write(&single_src, "test single clone")?;
    FastClone::clone_file(&single_src, &single_dst)?;
    assert_eq!(fs::read_to_string(&single_dst)?, "test single clone");

    // Overwrite existing
    fs::write(&single_src, "test single clone updated")?;
    FastClone::clone_file(&single_src, &single_dst)?;
    assert_eq!(
        fs::read_to_string(&single_dst)?,
        "test single clone updated"
    );

    Ok(())
}

#[test]
fn test_archive_importer_rejects_symlink_traversal() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let ws = temp.path().join("ws");
    let store = temp.path().join("store");
    let archive_path = temp.path().join("traversal_symlink.tar.zst");

    fs::create_dir_all(&ws)?;
    fs::create_dir_all(&store)?;

    let file = fs::File::create(&archive_path)?;
    let enc = zstd::stream::Encoder::new(file, 3)?;
    let mut tar = tar::Builder::new(enc);

    let manifest = dmz_core::archive::AirGapManifest {
        dmz_version: "1.2.8".to_string(),
        target_platform: None,
        vcs: None,
        delta_base: None,
        has_git_history: false,
        toolchains: vec![],
        workspace_project: None,
        closure_signature: "mock_sig".to_string(),
        package_count: 1,
        created_at_utc: 0,
        archive_sha256: "mock_sha".to_string(),
    };
    let manifest_bytes = serde_json::to_vec(&manifest)?;
    let mut header = tar::Header::new_gnu();
    header.set_size(manifest_bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append_data(
        &mut header,
        "airgap_manifest.json",
        manifest_bytes.as_slice(),
    )?;

    // Create a symlink entry that attempts to escape target_store_dir
    let mut sym_header = tar::Header::new_gnu();
    sym_header.set_entry_type(tar::EntryType::Symlink);
    sym_header.set_size(0);
    sym_header.set_mode(0o777);
    let link_target = b"../../../../../../etc/passwd";
    sym_header.as_mut_bytes()[157..157 + link_target.len()].copy_from_slice(link_target);
    let entry_name = b"store/malicious_symlink";
    sym_header.as_mut_bytes()[..entry_name.len()].copy_from_slice(entry_name);
    sym_header.set_cksum();
    tar.append(&sym_header, std::io::empty())?;

    let enc = tar.into_inner()?;
    enc.finish()?;

    let config = dmz_core::archive::importer::ImportConfig {
        archive_path,
        target_store_dir: store,
        target_workspace_dir: ws,
        verify_closure_hash: false,
    };

    let result = dmz_core::archive::importer::AirGapImporter::import(&config);
    assert!(
        result.is_err(),
        "Importer must reject symlinks escaping extraction root"
    );
    let err_msg = result.err().unwrap().to_string();
    assert!(
        err_msg.contains("Path traversal attempt detected in link target"),
        "Expected link target rejection, got: {}",
        err_msg
    );

    Ok(())
}

#[test]
fn test_fast_clone_store_immutability() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let store_dir = temp.path().join(".dmz/store");
    fs::create_dir_all(&store_dir)?;

    let store_file = store_dir.join("immutable_lib.txt");
    fs::write(&store_file, "original content in store")?;

    let ws_dest = temp.path().join("workspace/node_modules/immutable_lib.txt");
    fs::create_dir_all(ws_dest.parent().unwrap())?;

    FastClone::clone_file(&store_file, &ws_dest)?;
    assert!(ws_dest.exists());
    assert_eq!(fs::read_to_string(&ws_dest)?, "original content in store");

    Ok(())
}
