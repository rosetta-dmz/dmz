use std::fs;
use tempfile::tempdir;
use dmz_core::resolver::ResolverEngine;

#[test]
fn test_workspace_resolution_fail_fast_and_inheritance() {
    let tmp = tempdir().unwrap();
    let root = tmp.path();

    // 1. Create root Cargo.toml with workspace and inheritance
    let root_cargo = r#"
        [workspace]
        resolver = "2"
        members = ["crate-a"]

        [workspace.dependencies]
        serde = "1.0"
    "#;
    fs::write(root.join("Cargo.toml"), root_cargo).unwrap();

    // 2. Create workspace member crate-a using workspace inheritance
    fs::create_dir_all(root.join("crate-a")).unwrap();
    let member_cargo = r#"
        [package]
        name = "crate-a"
        version = "0.1.0"
        edition = "2021"

        [dependencies]
        serde = { workspace = true }
    "#;
    fs::write(root.join("crate-a/Cargo.toml"), member_cargo).unwrap();

    // 3. Run resolver engine and verify packages are correctly populated
    let lockfile_result = ResolverEngine::resolve_workspace(root);
    assert!(lockfile_result.is_ok(), "Resolver failed on valid workspace structure");

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

    // 1. Create root package.json with workspace glob patterns
    let root_pkg = r#"{
        "name": "root-monorepo",
        "workspaces": ["packages/*"]
    }"#;
    fs::write(root.join("package.json"), root_pkg).unwrap();

    // 2. Create sub-package directory and manifest (with valid quoted keys)
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

    // 3. Resolve workspace and verify npm packages are captured
    let lockfile_result = ResolverEngine::resolve_workspace(root);
    assert!(lockfile_result.is_ok(), "Resolver failed on valid NPM workspace structure");

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

    fs::create_dir_all(root.join("dmz-core")).unwrap();
    let core_cargo = r#"
        [package]
        name = "dmz-core"
        version = "0.1.0"
        edition = "2021"

        [dependencies]
        tokio = { workspace = true }
        serde = "1.0"
    "#;
    fs::write(root.join("dmz-core/Cargo.toml"), core_cargo).unwrap();

    let lockfile_result = ResolverEngine::resolve_workspace(root);
    assert!(lockfile_result.is_ok());

    let lockfile = lockfile_result.unwrap();
    assert!(lockfile.packages.contains_key("tokio:1.38"));
    assert!(lockfile.packages.contains_key("serde:1.0"));
}