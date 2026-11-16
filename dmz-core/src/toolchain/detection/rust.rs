use super::helpers::expand_workspace_member_dirs;
use crate::toolchain::types::ToolchainEcosystem;
use std::collections::HashSet;
use std::path::Path;

pub struct RustDetector;

impl RustDetector {
    pub fn detect(dir: &Path, ecosystems: &mut HashSet<ToolchainEcosystem>) {
        Self::detect_with_options(dir, ecosystems, false);
    }

    pub fn detect_with_options(
        dir: &Path,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
        include_dev: bool,
    ) {
        let cargo_dirs = crate::workspace::discover_tauri_cargo_dirs(dir);
        if !cargo_dirs.is_empty() || dir.join("Cargo.lock").is_file() {
            ecosystems.insert(ToolchainEcosystem::Rust);
            for cargo_dir in &cargo_dirs {
                Self::check_cargo_workspace_members(cargo_dir, ecosystems);
            }
            Self::check_cargo_build_tool_dependencies_with_options(dir, ecosystems, include_dev);
        }
    }

    /// Map Cargo build-script helper crates that execute external native build tools at compile time.
    pub fn map_cargo_build_tool_crate(
        crate_name: &str,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
    ) {
        match crate_name {
            "cmake" => {
                ecosystems.insert(ToolchainEcosystem::CMake);
                ecosystems.insert(ToolchainEcosystem::Cc);
            }
            "ninja" | "ninja-build" => {
                ecosystems.insert(ToolchainEcosystem::Ninja);
                ecosystems.insert(ToolchainEcosystem::Cc);
            }
            "pkg-config" => {
                ecosystems.insert(ToolchainEcosystem::PkgConfig);
            }
            _ => {}
        }
    }

    /// Inspect parsed `Cargo.toml` value for build-tool helper crates in dependency tables.
    pub fn check_cargo_toml_build_tool_deps(
        val: &toml::Value,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
    ) {
        Self::check_cargo_toml_build_tool_deps_with_options(val, ecosystems, false);
    }

    /// Inspect parsed `Cargo.toml` value for build-tool helper crates, gating `dev-dependencies` on `include_dev`.
    pub fn check_cargo_toml_build_tool_deps_with_options(
        val: &toml::Value,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
        include_dev: bool,
    ) {
        let sections: &[&str] = if include_dev {
            &["dependencies", "build-dependencies", "dev-dependencies"]
        } else {
            &["dependencies", "build-dependencies"]
        };
        for section in sections {
            if let Some(table) = val.get(*section).and_then(|v| v.as_table()) {
                for key in table.keys() {
                    Self::map_cargo_build_tool_crate(key, ecosystems);
                }
            }
        }
        if let Some(ws_deps) = val
            .get("workspace")
            .and_then(|w| w.get("dependencies"))
            .and_then(|d| d.as_table())
        {
            for key in ws_deps.keys() {
                Self::map_cargo_build_tool_crate(key, ecosystems);
            }
        }
    }

    /// Check declared `Cargo.toml` (root and declared workspace members) for Cargo build-tool helper crates.
    pub fn check_cargo_build_tool_dependencies(
        workspace_root: &Path,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
    ) {
        Self::check_cargo_build_tool_dependencies_with_options(workspace_root, ecosystems, false);
    }

    fn inspect_cargo_manifest_build_tools(
        cargo_dir: &Path,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
        include_dev: bool,
    ) {
        let cargo_toml = cargo_dir.join("Cargo.toml");
        if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
            if let Ok(val) = toml::from_str::<toml::Value>(&content) {
                Self::check_cargo_toml_build_tool_deps_with_options(&val, ecosystems, include_dev);
                if let Some(workspace) = val.get("workspace") {
                    if let Some(members) = workspace.get("members").and_then(|m| m.as_array()) {
                        for m in members {
                            if let Some(member_str) = m.as_str() {
                                for member_dir in
                                    expand_workspace_member_dirs(cargo_dir, member_str)
                                {
                                    let member_toml = member_dir.join("Cargo.toml");
                                    if let Ok(m_content) = std::fs::read_to_string(&member_toml) {
                                        if let Ok(m_val) = toml::from_str::<toml::Value>(&m_content)
                                        {
                                            Self::check_cargo_toml_build_tool_deps_with_options(
                                                &m_val,
                                                ecosystems,
                                                include_dev,
                                            );
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    /// Check declared `Cargo.toml` (root, nested Tauri manifests, and declared workspace members) for Cargo build-tool helper crates.
    pub fn check_cargo_build_tool_dependencies_with_options(
        workspace_root: &Path,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
        include_dev: bool,
    ) {
        let mut cargo_dirs = crate::workspace::discover_tauri_cargo_dirs(workspace_root);
        if workspace_root.join("Cargo.toml").is_file()
            && !cargo_dirs.iter().any(|d| d == workspace_root)
        {
            cargo_dirs.insert(0, workspace_root.to_path_buf());
        }
        for cargo_dir in cargo_dirs {
            Self::inspect_cargo_manifest_build_tools(&cargo_dir, ecosystems, include_dev);
        }
    }

    /// Check declared Cargo workspace members for explicit native build manifests.
    pub fn check_cargo_workspace_members(
        workspace_root: &Path,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
    ) {
        let cargo_toml = workspace_root.join("Cargo.toml");
        if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
            if let Ok(val) = toml::from_str::<toml::Value>(&content) {
                if let Some(workspace) = val.get("workspace") {
                    if let Some(members) = workspace.get("members").and_then(|m| m.as_array()) {
                        for m in members {
                            if let Some(member_str) = m.as_str() {
                                for member_dir in
                                    expand_workspace_member_dirs(workspace_root, member_str)
                                {
                                    super::helpers::check_member_native_manifests(
                                        &member_dir,
                                        ecosystems,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_rust_detector_discovers_nested_src_tauri_build_tools(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let tauri_dir = temp.path().join("frontend/src-tauri");
        fs::create_dir_all(&tauri_dir)?;
        fs::write(
            tauri_dir.join("Cargo.toml"),
            r#"
[package]
name = "tauri-app"
version = "0.1.0"
edition = "2021"

[build-dependencies]
cmake = "0.1"
pkg-config = "0.3"

[dev-dependencies]
ninja-build = "0.1"
"#,
        )?;

        let mut eco_prod = HashSet::new();
        RustDetector::detect_with_options(temp.path(), &mut eco_prod, false);
        assert!(eco_prod.contains(&ToolchainEcosystem::Rust));
        assert!(eco_prod.contains(&ToolchainEcosystem::CMake));
        assert!(eco_prod.contains(&ToolchainEcosystem::Cc));
        assert!(eco_prod.contains(&ToolchainEcosystem::PkgConfig));
        assert!(!eco_prod.contains(&ToolchainEcosystem::Ninja));

        let mut eco_dev = HashSet::new();
        RustDetector::detect_with_options(temp.path(), &mut eco_dev, true);
        assert!(eco_dev.contains(&ToolchainEcosystem::Ninja));
        assert!(super::super::native::has_explicit_ninja_manifest_with_options(temp.path(), true));
        Ok(())
    }
}
