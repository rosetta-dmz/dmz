use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage, WorkspaceMetadata};
use dmz_core::resolver::manifest::Ecosystem;
use dmz_core::toolchain::detection::ToolchainDetector;
use dmz_core::toolchain::types::ToolchainEcosystem;
use dmz_core::workspace::{
    cleanup_stale_socket, safe_socket_path, IpcBridgeConfig, MemberRole, WorkspaceMember,
    WorkspaceProjectConfig,
};
use std::fs;
use std::path::PathBuf;

#[test]
fn test_multi_workspace_config_manifest_lifecycle() {
    let temp_dir = tempfile::tempdir().unwrap();
    let manifest_path = temp_dir.path().join("dmz-workspace.toml");

    let primary = WorkspaceMember::new("app-ui", "./frontend", MemberRole::Primary);
    let mut sidecar = WorkspaceMember::new("ai-worker", "../sidecar-ai", MemberRole::Sidecar);
    sidecar.ecosystem = Some("python".to_string());
    sidecar.exec_command = Some("python3".to_string());
    sidecar.exec_args = Some(vec![
        "server.py".to_string(),
        "--port".to_string(),
        "0".to_string(),
    ]);
    sidecar.is_static_binary = false;

    let mut config = WorkspaceProjectConfig::new("dual-repo-project", vec![primary, sidecar]);
    config.version = Some("0.1.0".to_string());
    config.ipc = IpcBridgeConfig {
        socket_id: "ai-worker".to_string(),
        env_var: "DMZ_AI_SOCK".to_string(),
        readiness_timeout_ms: 5000,
        drain_timeout_ms: 2000,
    };

    // Save to disk
    config.save_to_file(&manifest_path).unwrap();
    assert!(manifest_path.exists());

    // Discovery from child directory
    let child_dir = temp_dir.path().join("sub").join("nested");
    fs::create_dir_all(&child_dir).unwrap();
    let discovered = WorkspaceProjectConfig::discover_workspace_manifest(&child_dir);
    assert!(discovered.is_some());

    let (loaded_config, loaded_path) = discovered.unwrap();
    assert_eq!(loaded_path, manifest_path);
    assert_eq!(loaded_config.name, "dual-repo-project");
    assert_eq!(loaded_config.members.len(), 2);

    let prim = loaded_config.primary_member().unwrap();
    assert_eq!(prim.id, "app-ui");
    assert_eq!(
        prim.canonical_virtual_path(),
        PathBuf::from("workspaces/app-ui")
    );

    let sc = loaded_config.find_member("ai-worker").unwrap();
    assert_eq!(sc.role, MemberRole::Sidecar);
    assert_eq!(
        sc.canonical_virtual_path(),
        PathBuf::from("workspaces/ai-worker")
    );
}

#[test]
fn test_safe_socket_path_and_stale_cleanup() {
    let temp_dir = tempfile::tempdir().unwrap();
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
        let temp = tempfile::tempdir().unwrap();
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
fn test_lockfile_multi_workspace_provenance() {
    let pkg_node = LockedPackage {
        name: "react".to_string(),
        version: "18.3.1".to_string(),
        ecosystem: Ecosystem::Npm,
        sha256: "1122334455667788".to_string(),
        source_url: None,
        dependencies: vec![],
        member_id: Some("frontend".to_string()),
    };

    let pkg_rust = LockedPackage {
        name: "serde".to_string(),
        version: "1.0.200".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: "99aabbccddeeff00".to_string(),
        source_url: None,
        dependencies: vec![],
        member_id: Some("backend-engine".to_string()),
    };

    let mut lockfile = DmzLockfile::new(vec![pkg_node, pkg_rust]);
    lockfile.add_workspace(WorkspaceMetadata {
        id: "frontend".to_string(),
        path: "./frontend".to_string(),
        role: "primary".to_string(),
        ecosystem: Some("npm".to_string()),
        closure_signature: None,
    });
    lockfile.add_workspace(WorkspaceMetadata {
        id: "backend-engine".to_string(),
        path: "../backend-engine".to_string(),
        role: "sidecar".to_string(),
        ecosystem: Some("cargo".to_string()),
        closure_signature: None,
    });

    let temp_dir = tempfile::tempdir().unwrap();
    let lockfile_path = temp_dir.path().join("dmz.lock");

    lockfile.save(&lockfile_path).unwrap();
    let loaded = DmzLockfile::load_and_verify(&lockfile_path).unwrap();

    assert_eq!(loaded.workspaces.len(), 2);
    assert_eq!(loaded.packages.len(), 2);
    assert_eq!(
        loaded
            .packages
            .get("react:18.3.1")
            .unwrap()
            .member_id
            .as_deref(),
        Some("frontend")
    );
    assert_eq!(
        loaded
            .packages
            .get("serde:1.0.200")
            .unwrap()
            .member_id
            .as_deref(),
        Some("backend-engine")
    );
}

#[test]
fn test_multi_workspace_toolchain_detection() {
    let temp_dir = tempfile::tempdir().unwrap();
    let ws_a = temp_dir.path().join("app_a");
    let ws_b = temp_dir.path().join("app_b");

    fs::create_dir_all(&ws_a).unwrap();
    fs::create_dir_all(&ws_b).unwrap();

    // ws_a is a Node project
    fs::write(ws_a.join("package.json"), r#"{"name": "app-a"}"#).unwrap();

    // ws_b is a Python project
    fs::write(ws_b.join("requirements.txt"), "flask>=2.0\n").unwrap();

    let members = vec![
        WorkspaceMember::new("app-a", &ws_a, MemberRole::Primary),
        WorkspaceMember::new("app-b", &ws_b, MemberRole::Sidecar),
    ];

    let ecosystems = ToolchainDetector::detect_multi_workspace_ecosystems(&members, None);
    assert!(ecosystems.contains(&ToolchainEcosystem::Node));
    assert!(ecosystems.contains(&ToolchainEcosystem::Python));
}

#[test]
fn test_member_command_detection_and_fail_fast_validation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();

    // 1. Empty directory without manifests or exec_command must fail fast with actionable error
    let empty_dir = root.join("empty-sidecar");
    fs::create_dir_all(&empty_dir).unwrap();
    let empty_member = WorkspaceMember::new("empty-sidecar", &empty_dir, MemberRole::Sidecar);
    let err = empty_member
        .resolve_command(&empty_dir, root)
        .expect_err("Expected fail-fast error for unconfigured member");
    assert!(err.contains("Unable to determine execution command"));
    assert!(err.contains("exec_command"));

    // 2. Node with pnpm-lock.yaml and dev script
    let node_dir = root.join("node-app");
    fs::create_dir_all(&node_dir).unwrap();
    fs::write(
        node_dir.join("package.json"),
        r#"{"name":"node-app","scripts":{"dev":"vite"}}"#,
    )
    .unwrap();
    fs::write(node_dir.join("pnpm-lock.yaml"), "lockfileVersion: '6.0'\n").unwrap();
    let node_member = WorkspaceMember::new("node-app", &node_dir, MemberRole::Primary);
    let (cmd, args) = node_member.resolve_command(&node_dir, root).unwrap();
    assert_eq!(cmd, "pnpm");
    assert_eq!(args, vec!["run", "dev"]);

    // 3. Python with pyproject.toml [project.scripts]
    let py_dir = root.join("py-worker");
    fs::create_dir_all(&py_dir).unwrap();
    fs::write(
        py_dir.join("pyproject.toml"),
        "[project]\nname = \"py-worker\"\n[project.scripts]\nworker = \"worker_pkg.cli:run\"\n",
    )
    .unwrap();
    let py_member = WorkspaceMember::new("py-worker", &py_dir, MemberRole::Sidecar);
    let (py_cmd, py_args) = py_member.resolve_command(&py_dir, root).unwrap();
    assert!(py_cmd.starts_with("python"));
    assert_eq!(
        py_args,
        vec![
            "-c",
            "import sys, worker_pkg.cli; sys.exit(worker_pkg.cli.run())"
        ]
    );

    // 4. Go with go.mod
    let go_dir = root.join("go-svc");
    fs::create_dir_all(&go_dir).unwrap();
    fs::write(go_dir.join("go.mod"), "module example.com/gosvc\ngo 1.22\n").unwrap();
    let go_member = WorkspaceMember::new("go-svc", &go_dir, MemberRole::Service);
    let (go_cmd, go_args) = go_member.resolve_command(&go_dir, root).unwrap();
    assert_eq!(go_cmd, "go");
    assert_eq!(go_args, vec!["run", "."]);
}

#[test]
fn test_structured_abi_binary_header_diagnostics() {
    use dmz_core::workspace::analyze_sidecar_abi;

    let temp = tempfile::tempdir().unwrap();
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
