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

    // Sandbox with --sidecar and --strict-kernel
    let args = vec![
        "dmz",
        "sandbox",
        "--workspace",
        "./my-app",
        "--sidecar",
        "./my-sidecar",
        "--allow-network",
        "--strict-kernel",
    ];
    let parsed = Cli::try_parse_from(args).expect("Failed to parse sandbox command");
    match parsed.command {
        Commands::Sandbox {
            workspace,
            sidecar,
            allow_network,
            strict_kernel,
            ..
        } => {
            assert_eq!(workspace, PathBuf::from("./my-app"));
            assert_eq!(sidecar, Some(PathBuf::from("./my-sidecar")));
            assert!(allow_network);
            assert!(strict_kernel);
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

    // Must be short enough to prevent POSIX sun_path buffer overflow (104 on macOS, 108 on Linux)
    #[cfg(unix)]
    {
        let sock_str = safe_sock.to_string_lossy();
        assert!(
            sock_str.len() < 104,
            "Safe socket path exceeded 104 characters: {}",
            sock_str
        );
    }

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

#[test]
fn test_cli_include_dev_and_toolchain_profile_defaults_and_overrides(
) -> Result<(), Box<dyn std::error::Error>> {
    use dmz_cli::cli::CliToolchainProfile;

    let res_default = Cli::try_parse_from(["dmz", "resolve"])?;
    if let Commands::Resolve {
        include_dev,
        prod_only,
        ..
    } = res_default.command
    {
        assert!(include_dev);
        assert!(!prod_only);
    } else {
        return Err("Expected Commands::Resolve".into());
    }

    let res_prod = Cli::try_parse_from(["dmz", "resolve", "--prod-only"])?;
    if let Commands::Resolve {
        include_dev,
        prod_only,
        ..
    } = res_prod.command
    {
        assert!(include_dev);
        assert!(prod_only);
    } else {
        return Err("Expected Commands::Resolve".into());
    }

    let bundle_default = Cli::try_parse_from(["dmz", "bundle"])?;
    if let Commands::Bundle(args) = bundle_default.command {
        assert!(args.include_dev_enabled());
        assert!(!args.allow_missing_toolchains);
        assert_eq!(args.toolchain_profile, CliToolchainProfile::Minimal);
    } else {
        return Err("Expected Commands::Bundle".into());
    }

    let bundle_allow_missing =
        Cli::try_parse_from(["dmz", "bundle", "--allow-missing-toolchains"])?;
    if let Commands::Bundle(args) = bundle_allow_missing.command {
        assert!(args.allow_missing_toolchains);
    } else {
        return Err("Expected Commands::Bundle".into());
    }

    let bundle_prod =
        Cli::try_parse_from(["dmz", "bundle", "--no-dev", "--toolchain-profile", "full"])?;
    if let Commands::Bundle(args) = bundle_prod.command {
        assert!(!args.include_dev_enabled());
        assert_eq!(args.toolchain_profile, CliToolchainProfile::Full);
    } else {
        return Err("Expected Commands::Bundle".into());
    }

    // --include-dev passed after --prod-only overrides back to true (explicit intent wins)
    let bundle_override = Cli::try_parse_from(["dmz", "bundle", "--prod-only", "--include-dev"])?;
    if let Commands::Bundle(args) = bundle_override.command {
        assert!(args.include_dev_enabled());
    } else {
        return Err("Expected Commands::Bundle".into());
    }

    let export_full = Cli::try_parse_from([
        "dmz",
        "export",
        "--output",
        "out.tar.zst",
        "--toolchain-profile",
        "full",
        "--allow-missing-toolchains",
    ])?;
    if let Commands::Export(args) = export_full.command {
        assert!(args.include_dev_enabled());
        assert!(args.allow_missing_toolchains);
        assert_eq!(args.toolchain_profile, CliToolchainProfile::Full);
    } else {
        return Err("Expected Commands::Export".into());
    }

    let export_prod =
        Cli::try_parse_from(["dmz", "export", "--output", "out.tar.zst", "--prod-only"])?;
    if let Commands::Export(args) = export_prod.command {
        assert!(!args.include_dev_enabled());
    } else {
        return Err("Expected Commands::Export".into());
    }

    let import_dev =
        Cli::try_parse_from(["dmz", "import", "--archive", "in.tar.zst", "--include-dev"])?;
    if let Commands::Import(args) = import_dev.command {
        assert!(args.include_dev_enabled());
    } else {
        return Err("Expected Commands::Import".into());
    }

    let import_prod =
        Cli::try_parse_from(["dmz", "import", "--archive", "in.tar.zst", "--prod-only"])?;
    if let Commands::Import(args) = import_prod.command {
        assert!(!args.include_dev_enabled());
    } else {
        return Err("Expected Commands::Import".into());
    }

    Ok(())
}

#[test]
fn test_cli_bundle_ignores_unrelated_ancestor_dmz_workspace_toml(
) -> Result<(), Box<dyn std::error::Error>> {
    use dmz_core::resolver::lockfile::{DmzLockfile, LockedPackage};
    use dmz_core::resolver::manifest::Ecosystem;

    let tmp = tempfile::tempdir()?;
    let ancestor = tmp.path();
    let child_ws = ancestor.join("standalone-app");
    let out_archive = child_ws.join("dist/bundle.tar.zst");

    // Ancestor umbrella referencing an unrelated non-existent sibling member.
    // If `run_bundle` climbed ancestors, exporting the umbrella would fail.
    std::fs::write(
        ancestor.join("dmz-workspace.toml"),
        r#"
name = "unrelated-umbrella"
[[members]]
id = "missing-sidecar"
path = "nonexistent-sibling"
role = "sidecar"
ecosystem = "python"
"#,
    )?;

    std::fs::create_dir_all(child_ws.join("src"))?;
    std::fs::write(
        child_ws.join("Cargo.toml"),
        "[package]\nname = \"standalone-app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    std::fs::write(child_ws.join("src/lib.rs"), "pub fn ok() {}\n")?;

    let lockfile = DmzLockfile::new(vec![LockedPackage {
        name: "standalone-app".to_string(),
        version: "path:Cargo.toml".to_string(),
        ecosystem: Ecosystem::Cargo,
        sha256: DmzLockfile::hash_bytes(b"standalone-app"),
        source_url: None,
        dependencies: Vec::new(),
        member_id: None,
    }]);
    lockfile.save(&child_ws.join("dmz.lock"))?;

    let ws_str = child_ws.to_string_lossy().into_owned();
    let out_str = out_archive.to_string_lossy().into_owned();
    let parsed = Cli::try_parse_from([
        "dmz",
        "bundle",
        "--workspace",
        &ws_str,
        "--output",
        &out_str,
        "--mode",
        "code-only",
        "--no-toolchain",
    ])?;

    let exit_code = dmz_cli::commands::dispatch(parsed.command)?;
    assert_eq!(exit_code, 0);
    assert!(out_archive.is_file());

    Ok(())
}

#[test]
fn test_cli_strict_metadata_flag_parsing() -> Result<(), Box<dyn std::error::Error>> {
    let sb_default = Cli::try_parse_from(["dmz", "sandbox"])?;
    if let Commands::Sandbox {
        strict_metadata, ..
    } = sb_default.command
    {
        assert!(!strict_metadata);
    } else {
        return Err("Expected Commands::Sandbox".into());
    }

    let sb_strict = Cli::try_parse_from(["dmz", "sandbox", "--strict-metadata"])?;
    if let Commands::Sandbox {
        strict_metadata, ..
    } = sb_strict.command
    {
        assert!(strict_metadata);
    } else {
        return Err("Expected Commands::Sandbox".into());
    }

    let sh_default = Cli::try_parse_from(["dmz", "shell"])?;
    if let Commands::Shell {
        strict_metadata, ..
    } = sh_default.command
    {
        assert!(!strict_metadata);
    } else {
        return Err("Expected Commands::Shell".into());
    }

    let sh_strict = Cli::try_parse_from(["dmz", "shell", "--strict-metadata"])?;
    if let Commands::Shell {
        strict_metadata, ..
    } = sh_strict.command
    {
        assert!(strict_metadata);
    } else {
        return Err("Expected Commands::Shell".into());
    }

    let run_strict = Cli::try_parse_from(["dmz", "run", "--strict-metadata"])?;
    if let Commands::Run(run_args) = run_strict.command {
        assert!(run_args.strict_metadata);
    } else {
        return Err("Expected Commands::Run".into());
    }

    Ok(())
}

#[test]
fn test_cli_run_nested_sandbox_guard() {
    let ws = Path::new("./project");
    assert!(dmz_cli::commands::run::nested_sandbox_error(None, None, ws).is_none());
    assert!(dmz_cli::commands::run::nested_sandbox_error(Some("0"), Some("/tmp/ws"), ws).is_none());

    let msg = dmz_cli::commands::run::nested_sandbox_error(Some("1"), Some("/tmp/ws"), ws);
    assert!(msg.is_some());
    if let Some(text) = msg {
        assert!(text.contains("cannot be nested"));
        assert!(text.contains("DMZ_WORKSPACE=/tmp/ws"));
        assert!(text.contains("exit"));
        assert!(text.contains("dmz run --workspace ./project"));
    }
}

#[test]
fn test_cli_import_next_steps_dual_vs_single_repo() -> Result<(), Box<dyn std::error::Error>> {
    use dmz_core::archive::ImportResult;
    use dmz_core::workspace::{MemberRole, WorkspaceMember, WorkspaceProjectConfig};

    let tmp = tempfile::tempdir()?;
    let target = Path::new("./imported-dual");

    let dual_project = WorkspaceProjectConfig::new(
        "desktop-ai-suite",
        vec![
            WorkspaceMember::new("client", "workspaces/client", MemberRole::Primary),
            WorkspaceMember::new("backend", "workspaces/backend", MemberRole::Sidecar),
        ],
    );
    let dual_result = ImportResult {
        imported_packages: 10,
        installed_packages: 10,
        install_warning: None,
        closure_signature: "abc123".to_string(),
        store_path: tmp.path().join(".dmz/store"),
        workspace_path: tmp.path().to_path_buf(),
        is_delta: false,
        has_git_history: false,
        has_toolchains: true,
        workspace_project: Some(dual_project),
        vcs: None,
    };
    let dual_steps = dmz_cli::commands::import::format_import_next_steps(target, &dual_result);
    assert!(dual_steps.contains("cd ./imported-dual"));
    assert!(dual_steps.contains("dmz run --workspace ."));
    assert!(dual_steps.contains("host shell"));
    assert!(dual_steps.contains("code desktop-ai-suite.code-workspace"));

    // Verify shell-quoting for paths containing spaces and read_dir fallback when proj.name differs from disk
    std::fs::write(tmp.path().join("actual-on-disk.code-workspace"), "{}")?;
    let spaced_steps = dmz_cli::commands::import::format_import_next_steps(
        Path::new("/tmp/my imported ws"),
        &dual_result,
    );
    if cfg!(windows) {
        assert!(spaced_steps.contains("cd \"/tmp/my imported ws\""));
    } else {
        assert!(spaced_steps.contains("cd '/tmp/my imported ws'"));
    }
    assert!(spaced_steps.contains("code actual-on-disk.code-workspace"));

    let single_result = ImportResult {
        workspace_project: None,
        ..dual_result
    };
    let single_steps =
        dmz_cli::commands::import::format_import_next_steps(Path::new("./single"), &single_result);
    assert!(single_steps.contains("cd ./single"));
    assert!(single_steps.contains("dmz shell"));
    if cfg!(windows) {
        assert!(single_steps.contains(r".dmz\env.bat"));
    } else {
        assert!(single_steps.contains("source .dmz/env.sh"));
    }
    assert!(!single_steps.contains("dmz run --workspace ."));

    Ok(())
}
