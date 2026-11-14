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
        if dir.join("Cargo.toml").is_file()
            || dir.join("Cargo.lock").is_file()
            || dir.join("src-tauri/Cargo.toml").is_file()
        {
            ecosystems.insert(ToolchainEcosystem::Rust);
            Self::check_cargo_workspace_members(dir, ecosystems);
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

    /// Check declared `Cargo.toml` (root and declared workspace members) for Cargo build-tool helper crates.
    pub fn check_cargo_build_tool_dependencies_with_options(
        workspace_root: &Path,
        ecosystems: &mut HashSet<ToolchainEcosystem>,
        include_dev: bool,
    ) {
        let cargo_toml = workspace_root.join("Cargo.toml");
        if let Ok(content) = std::fs::read_to_string(&cargo_toml) {
            if let Ok(val) = toml::from_str::<toml::Value>(&content) {
                Self::check_cargo_toml_build_tool_deps_with_options(&val, ecosystems, include_dev);
                if let Some(workspace) = val.get("workspace") {
                    if let Some(members) = workspace.get("members").and_then(|m| m.as_array()) {
                        for m in members {
                            if let Some(member_str) = m.as_str() {
                                for member_dir in
                                    expand_workspace_member_dirs(workspace_root, member_str)
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
