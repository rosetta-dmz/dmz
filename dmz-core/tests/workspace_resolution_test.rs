use dmz_core::resolver::ResolverEngine;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_workspace_resolution_fail_fast_and_inheritance() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Create root Cargo.toml with workspace and inheritance
    let root_cargo = r#"
        [workspace]
        resolver = "2"
        members = ["crate-a"]

        [workspace.dependencies]
        serde = "1.0"
    "#;
    fs::write(root.join("Cargo.toml"), root_cargo).unwrap();

    // Create workspace member crate-a using workspace inheritance
    fs::create_dir_all(root.join("crate-a/src")).unwrap();
    let member_cargo = r#"
        [package]
        name = "crate-a"
        version = "1.0.8"
        edition = "2021"

        [dependencies]
        serde = { workspace = true }
    "#;
    fs::write(root.join("crate-a/Cargo.toml"), member_cargo).unwrap();
    fs::write(root.join("crate-a/src/lib.rs"), "").unwrap();

    // Run resolver engine and verify packages are correctly populated
    let lockfile_result = ResolverEngine::resolve_workspace(root);
    assert!(
        lockfile_result.is_ok(),
        "Resolver failed on valid workspace structure"
    );

    let lockfile = lockfile_result.unwrap();
    assert!(
        lockfile.packages.contains_key("serde:1.0"),
        "Inherited workspace dependency 'serde' was missing from closure packages"
    );
}

#[test]
fn test_fail_fast_on_empty_workspace() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Create empty workspace Cargo.toml with no dependencies or members
    fs::write(root.join("Cargo.toml"), "[workspace]\nresolver = \"2\"").unwrap();

    let lockfile_result = ResolverEngine::resolve_workspace(root);
    assert!(
        lockfile_result.is_err(),
        "Resolver should have triggered Fail-Fast Guardrail for 0 packages resolved"
    );
}

#[test]
fn test_npm_workspace_resolution() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Create root package.json with workspace glob patterns
    let root_pkg = r#"{
        "name": "root-monorepo",
        "workspaces": ["packages/*"]
    }"#;
    fs::write(root.join("package.json"), root_pkg).unwrap();

    // Create sub-package directory and manifest (with valid quoted keys)
    fs::create_dir_all(root.join("packages/app-a")).unwrap();
    let sub_pkg = r#"{
        "name": "app-a",
        "dependencies": {
            "express": "4.19.2"
        },
        "devDependencies": {
            "jest": "29.7.0"
        }
    }"#;
    fs::write(root.join("packages/app-a/package.json"), sub_pkg).unwrap();

    // Resolve workspace and verify npm packages are captured
    let lockfile_result = ResolverEngine::resolve_workspace(root);
    assert!(
        lockfile_result.is_ok(),
        "Resolver failed on valid NPM workspace structure"
    );

    let lockfile = lockfile_result.unwrap();
    assert!(
        lockfile.packages.contains_key("express:4.19.2"),
        "NPM workspace dependency 'express' was missing from closure packages"
    );
    assert!(
        lockfile.packages.contains_key("jest:29.7.0"),
        "NPM workspace devDependency 'jest' was missing from closure packages"
    );
}

#[test]
fn test_mixed_cargo_workspace_and_paths() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Root Cargo.toml with workspace and path-based member
    let root_cargo = r#"
        [workspace]
        resolver = "2"
        members = ["dmz-core"]

        [workspace.dependencies]
        tokio = "1.38"
    "#;
    fs::write(root.join("Cargo.toml"), root_cargo).unwrap();

    fs::create_dir_all(root.join("dmz-core/src")).unwrap();
    let core_cargo = r#"
        [package]
        name = "dmz-core"
        version = "1.0.8"
        edition = "2021"

        [dependencies]
        tokio = { workspace = true }
        serde = "1.0"
    "#;
    fs::write(root.join("dmz-core/Cargo.toml"), core_cargo).unwrap();
    fs::write(root.join("dmz-core/src/lib.rs"), "").unwrap();

    let lockfile_result = ResolverEngine::resolve_workspace(root);
    assert!(lockfile_result.is_ok());

    let lockfile = lockfile_result.unwrap();
    assert!(lockfile.packages.contains_key("tokio:1.38"));
    assert!(lockfile.packages.contains_key("serde:1.0"));
}

#[test]
fn test_version_range_sanitization_and_operators() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // Create a package.json with semver ranges (^ and ~)
    let root_pkg = r#"{
        "name": "sanitization-test",
        "dependencies": {
            "react": "^18.2.0",
            "lodash": "~4.17.21"
        }
    }"#;
    fs::write(root.join("package.json"), root_pkg).unwrap();

    // Create requirements.txt with various comparison operators (>=, ~=, ==)
    let reqs = r#"
        requests >= 2.28.0
        flask ~= 2.2.0
        numpy == 1.24.3
    "#;
    fs::write(root.join("requirements.txt"), reqs).unwrap();

    // Resolve workspace
    let lockfile_result = ResolverEngine::resolve_workspace(root);
    assert!(
        lockfile_result.is_ok(),
        "Resolver failed on semver range sanitization test"
    );

    let lockfile = lockfile_result.unwrap();

    // Verify version requirements are clean and range/comparison symbols are stripped
    assert!(
        lockfile.packages.contains_key("react:18.2.0"),
        "Caret symbol '^' should be stripped from react:^18.2.0, expected react:18.2.0"
    );
    assert!(
        lockfile.packages.contains_key("lodash:4.17.21"),
        "Tilde symbol '~' should be stripped from lodash:~4.17.21, expected lodash:4.17.21"
    );
    assert!(
        lockfile.packages.contains_key("requests:2.28.0"),
        "Operator '>=' should be parsed out from requests>=2.28.0, expected requests:2.28.0"
    );
    assert!(
        lockfile.packages.contains_key("flask:2.2.0"),
        "Operator '~=' should be parsed out from flask~=2.2.0, expected flask:2.2.0"
    );
    assert!(
        lockfile.packages.contains_key("numpy:1.24.3"),
        "Operator '==' should be parsed out from numpy==1.24.3, expected numpy:1.24.3"
    );
}
