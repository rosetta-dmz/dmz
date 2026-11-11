use dmz_core::archive::exporter::{AirGapExporter, ExportConfig, ExportMode};
use dmz_core::resolver::lockfile::DmzLockfile;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_deterministic_archive_reproducibility() -> anyhow::Result<()> {
    std::env::set_var("SOURCE_DATE_EPOCH", "1700000000");

    let temp_ws = tempdir()?;
    let ws_path = temp_ws.path().join("workspace");
    fs::create_dir_all(&ws_path)?;

    // Create several files in varying order
    fs::write(
        ws_path.join("Cargo.toml"),
        "[package]\nname = \"deterministic-demo\"\nversion = \"1.0.0\"\n",
    )?;
    fs::create_dir_all(ws_path.join("src"))?;
    fs::write(
        ws_path.join("src/lib.rs"),
        "pub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )?;
    fs::write(
        ws_path.join("package.json"),
        "{\n  \"name\": \"deterministic-demo\",\n  \"version\": \"1.0.0\"\n}\n",
    )?;
    fs::write(ws_path.join("README.md"), "# Deterministic Archive\n")?;

    let lockfile = DmzLockfile::new(Vec::new());
    let lockfile_path = ws_path.join("dmz.lock");
    lockfile.save(&lockfile_path)?;

    let dist_dir1 = tempdir()?;
    let archive_path1 = dist_dir1.path().join("bundle_run1.tar.zst");
    let export_cfg1 = ExportConfig {
        workspace_path: ws_path.clone(),
        lockfile_path: lockfile_path.clone(),
        store_dir: ws_path.join(".dmz/store"),
        output_path: archive_path1.clone(),
        zstd_level: 3,
        mode: ExportMode::CodeOnly,
        target_platform: None,
        with_git_history: false,
        since: None,
        with_toolchain: false,
        multi_workspace: None,
    };

    let manifest1 = AirGapExporter::export(&export_cfg1)?;
    assert!(archive_path1.exists());
    let sha1 = manifest1.archive_sha256;
    let bytes1 = fs::read(&archive_path1)?;

    // Simulate delay and touch files to change host mtime
    std::thread::sleep(std::time::Duration::from_millis(50));
    fs::write(ws_path.join("README.md"), "# Deterministic Archive\n")?;

    let dist_dir2 = tempdir()?;
    let archive_path2 = dist_dir2.path().join("bundle_run2.tar.zst");
    let export_cfg2 = ExportConfig {
        workspace_path: ws_path.clone(),
        lockfile_path,
        store_dir: ws_path.join(".dmz/store"),
        output_path: archive_path2.clone(),
        zstd_level: 3,
        mode: ExportMode::CodeOnly,
        target_platform: None,
        with_git_history: false,
        since: None,
        with_toolchain: false,
        multi_workspace: None,
    };

    let manifest2 = AirGapExporter::export(&export_cfg2)?;
    assert!(archive_path2.exists());
    let sha2 = manifest2.archive_sha256;
    let bytes2 = fs::read(&archive_path2)?;

    // Both SHA-256 and byte-for-byte contents must be identical!
    assert_eq!(
        sha1, sha2,
        "Deterministic build SHA256 must match across exports"
    );
    assert_eq!(
        bytes1, bytes2,
        "Archive byte content must be bit-for-bit identical"
    );

    Ok(())
}

#[test]
fn test_single_pass_streaming_hash_matches_disk_hash() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let ws_path = temp.path().join("hash_ws");
    fs::create_dir_all(&ws_path)?;
    fs::write(
        ws_path.join("file1.txt"),
        "Single-pass streaming test content 1",
    )?;
    fs::write(
        ws_path.join("file2.txt"),
        "Single-pass streaming test content 2",
    )?;

    let lockfile = DmzLockfile::new(Vec::new());
    let lockfile_path = ws_path.join("dmz.lock");
    lockfile.save(&lockfile_path)?;

    let archive_path = temp.path().join("single_pass_bundle.tar.zst");
    let export_cfg = ExportConfig {
        workspace_path: ws_path,
        lockfile_path,
        store_dir: temp.path().join("store"),
        output_path: archive_path.clone(),
        zstd_level: 3,
        mode: ExportMode::CodeOnly,
        target_platform: None,
        with_git_history: false,
        since: None,
        with_toolchain: false,
        multi_workspace: None,
    };

    let manifest = AirGapExporter::export(&export_cfg)?;
    assert!(archive_path.exists());

    // Single-pass inline computed hash must match the independently verified disk hash
    let disk_sha = AirGapExporter::calculate_file_sha256(&archive_path)?;
    assert_eq!(
        manifest.archive_sha256, disk_sha,
        "Inline streaming hash must match re-read disk hash"
    );

    Ok(())
}

#[test]
fn test_export_symlink_containment() -> anyhow::Result<()> {
    let temp = tempdir()?;
    let ws = temp.path().join("ws");
    fs::create_dir_all(ws.join("src"))?;
    fs::write(
        ws.join("Cargo.toml"),
        "[package]\nname = \"symlink-test\"\nversion = \"1.0.0\"\n",
    )?;
    fs::write(ws.join("src/lib.rs"), "pub fn hello() {}\n")?;

    #[cfg(unix)]
    {
        // Create an absolute symlink pointing outside the workspace
        let outside_dir = temp.path().join("outside_secrets");
        fs::create_dir_all(&outside_dir)?;
        let secret_file = outside_dir.join("secret.txt");
        fs::write(&secret_file, "super secret token")?;

        let external_link = ws.join("secrets_link");
        let _ = std::os::unix::fs::symlink(&secret_file, &external_link);

        // Also create a safe relative internal symlink
        let safe_link = ws.join("src/lib_alias.rs");
        let _ = std::os::unix::fs::symlink("lib.rs", &safe_link);
    }

    let lockfile = DmzLockfile::new(Vec::new());
    let lock_path = ws.join("dmz.lock");
    lockfile.save(&lock_path)?;

    let archive_path = temp.path().join("bundle.tar.zst");
    let export_cfg = ExportConfig {
        workspace_path: ws.clone(),
        lockfile_path: lock_path,
        store_dir: ws.join(".dmz/store"),
        output_path: archive_path.clone(),
        zstd_level: 3,
        mode: ExportMode::CodeOnly,
        target_platform: None,
        with_git_history: false,
        since: None,
        with_toolchain: false,
        multi_workspace: None,
    };

    let manifest = AirGapExporter::export(&export_cfg)?;
    assert!(archive_path.exists());
    assert_eq!(manifest.package_count, 0);

    Ok(())
}
