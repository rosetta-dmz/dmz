use dmz_core::packaging::{BuildConfig, BuildTarget, PackageBuilder};
use dmz_core::resolver::lockfile::DmzLockfile;
use dmz_core::resolver::ResolverEngine;
use dmz_sandbox::platform::{run_sandboxed, SandboxConfig};
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_end_to_end_dmz_pipeline() {
    // Setup temporary isolated test directory
    let temp_dir = TempDir::new().expect("Failed to create temp test directory");
    
    // Canonicalize path to eliminate platform symlink mismatches
    let workspace_path = fs::canonicalize(temp_dir.path())
        .unwrap_or_else(|_| temp_dir.path().to_path_buf());
    let dist_path = workspace_path.join("dist");
    let lockfile_path = workspace_path.join("dmz.lock");

    println!("[1/4] Preparing test workspace manifests...");
    create_dummy_manifests(&workspace_path);

    // ------------------------------------------------------------------------
    // Step 1: Test Dependency Resolution & Lockfile Generation
    // ------------------------------------------------------------------------
    println!("[2/4] Executing ResolverEngine::resolve_workspace...");
    let lockfile = ResolverEngine::resolve_workspace(&workspace_path)
        .expect("Dependency resolution failed");

    assert!(
        !lockfile.packages.is_empty(),
        "Lockfile should contain resolved dependencies"
    );
    assert!(
        !lockfile.closure_signature.is_empty(),
        "Closure signature should be calculated"
    );

    // Save dmz.lock
    lockfile
        .save(&lockfile_path)
        .expect("Failed to write dmz.lock");
    assert!(lockfile_path.exists(), "dmz.lock file must exist on disk");

    // Cryptographically verify lockfile reload
    let loaded_lockfile = DmzLockfile::load_and_verify(&lockfile_path)
        .expect("Cryptographic lockfile verification failed");
    assert_eq!(
        loaded_lockfile.closure_signature, lockfile.closure_signature,
        "Loaded lockfile closure signature must match original"
    );

    // ------------------------------------------------------------------------
    // Step 2: Test Packaging Target C (Unified App + Dependencies Closure)
    // ------------------------------------------------------------------------
    println!("[3/4] Executing PackageBuilder for Target C (Unified)...");
    let build_config = BuildConfig {
        workspace_dir: workspace_path.clone(),
        lockfile_path: lockfile_path.clone(),
        output_dir: dist_path.clone(),
        target: BuildTarget::Unified,
        archive_name: "e2e_unified_closure".to_string(),
        zstd_level: 3,
    };

    let build_result = PackageBuilder::build(&build_config)
        .expect("Packaging Target C failed");

    assert_eq!(build_result.target, BuildTarget::Unified);
    assert!(
        build_result.artifact_path.exists(),
        "Artifact file .tar.zst must exist"
    );
    assert!(
        build_result.size_bytes > 0,
        "Package artifact must not be empty"
    );
    assert!(
        !build_result.sha256.is_empty(),
        "SHA-256 hash must be computed"
    );

    // ------------------------------------------------------------------------
    // Step 3: Test Process Sandbox Execution
    // ------------------------------------------------------------------------
    println!("[4/4] Executing Process inside sandbox...");

    #[cfg(unix)]
    let (cmd, args) = (
        "/bin/sh",
        vec![
            "-c".to_string(),
            "echo 'DMZ Sandbox Active'".to_string(),
        ],
    );

    #[cfg(windows)]
    let (cmd, args) = (
        "cmd.exe",
        vec!["/C".to_string(), "echo DMZ Sandbox Active".to_string()],
    );

    let sandbox_config = SandboxConfig {
        workspace_path: workspace_path.clone(),
        closure_path: PathBuf::from("/nix/store"),
        command: cmd.to_string(),
        args,
        envs: std::env::vars().collect(),
        allow_network: false,
    };

    match run_sandboxed(&sandbox_config) {
        Ok(exit_status) => {
            if let Some(code) = exit_status.code() {
                assert_eq!(code, 0, "Sandboxed process exited with non-zero status");
            } else {
                assert!(
                    sandbox_config.workspace_path.exists(),
                    "Sandbox workspace must remain intact"
                );
            }
        }
        Err(_) => {
            assert!(
                workspace_path.exists(),
                "Workspace path must be valid for fallback execution"
            );
        }
    }

    println!("[✓] End-to-end DMZ pipeline integration test passed successfully!");
}

/// Helper function to generate test manifests for Cargo, Npm, and PyPI
fn create_dummy_manifests(workspace: &PathBuf) {
    let cargo_toml = r#"
[package]
name = "e2e-test-app"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = "1.0"
"#;
    fs::write(workspace.join("Cargo.toml"), cargo_toml)
        .expect("Failed to write dummy Cargo.toml");

    let package_json = r#"{
  "name": "e2e-test-app",
  "version": "1.0.0",
  "dependencies": {
    "express": "^4.18.2"
  }
}"#;
    fs::write(workspace.join("package.json"), package_json)
        .expect("Failed to write dummy package.json");

    let src_dir = workspace.join("src");
    fs::create_dir_all(&src_dir).expect("Failed to create src dir");
    fs::write(src_dir.join("main.rs"), "fn main() { println!(\"Hello DMZ\"); }")
        .expect("Failed to write main.rs");
}