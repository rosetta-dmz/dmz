use clap::Parser;
use dmz_cli::cli::{Cli, Commands, TargetMode, DEFAULT_STORE_PATH};
use dmz_core::workspace::{analyze_sidecar_abi, check_socket_path_length, safe_socket_path};
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn test_cli_sidecar_flag_parsing() {
    // Resolve with --sidecar
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

    // Bundle with --sidecar
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
        Commands::Bundle(bundle_args) => {
            assert_eq!(bundle_args.workspace, PathBuf::from("./my-app"));
            assert_eq!(bundle_args.sidecar, Some(PathBuf::from("./my-sidecar")));
            assert_eq!(bundle_args.output, PathBuf::from("dist/bundle.tar.zst"));
        }
        _ => panic!("Expected Commands::Bundle"),
    }

    // Shell with --sidecar
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

    // Sandbox with --sidecar
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
            assert_eq!(closure, PathBuf::from(DEFAULT_STORE_PATH));
            if cfg!(windows) {
                assert_eq!(exec_command, "cmd.exe");
            } else {
                assert_eq!(exec_command, "/bin/sh");
            }
        }
        _ => panic!("Expected Commands::Sandbox"),
    }
}

#[test]
fn test_cli_run_subcommand_parsing() {
    // 1. Basic dmz run --workspace .
    let args = vec!["dmz", "run", "--workspace", "."];
    let parsed = Cli::try_parse_from(args).expect("Failed to parse run command");
    match parsed.command {
        Commands::Run(run_args) => {
            assert_eq!(run_args.workspace, PathBuf::from("."));
            assert_eq!(run_args.sidecar, None);
            assert!(!run_args.allow_network);
            assert!(!run_args.no_sandbox);
            assert_eq!(run_args.command, None);
            assert!(run_args.args.is_empty());
        }
        _ => panic!("Expected Commands::Run"),
    }

    // 2. dmz run with companion sidecar and custom command
    let args = vec![
        "dmz",
        "run",
        "--workspace",
        "./client",
        "--sidecar",
        "./server",
        "--allow-network",
        "-c",
        "npm run tauri dev",
    ];
    let parsed =
        Cli::try_parse_from(args).expect("Failed to parse run command with custom command");
    match parsed.command {
        Commands::Run(run_args) => {
            assert_eq!(run_args.workspace, PathBuf::from("./client"));
            assert_eq!(run_args.sidecar, Some(PathBuf::from("./server")));
            assert!(run_args.allow_network);
            assert_eq!(run_args.command, Some("npm run tauri dev".to_string()));
        }
        _ => panic!("Expected Commands::Run"),
    }
}

#[test]
fn test_cli_shell_defaults_to_local_store() {
    let args = vec!["dmz", "shell"];
    let parsed = Cli::try_parse_from(args).expect("Failed to parse shell default command");
    match parsed.command {
        Commands::Shell { closure, .. } => {
            assert_eq!(
                closure,
                PathBuf::from(DEFAULT_STORE_PATH),
                "dmz shell must default closure store to DEFAULT_STORE_PATH"
            );
        }
        _ => panic!("Expected Commands::Shell"),
    }
}

#[test]
fn test_cli_export_subcommand_parsing() {
    let args = vec![
        "dmz",
        "export",
        "--workspace",
        "./test-app",
        "--output",
        "dist/archive.tar.zst",
    ];
    let parsed = Cli::try_parse_from(args).expect("Failed to parse export command");
    match parsed.command {
        Commands::Export(args) => {
            assert_eq!(args.workspace, PathBuf::from("./test-app"));
            assert_eq!(args.output, PathBuf::from("dist/archive.tar.zst"));
        }
        _ => panic!("Expected Commands::Export"),
    }
}

#[test]
fn test_cli_bundle_args_toolchain_flags() {
    // Default has with_toolchain = true, no_toolchain = false
    let args = vec!["dmz", "bundle"];
    let parsed = Cli::try_parse_from(args).expect("Failed to parse bundle command");
    if let Commands::Bundle(bundle_args) = parsed.command {
        assert!(bundle_args.toolchain_enabled());
    } else {
        panic!("Expected Commands::Bundle");
    }

    // Explicit --no-toolchain disables it
    let args_no = vec!["dmz", "bundle", "--no-toolchain"];
    let parsed_no =
        Cli::try_parse_from(args_no).expect("Failed to parse bundle command with --no-toolchain");
    if let Commands::Bundle(bundle_args) = parsed_no.command {
        assert!(!bundle_args.toolchain_enabled());
    } else {
        panic!("Expected Commands::Bundle");
    }
}

#[test]
fn test_cli_dispatch_error_propagation_no_exit() {
    // Calling dispatch with nonexistent lockfile must return Ok(1) rather than calling std::process::exit(1)
    let bad_cmd = Commands::Fetch {
        lockfile: PathBuf::from("/nonexistent/path/dmz.lock"),
        store: PathBuf::from("/nonexistent/store"),
    };
    let res = dmz_cli::commands::dispatch(bad_cmd);
    assert!(res.is_ok());
    assert_eq!(res.unwrap(), 1, "Expected exit code 1 on missing lockfile");
}

#[test]
fn test_cli_bundle_zstd_level_bounds_validation() {
    // Value 0 is below range 1..=22
    let args_low = vec!["dmz", "bundle", "--zstd-level", "0"];
    assert!(Cli::try_parse_from(args_low).is_err());

    // Value 23 is above range 1..=22
    let args_high = vec!["dmz", "bundle", "--zstd-level", "23"];
    assert!(Cli::try_parse_from(args_high).is_err());

    // Valid values 1 and 22 must parse cleanly
    let args_valid_1 = vec!["dmz", "bundle", "--zstd-level", "1"];
    assert!(Cli::try_parse_from(args_valid_1).is_ok());

    let args_valid_22 = vec!["dmz", "bundle", "--zstd-level", "22"];
    assert!(Cli::try_parse_from(args_valid_22).is_ok());
}

#[test]
fn test_cli_build_mode_alias_parsing() {
    let args_target = vec!["dmz", "build", "--target", "app"];
    let parsed_target = Cli::try_parse_from(args_target).expect("Failed to parse build --target");
    if let Commands::Build { target, .. } = parsed_target.command {
        assert_eq!(target, TargetMode::App);
    } else {
        panic!("Expected Commands::Build");
    }

    let args_mode = vec!["dmz", "build", "--mode", "app"];
    let parsed_mode = Cli::try_parse_from(args_mode).expect("Failed to parse build --mode");
    if let Commands::Build { target, .. } = parsed_mode.command {
        assert_eq!(target, TargetMode::App);
    } else {
        panic!("Expected Commands::Build");
    }
}

#[test]
fn test_cli_resolve_workspace_relative_path_resolution() {
    let ws = Path::new("/workspace");

    // 1. Matches default candidate name -> joins directly with workspace
    let res_default = dmz_cli::commands::resolve_workspace_relative_path(
        ws,
        Path::new(".dmz/store"),
        ".dmz/store",
    );
    assert_eq!(res_default, PathBuf::from("/workspace/.dmz/store"));

    // 2. Absolute candidate path -> preserved as-is
    let abs_candidate = Path::new("/var/dmz/custom_store");
    let res_abs =
        dmz_cli::commands::resolve_workspace_relative_path(ws, abs_candidate, ".dmz/store");
    assert_eq!(res_abs, PathBuf::from("/var/dmz/custom_store"));

    // 3. Relative non-default candidate -> resolved under workspace
    let rel_candidate = Path::new("sub/custom_store");
    let res_rel =
        dmz_cli::commands::resolve_workspace_relative_path(ws, rel_candidate, ".dmz/store");
    assert_eq!(res_rel, PathBuf::from("/workspace/sub/custom_store"));
}
