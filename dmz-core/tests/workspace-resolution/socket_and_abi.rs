use dmz_core::workspace::{
    analyze_sidecar_abi, cleanup_stale_socket, safe_socket_path,
};
use std::fs;
use tempfile::tempdir;

#[test]
fn test_safe_socket_path_and_stale_cleanup() {
    let temp_dir = tempdir().unwrap();
    let very_long_path = temp_dir
        .path()
        .join("extremely_long_directory_name_that_could_break_traditional_posix_sun_path_buffers_on_darwin_and_bsd_systems");
    fs::create_dir_all(&very_long_path).unwrap();

    let sock_path = safe_socket_path(&very_long_path, "sidecar");
    let sock_str = sock_path.to_string_lossy();

    #[cfg(unix)]
    {
        // POSIX sockaddr_un.sun_path hard limits: 104 on macOS, 108 on Linux
        assert!(
            sock_str.len() < 104,
            "Safe socket path exceeded 104 characters: {}",
            sock_str
        );
        assert!(sock_str.contains("dmz_"));

        // Test stale socket cleanup
        fs::write(&sock_path, b"stale socket mock").unwrap();
        assert!(sock_path.exists());

        let removed = cleanup_stale_socket(&sock_path).unwrap();
        assert!(removed);
        assert!(!sock_path.exists());

        // Clean up when not exists returns false without error
        let removed_again = cleanup_stale_socket(&sock_path).unwrap();
        assert!(!removed_again);
    }

    #[cfg(windows)]
    {
        assert!(sock_str.starts_with(r"\\.\pipe\dmz_"));

        // Windows named pipes are managed by NPFS in kernel memory and do not create disk files.
        // Verify cleanup_stale_socket safely returns Ok(false) for named pipes without error.
        let removed_pipe = cleanup_stale_socket(&sock_path).unwrap();
        assert!(!removed_pipe);

        // Verify cleanup_stale_socket removes physical stale files on Windows
        let temp = tempdir().unwrap();
        let mock_sock_file = temp.path().join("dmz_mock.sock");
        fs::write(&mock_sock_file, b"stale socket mock").unwrap();
        assert!(mock_sock_file.exists());

        let removed = cleanup_stale_socket(&mock_sock_file).unwrap();
        assert!(removed);
        assert!(!mock_sock_file.exists());

        let removed_again = cleanup_stale_socket(&mock_sock_file).unwrap();
        assert!(!removed_again);
    }
}

#[test]
fn test_structured_abi_binary_header_diagnostics() {
    let temp = tempdir().unwrap();
    let sidecar_dir = temp.path().join("bin");
    fs::create_dir_all(&sidecar_dir).unwrap();

    // Construct a synthetic 64-bit little-endian ELF binary with 1 PT_INTERP program header
    let mut elf_dynamic = vec![0u8; 128];
    elf_dynamic[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    elf_dynamic[4] = 2; // ELFCLASS64
    elf_dynamic[5] = 1; // ELFDATA2LSB (little-endian)
    elf_dynamic[16..18].copy_from_slice(&2u16.to_le_bytes()); // ET_EXEC
    elf_dynamic[32..40].copy_from_slice(&64u64.to_le_bytes()); // e_phoff = 64
    elf_dynamic[54..56].copy_from_slice(&56u16.to_le_bytes()); // e_phentsize = 56
    elf_dynamic[56..58].copy_from_slice(&1u16.to_le_bytes()); // e_phnum = 1
    elf_dynamic[64..68].copy_from_slice(&3u32.to_le_bytes()); // p_type = PT_INTERP (3)

    let elf_path = sidecar_dir.join("dynamic_worker");
    fs::write(&elf_path, &elf_dynamic).unwrap();

    let report = analyze_sidecar_abi(&sidecar_dir);
    assert!(!report.is_static);
    assert!(!report.warnings.is_empty());

    // Replace with a static 64-bit ELF binary (only PT_LOAD = 1, no PT_INTERP or PT_DYNAMIC)
    elf_dynamic[64..68].copy_from_slice(&1u32.to_le_bytes()); // p_type = PT_LOAD (1)
    fs::write(&elf_path, &elf_dynamic).unwrap();

    let static_report = analyze_sidecar_abi(&sidecar_dir);
    assert!(static_report.is_static);
    assert!(static_report.warnings.is_empty());
}

