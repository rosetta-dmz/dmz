use clap::Parser;
use dmz_cli::cli::{Cli, Commands};
use dmz_core::workspace::{analyze_sidecar_abi, check_socket_path_length, safe_socket_path};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn test_cli_sidecar_flag_parsing() {
    // 1. Resolve with --sidecar
    let args = vec![
        "dmz",
        "resolve",
        "--workspace",
        "./my-app",
        "--sidecar",
        "./my-sidecar",
        "--output",
        "custom.lock",
    ];
    let parsed = Cli::try_parse_from(args).expect("Failed to parse resolve command");
    match parsed.command {
        Commands::Resolve {
            workspace,
            sidecar,
            output,
            ..
        } => {
            assert_eq!(workspace, PathBuf::from("./my-app"));
            assert_eq!(sidecar, Some(PathBuf::from("./my-sidecar")));
            assert_eq!(output, PathBuf::from("custom.lock"));
        }
        _ => panic!("Expected Commands::Resolve"),
    }

    // 2. Bundle with --sidecar
    let args = vec![
        "dmz",
        "bundle",
        "--workspace",
        "./my-app",
        "--sidecar",
        "./my-sidecar",
        "--output",
        "dist/bundle.tar.zst",
    ];
    let parsed = Cli::try_parse_from(args).expect("Failed to parse bundle command");
    match parsed.command {
        Commands::Bundle {
            workspace,
            sidecar,
            output,
            ..
        } => {
            assert_eq!(workspace, PathBuf::from("./my-app"));
            assert_eq!(sidecar, Some(PathBuf::from("./my-sidecar")));
            assert_eq!(output, PathBuf::from("dist/bundle.tar.zst"));
        }
        _ => panic!("Expected Commands::Bundle"),
    }

    // 3. Shell with --sidecar
    let args = vec![
        "dmz",
        "shell",
        "--workspace",
        "./my-app",
        "--sidecar",
        "./my-sidecar",
        "--no-sandbox",
    ];
    let parsed = Cli::try_parse_from(args).expect("Failed to parse shell command");
    match parsed.command {
        Commands::Shell {
            workspace,
            sidecar,
            no_sandbox,
            ..
        } => {
            assert_eq!(workspace, PathBuf::from("./my-app"));
            assert_eq!(sidecar, Some(PathBuf::from("./my-sidecar")));
            assert!(no_sandbox);
        }
        _ => panic!("Expected Commands::Shell"),
    }

    // 4. Sandbox with --sidecar
    let args = vec![
        "dmz",
        "sandbox",
        "--workspace",
        "./my-app",
        "--sidecar",
        "./my-sidecar",
        "--allow-network",
    ];
    let parsed = Cli::try_parse_from(args).expect("Failed to parse sandbox command");
    match parsed.command {
        Commands::Sandbox {
            workspace,
            sidecar,
            allow_network,
            ..
        } => {
            assert_eq!(workspace, PathBuf::from("./my-app"));
            assert_eq!(sidecar, Some(PathBuf::from("./my-sidecar")));
            assert!(allow_network);
        }
        _ => panic!("Expected Commands::Sandbox"),
    }
}

#[test]
fn test_cli_sidecar_abi_diagnostic() {
    let temp_dir = tempfile::tempdir().unwrap();
    let sidecar_dir = temp_dir.path().join("sidecar_engine");
    fs::create_dir_all(&sidecar_dir).unwrap();

    // Create a mock native dynamic addon
    let addon = sidecar_dir.join("native_module.node");
    fs::write(&addon, b"mock node native addon binary").unwrap();

    let report = analyze_sidecar_abi(&sidecar_dir);
    assert_eq!(report.dynamic_libraries.len(), 1);
    assert_eq!(report.dynamic_libraries[0], addon);
    assert!(!report.warnings.is_empty());
    assert!(report.warnings[0].contains("dynamic shared library file(s)"));
    assert!(!report.is_static);
}

#[test]
fn test_cli_safe_socket_path_length_guard() {
    let workspace = Path::new("/Users/developer/very/deep/nested/project/directory/structure");
    let safe_sock = safe_socket_path(workspace, "sidecar");
    let sock_str = safe_sock.to_string_lossy();

    // Must be short enough to prevent POSIX sun_path buffer overflow (104 on macOS, 108 on Linux)
    #[cfg(unix)]
    assert!(
        sock_str.len() < 104,
        "Safe socket path exceeded 104 characters: {}",
        sock_str
    );

    // Advisory check should pass for safe_socket
    assert!(check_socket_path_length(&safe_sock).is_none());

    // Artificial dangerous path should trigger advisory
    let dangerous_path = PathBuf::from(format!("/var/run/dmz/{}.sock", "x".repeat(110)));
    assert!(check_socket_path_length(&dangerous_path).is_some());

    #[cfg(windows)]
    {
        let dangerous_pipe = PathBuf::from(format!(r"\\.\pipe\dmz_{}", "x".repeat(260)));
        assert!(check_socket_path_length(&dangerous_pipe).is_some());
    }
}

#[test]
fn test_cli_sandbox_defaults_to_local_store() {
    let args = vec!["dmz", "sandbox"];
    let parsed = Cli::try_parse_from(args).expect("Failed to parse sandbox default command");
    match parsed.command {
        Commands::Sandbox {
            closure,
            exec_command,
            ..
        } => {
            assert_eq!(closure, PathBuf::from(".dmz/store"));
            if cfg!(windows) {
                assert_eq!(exec_command, "cmd.exe");
            } else {
                assert_eq!(exec_command, "/bin/sh");
            }
        }
        _ => panic!("Expected Commands::Sandbox"),
    }
}
