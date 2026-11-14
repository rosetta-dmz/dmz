use super::*;
use ::zip::write::SimpleFileOptions;
use ::zip::ZipWriter;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;
use tempfile::tempdir;

#[test]
fn test_is_safe_relative_target() {
    assert!(SafeArchiveExtractor::is_safe_relative_target(Path::new(
        "lib/foo"
    )));
    assert!(SafeArchiveExtractor::is_safe_relative_target(Path::new(
        "./foo"
    )));
    assert!(SafeArchiveExtractor::is_safe_relative_target(Path::new(
        "sub/../foo"
    )));
    assert!(!SafeArchiveExtractor::is_safe_relative_target(Path::new(
        "/etc/shadow"
    )));
    assert!(!SafeArchiveExtractor::is_safe_relative_target(Path::new(
        "../escape"
    )));
    assert!(!SafeArchiveExtractor::is_safe_relative_target(Path::new(
        "foo/../../escape"
    )));
}

#[test]
fn test_detect_format_zip_and_tar_gz() {
    let temp = tempdir().unwrap();
    let zip_path = temp.path().join("test.zip");
    let targz_path = temp.path().join("test.tar.gz");

    {
        let file = File::create(&zip_path).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file("hello.txt", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"world").unwrap();
        writer.finish().unwrap();
    }

    {
        let file = File::create(&targz_path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar_builder = ::tar::Builder::new(encoder);
        let mut header = ::tar::Header::new_gnu();
        header.set_size(5);
        header.set_cksum();
        tar_builder
            .append_data(&mut header, "hello.txt", &b"world"[..])
            .unwrap();
        tar_builder.into_inner().unwrap().finish().unwrap();
    }

    assert_eq!(
        SafeArchiveExtractor::detect_format(&zip_path).unwrap(),
        ArchiveFormat::Zip
    );
    assert_eq!(
        SafeArchiveExtractor::detect_format(&targz_path).unwrap(),
        ArchiveFormat::TarGz
    );
}

#[test]
fn test_extract_zip_success() {
    let temp = tempdir().unwrap();
    let zip_path = temp.path().join("test.zip");
    let extract_dir = temp.path().join("extracted");

    {
        let file = File::create(&zip_path).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file("pkg/mod.py", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"print('hello')").unwrap();
        writer.finish().unwrap();
    }

    SafeArchiveExtractor::extract(&zip_path, &extract_dir).unwrap();
    assert!(extract_dir.join("pkg/mod.py").is_file());
    assert_eq!(
        fs::read_to_string(extract_dir.join("pkg/mod.py")).unwrap(),
        "print('hello')"
    );
}

#[test]
fn test_zipslip_rejection() {
    let temp = tempdir().unwrap();
    let traversal_zip = temp.path().join("traversal.zip");
    let extract_dir = temp.path().join("extracted");
    fs::create_dir_all(&extract_dir).unwrap();

    {
        let file = File::create(&traversal_zip).unwrap();
        let mut writer = ZipWriter::new(file);
        writer
            .start_file("../outside.txt", SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"malicious").unwrap();
        writer.finish().unwrap();
    }

    let result = SafeArchiveExtractor::extract(&traversal_zip, &extract_dir);
    assert!(result.is_err(), "ZipSlip attempt must fail");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Path traversal attempt"),
        "Error must mention path traversal: {}",
        err_msg
    );
    assert!(
        !temp.path().join("outside.txt").exists(),
        "File outside target directory must not be created"
    );
}

#[test]
fn test_tar_zipslip_rejection() {
    let temp = tempdir().unwrap();
    let traversal_tar_gz = temp.path().join("traversal.tar.gz");
    let extract_dir = temp.path().join("extracted");
    fs::create_dir_all(&extract_dir).unwrap();

    {
        let file = File::create(&traversal_tar_gz).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar_builder = ::tar::Builder::new(encoder);
        let payload = b"malicious";
        let mut header = ::tar::Header::new_gnu();
        let raw_path = b"../outside_tar.txt";
        header.as_gnu_mut().unwrap().name[..raw_path.len()].copy_from_slice(raw_path);
        header.set_size(payload.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar_builder.append(&header, &payload[..]).unwrap();
        tar_builder.into_inner().unwrap().finish().unwrap();
    }

    let result = SafeArchiveExtractor::extract(&traversal_tar_gz, &extract_dir);
    assert!(result.is_err(), "Tar ZipSlip attempt must fail");
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("Path traversal attempt"),
        "Error must mention path traversal: {}",
        err_msg
    );
    assert!(
        !temp.path().join("outside_tar.txt").exists(),
        "File outside target directory must not be created"
    );
}

#[cfg(unix)]
#[test]
fn test_zip_reextract_overwrites_readonly_and_zero_mode_files() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempdir().unwrap();
    let wheel_path = temp.path().join("protobuf-5.0.0-py3-none-any.whl");
    let target_dir = temp.path().join("site-packages");

    {
        let file = File::create(&wheel_path).unwrap();
        let mut zip = ZipWriter::new(file);
        let ro_opts = SimpleFileOptions::default().unix_permissions(0o444);
        zip.start_file("google/protobuf/__init__.py", ro_opts)
            .unwrap();
        zip.write_all(b"__version__ = '5.0.0'\n").unwrap();

        let zero_opts = SimpleFileOptions::default().unix_permissions(0o000);
        zip.start_file("protobuf-5.0.0.dist-info/METADATA", zero_opts)
            .unwrap();
        zip.write_all(b"Name: protobuf\nVersion: 5.0.0\n").unwrap();
        zip.finish().unwrap();
    }

    SafeArchiveExtractor::extract(&wheel_path, &target_dir).expect("First extract must succeed");

    let init_py = target_dir.join("google/protobuf/__init__.py");
    let metadata = target_dir.join("protobuf-5.0.0.dist-info/METADATA");
    let init_mode = fs::metadata(&init_py).unwrap().permissions().mode() & 0o777;
    let meta_mode = fs::metadata(&metadata).unwrap().permissions().mode() & 0o777;
    assert_eq!(init_mode, 0o644, "Read-only ZIP entry must gain owner rw");
    assert_eq!(meta_mode, 0o644, "Zero-mode ZIP entry must default to 0644");

    fs::set_permissions(&init_py, fs::Permissions::from_mode(0o444)).unwrap();

    SafeArchiveExtractor::extract(&wheel_path, &target_dir)
        .expect("Second extract over read-only files must succeed");
    assert!(init_py.is_file());
    assert!(metadata.is_file());
}

#[cfg(unix)]
#[test]
fn test_tar_reextract_overwrites_readonly_files_and_dirs() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempdir().unwrap();
    let tar_gz_path = temp.path().join("pkg.tar.gz");
    let extract_dir = temp.path().join("extracted");

    {
        let file = File::create(&tar_gz_path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar_builder = ::tar::Builder::new(encoder);

        let mut dir_hdr = ::tar::Header::new_gnu();
        dir_hdr.set_entry_type(::tar::EntryType::Directory);
        dir_hdr.set_mode(0o555);
        dir_hdr.set_size(0);
        dir_hdr.set_cksum();
        tar_builder
            .append_data(&mut dir_hdr, "pkg_dir", std::io::empty())
            .unwrap();

        let payload = b"hello";
        let mut file_hdr = ::tar::Header::new_gnu();
        file_hdr.set_mode(0o444);
        file_hdr.set_size(payload.len() as u64);
        file_hdr.set_cksum();
        tar_builder
            .append_data(&mut file_hdr, "pkg_dir/mod.rs", &payload[..])
            .unwrap();

        tar_builder.into_inner().unwrap().finish().unwrap();
    }

    SafeArchiveExtractor::extract(&tar_gz_path, &extract_dir)
        .expect("First tar extract must succeed");
    let dir_mode = fs::metadata(extract_dir.join("pkg_dir"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode & 0o700, 0o700, "Dir must preserve owner rwx");

    SafeArchiveExtractor::extract(&tar_gz_path, &extract_dir)
        .expect("Second tar extract over readonly entries must succeed");
}

#[cfg(unix)]
#[test]
fn test_zip_intermediate_symlink_rejection() {
    let temp = tempdir().unwrap();
    let outside_dir = temp.path().join("outside");
    let extract_dir = temp.path().join("extracted");
    fs::create_dir_all(&outside_dir).unwrap();
    fs::create_dir_all(&extract_dir).unwrap();

    std::os::unix::fs::symlink(&outside_dir, extract_dir.join("escape")).unwrap();

    let zip_path = temp.path().join("sym_escape.zip");
    {
        let file = File::create(&zip_path).unwrap();
        let mut zip = ZipWriter::new(file);
        zip.start_file("escape/pwned.txt", SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"evil").unwrap();
        zip.finish().unwrap();
    }

    let err = SafeArchiveExtractor::extract(&zip_path, &extract_dir).unwrap_err();
    assert!(
        err.to_string().contains("Symlink traversal detected"),
        "Unexpected error: {}",
        err
    );
    assert!(!outside_dir.join("pwned.txt").exists());
}

#[test]
fn test_zip_symlink_entry_traversal_rejection() {
    let temp = tempdir().unwrap();
    let zip_path = temp.path().join("bad_symlink_entry.zip");
    let extract_dir = temp.path().join("extracted");

    {
        let file = File::create(&zip_path).unwrap();
        let mut zip = ZipWriter::new(file);
        zip.add_symlink(
            "evil_link",
            "../../etc/passwd",
            SimpleFileOptions::default(),
        )
        .unwrap();
        zip.finish().unwrap();
    }

    let err = SafeArchiveExtractor::extract(&zip_path, &extract_dir).unwrap_err();
    assert!(
        err.to_string()
            .contains("Path traversal attempt in archive link target"),
        "Unexpected error: {}",
        err
    );
}
