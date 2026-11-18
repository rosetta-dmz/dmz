use super::*;
use std::error::Error;
use tempfile::tempdir;

#[test]
fn test_discover_workspace_manifest_in_current_dir() -> Result<(), Box<dyn Error>> {
    let dir = tempdir()?;
    let manifest = dir.path().join("dmz-workspace.toml");
    fs::write(
        &manifest,
        "name = \"test-ws\"\n[[members]]\nid = \"m1\"\npath = \".\"\nrole = \"primary\"\n",
    )?;

    let discovered = discover_workspace_manifest(dir.path());
    assert!(discovered.is_some());
    let (cfg, path) = discovered.ok_or("expected discovered manifest")?;
    assert_eq!(cfg.name, "test-ws");
    assert_eq!(path.canonicalize()?, manifest.canonicalize()?);
    Ok(())
}

#[test]
fn test_discover_workspace_manifest_climbs_parents() -> Result<(), Box<dyn Error>> {
    let root = tempdir()?;
    let manifest = root.path().join("dmz-workspace.toml");
    fs::write(
        &manifest,
        "name = \"parent-ws\"\n[[members]]\nid = \"root\"\npath = \".\"\nrole = \"primary\"\n",
    )?;

    let deep_nested = root.path().join("a").join("b").join("c");
    fs::create_dir_all(&deep_nested)?;

    let discovered = discover_workspace_manifest(&deep_nested);
    assert!(discovered.is_some());
    let (cfg, path) = discovered.ok_or("expected discovered manifest from deep child")?;
    assert_eq!(cfg.name, "parent-ws");
    assert_eq!(path.canonicalize()?, manifest.canonicalize()?);
    Ok(())
}

#[test]
fn test_discover_workspace_manifest_from_file_path() -> Result<(), Box<dyn Error>> {
    let root = tempdir()?;
    let manifest = root.path().join("dmz-workspace.toml");
    fs::write(
        &manifest,
        "name = \"file-ws\"\n[[members]]\nid = \"root\"\npath = \".\"\nrole = \"primary\"\n",
    )?;

    let nested_file = root.path().join("src").join("main.rs");
    fs::create_dir_all(nested_file.parent().ok_or("missing parent")?)?;
    fs::write(&nested_file, "fn main() {}")?;

    let discovered = discover_workspace_manifest(&nested_file);
    assert!(discovered.is_some());
    let (cfg, path) = discovered.ok_or("expected discovered manifest starting from file")?;
    assert_eq!(cfg.name, "file-ws");
    assert_eq!(path.canonicalize()?, manifest.canonicalize()?);
    Ok(())
}

#[test]
fn test_discover_workspace_manifest_not_found() -> Result<(), Box<dyn Error>> {
    let dir = tempdir()?;
    assert!(discover_workspace_manifest(dir.path()).is_none());
    assert!(discover_workspace_manifest_checked(dir.path())?.is_none());
    Ok(())
}

#[test]
fn test_discover_workspace_manifest_checked_surfaces_parse_error() -> Result<(), Box<dyn Error>> {
    let dir = tempdir()?;
    let manifest = dir.path().join("dmz-workspace.toml");
    fs::write(&manifest, "name = [invalid toml syntax\n")?;

    let err = discover_workspace_manifest_checked(dir.path())
        .err()
        .ok_or("expected parse error for malformed dmz-workspace.toml")?;
    assert!(err.to_string().contains("Invalid workspace manifest"));
    Ok(())
}

#[test]
fn test_scoped_discovery_ignores_unrelated_ancestor_umbrella() -> Result<(), Box<dyn Error>> {
    let root = tempdir()?;
    let sibling_sidecar = root.path().join("sidecar");
    let standalone_child = root.path().join("standalone-child");
    let declared_member = root.path().join("declared-member");
    fs::create_dir_all(&sibling_sidecar)?;
    fs::create_dir_all(&standalone_child)?;
    fs::create_dir_all(&declared_member)?;

    fs::write(
        root.path().join("dmz-workspace.toml"),
        r#"name = "parent-umbrella"
[[members]]
id = "primary"
path = "."
role = "primary"

[[members]]
id = "declared-member"
path = "declared-member"
role = "sidecar"
"#,
    )?;

    assert!(discover_workspace_manifest_scoped(&standalone_child).is_none());
    assert!(discover_workspace_manifest_scoped_checked(&standalone_child)?.is_none());

    let found = discover_workspace_manifest_scoped(&declared_member)
        .ok_or("expected declared member to resolve to parent umbrella")?;
    assert_eq!(found.0.name, "parent-umbrella");

    let direct = discover_workspace_manifest_scoped(root.path())
        .ok_or("expected root manifest to be discovered")?;
    assert_eq!(direct.0.name, "parent-umbrella");
    Ok(())
}

#[test]
fn test_discover_nested_node_and_tauri_roots_skips_symlinks() -> Result<(), Box<dyn Error>> {
    let tmp = tempdir()?;
    let outside = tmp.path().join("outside-pkg");
    fs::create_dir_all(outside.join("src-tauri"))?;
    fs::write(outside.join("package.json"), r#"{"name":"outside"}"#)?;
    fs::write(
        outside.join("src-tauri/Cargo.toml"),
        "[package]\nname=\"out\"\n",
    )?;

    let member = tmp.path().join("member");
    let frontend = member.join("frontend");
    fs::create_dir_all(frontend.join("src-tauri"))?;
    fs::create_dir_all(member.join("node_modules/ignored-pkg"))?;
    fs::write(frontend.join("package.json"), r#"{"name":"frontend"}"#)?;
    fs::write(
        frontend.join("src-tauri/Cargo.toml"),
        "[package]\nname=\"fe\"\n",
    )?;
    fs::write(
        member.join("node_modules/ignored-pkg/package.json"),
        r#"{"name":"ignored"}"#,
    )?;

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, member.join("escape-link"))?;
        std::os::unix::fs::symlink(&member, frontend.join("loop-link"))?;
    }

    let node_roots = discover_node_project_roots(&member);
    assert_eq!(node_roots, vec![frontend.clone()]);

    let cargo_dirs = discover_tauri_cargo_dirs(&member);
    assert_eq!(cargo_dirs, vec![frontend.join("src-tauri")]);
    Ok(())
}

#[test]
fn test_discover_node_roots_includes_root_plus_undeclared_frontend_and_excludes_monorepo_globs(
) -> Result<(), Box<dyn Error>> {
    let tmp = tempdir()?;
    let root = tmp.path();
    let ws_pkg = root.join("packages/ui");
    let standalone_fe = root.join("frontend");
    fs::create_dir_all(&ws_pkg)?;
    fs::create_dir_all(&standalone_fe)?;

    fs::write(
        root.join("package.json"),
        r#"{"name":"wrapper","workspaces":["packages/*"]}"#,
    )?;
    fs::write(ws_pkg.join("package.json"), r#"{"name":"ui"}"#)?;
    fs::write(standalone_fe.join("package.json"), r#"{"name":"frontend"}"#)?;

    let roots = discover_node_project_roots(root);
    assert!(roots.contains(&root.to_path_buf()));
    assert!(roots.contains(&standalone_fe));
    assert!(!roots.contains(&ws_pkg));
    Ok(())
}

#[test]
fn test_discover_roots_handles_large_sibling_trees_and_excludes_declared_members(
) -> Result<(), Box<dyn Error>> {
    let tmp = tempdir()?;
    let root = tmp.path();

    let assets = root.join("assets");
    fs::create_dir_all(&assets)?;
    for idx in 0..650 {
        fs::write(assets.join(format!("file_{:04}.txt", idx)), "data")?;
    }

    let frontend = root.join("frontend");
    let tauri = frontend.join("src-tauri");
    fs::create_dir_all(&tauri)?;
    fs::write(frontend.join("package.json"), r#"{"name":"frontend"}"#)?;
    fs::write(
        tauri.join("Cargo.toml"),
        "[package]\nname=\"fe\"\nversion=\"0.1.0\"\n",
    )?;

    let sidecar = root.join("codebase-ui");
    let sidecar_fe = sidecar.join("frontend");
    let sidecar_tauri = sidecar_fe.join("src-tauri");
    fs::create_dir_all(&sidecar_tauri)?;
    fs::write(sidecar_fe.join("package.json"), r#"{"name":"sc-fe"}"#)?;
    fs::write(
        sidecar_tauri.join("Cargo.toml"),
        "[package]\nname=\"sc-tauri\"\nversion=\"0.1.0\"\n",
    )?;

    let all_cargo = discover_tauri_cargo_dirs(root);
    assert!(all_cargo.contains(&tauri));
    assert!(all_cargo.contains(&sidecar_tauri));

    let primary_cargo = discover_tauri_cargo_dirs_excluding(root, std::slice::from_ref(&sidecar));
    assert_eq!(primary_cargo, vec![tauri]);

    let primary_node = discover_node_project_roots_excluding(root, std::slice::from_ref(&sidecar));
    assert_eq!(primary_node, vec![frontend]);

    // Excluding an ancestor (e.g. `.`) must never block scanning inside child `sidecar`
    let sidecar_cargo = discover_tauri_cargo_dirs_excluding(&sidecar, &[root.to_path_buf()]);
    assert_eq!(sidecar_cargo, vec![sidecar_tauri]);
    Ok(())
}
