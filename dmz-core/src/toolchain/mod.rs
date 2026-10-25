<<<<<<< Updated upstream
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use tar::Builder as TarBuilder;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolchainEcosystem {
    Rust,
    Node,
    Python,
    Go,
    Php,
    Ruby,
}

impl ToolchainEcosystem {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::Node => "node",
            Self::Python => "python",
            Self::Go => "go",
            Self::Php => "php",
            Self::Ruby => "ruby",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolchainManifestEntry {
    pub ecosystem: String,
    pub name: String,
    pub version: String,
    pub relative_path: String,
    pub binaries: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct DiscoveredToolchain {
    pub ecosystem: ToolchainEcosystem,
    pub name: String,
    pub version: String,
    /// (source_file_path, archive_relative_path)
    pub files: Vec<(PathBuf, PathBuf)>,
    pub binaries: Vec<String>,
}

pub struct ToolchainProvider;

impl ToolchainProvider {
    /// Discovers active host toolchains for the ecosystems present in the workspace.
    pub fn discover_active_toolchains(
        _workspace_root: &Path,
        active_ecosystems: &[&str],
    ) -> Vec<DiscoveredToolchain> {
        let mut toolchains = Vec::new();

        for eco_name in active_ecosystems {
            match *eco_name {
                "rust" => {
                    if let Some(tc) = Self::discover_rust() {
                        toolchains.push(tc);
                    }
                }
                "node" | "npm" | "pnpm" | "yarn" => {
                    // Only discover node once
                    if !toolchains
                        .iter()
                        .any(|t| t.ecosystem == ToolchainEcosystem::Node)
                    {
                        if let Some(tc) = Self::discover_node() {
                            toolchains.push(tc);
                        }
                    }
                }
                "python" => {
                    if let Some(tc) = Self::discover_python() {
                        toolchains.push(tc);
                    }
                }
                "go" => {
                    if let Some(tc) = Self::discover_go() {
                        toolchains.push(tc);
                    }
                }
                "php" => {
                    if let Some(tc) = Self::discover_php() {
                        toolchains.push(tc);
                    }
                }
                "ruby" => {
                    if let Some(tc) = Self::discover_ruby() {
                        toolchains.push(tc);
                    }
                }
                _ => {}
            }
        }

        toolchains
    }

    /// Discovers the active Rust toolchain (cargo, rustc, and standard libraries).
    pub fn discover_rust() -> Option<DiscoveredToolchain> {
        let rustc_path = which::which("rustc").ok()?;
        let cargo_path = which::which("cargo").ok()?;

        let version = Command::new(&rustc_path)
            .arg("--version")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "rustc-unknown".to_string());

        let sysroot = Command::new(&rustc_path)
            .arg("--print")
            .arg("sysroot")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| PathBuf::from(s.trim()))?;

        info!(sysroot = ?sysroot, version = %version, "Discovered host Rust toolchain sysroot");

        let mut files = Vec::new();
        let target_prefix = Path::new("toolchains/rust");

        // 1. Binaries (cargo, rustc)
        let sysroot_rustc = sysroot.join("bin/rustc");
        let sysroot_cargo = sysroot.join("bin/cargo");

        let actual_rustc = if sysroot_rustc.exists() {
            sysroot_rustc
        } else {
            rustc_path.clone()
        };
        let actual_cargo = if sysroot_cargo.exists() {
            sysroot_cargo
        } else {
            cargo_path.clone()
        };

        if actual_rustc.exists() {
            files.push((actual_rustc, target_prefix.join("bin/rustc")));
        }
        if actual_cargo.exists() {
            files.push((actual_cargo, target_prefix.join("bin/cargo")));
        }

        // 2. Sysroot libraries (needed for rustc and libstd linking)
        let sysroot_lib = sysroot.join("lib");
        if sysroot_lib.exists() {
            Self::collect_files_recursive(
                &sysroot_lib,
                &target_prefix.join("lib"),
                &mut files,
                &["doc", "man"],
            );
        }

        Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Rust,
            name: "rust".to_string(),
            version,
            files,
            binaries: vec!["rustc".into(), "cargo".into()],
        })
    }

    /// Discovers Node.js and package manager binaries.
    pub fn discover_node() -> Option<DiscoveredToolchain> {
        let node_path = which::which("node").ok()?;
        let version = Command::new(&node_path)
            .arg("--version")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "node-unknown".to_string());

        let mut files = Vec::new();
        let mut binaries = vec!["node".into()];
        let target_prefix = Path::new("toolchains/node");

        // Canonicalize node binary if symlink
        let canonical_node = fs::canonicalize(&node_path).unwrap_or(node_path.clone());
        files.push((canonical_node, target_prefix.join("bin/node")));

        // Optional package manager CLIs (npm, pnpm, yarn)
        for pm in &["npm", "pnpm", "yarn", "corepack"] {
            if let Ok(pm_path) = which::which(pm) {
                if let Ok(canon_pm) = fs::canonicalize(&pm_path) {
                    files.push((canon_pm, target_prefix.join(format!("bin/{}", pm))));
                    binaries.push(pm.to_string());
                }
            }
        }

        // If node binary is part of a standard prefix (e.g. /usr/local or ~/.nvm/...), gather lib/node_modules if present
        if let Some(bin_dir) = node_path.parent() {
            if let Some(prefix) = bin_dir.parent() {
                let node_modules_dir = prefix.join("lib/node_modules");
                if node_modules_dir.exists() {
                    Self::collect_files_recursive(
                        &node_modules_dir,
                        &target_prefix.join("lib/node_modules"),
                        &mut files,
                        &["doc", "man", "test", "tests"],
                    );
                }
            }
        }

        info!(version = %version, "Discovered host Node.js toolchain");

        Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Node,
            name: "node".to_string(),
            version,
            files,
            binaries,
        })
    }

    /// Discovers the active Python 3 runtime and standard library.
    pub fn discover_python() -> Option<DiscoveredToolchain> {
        let python_path = which::which("python3")
            .or_else(|_| which::which("python"))
            .ok()?;
        let version = Command::new(&python_path)
            .arg("--version")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "python-unknown".to_string());

        let base_prefix = Command::new(&python_path)
            .args(["-c", "import sys; print(sys.base_prefix)"])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| PathBuf::from(s.trim()))?;

        let mut files = Vec::new();
        let target_prefix = Path::new("toolchains/python");

        let canonical_python = fs::canonicalize(&python_path).unwrap_or(python_path.clone());
        files.push((canonical_python, target_prefix.join("bin/python3")));

        // Look for pip
        if let Ok(pip_path) = which::which("pip3").or_else(|_| which::which("pip")) {
            if let Ok(canon_pip) = fs::canonicalize(&pip_path) {
                files.push((canon_pip, target_prefix.join("bin/pip3")));
            }
        }

        // Standard library in base_prefix/lib
        let lib_dir = base_prefix.join("lib");
        if lib_dir.exists() {
            Self::collect_files_recursive(
                &lib_dir,
                &target_prefix.join("lib"),
                &mut files,
                &["test", "tests", "idlelib", "tkinter", "turtledemo"],
            );
        }

        info!(version = %version, base_prefix = ?base_prefix, "Discovered host Python toolchain");

        Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Python,
            name: "python".to_string(),
            version,
            files,
            binaries: vec!["python3".into(), "pip3".into()],
        })
    }

    /// Discovers Go toolchain using go env GOROOT.
    pub fn discover_go() -> Option<DiscoveredToolchain> {
        let go_path = which::which("go").ok()?;
        let version = Command::new(&go_path)
            .arg("version")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "go-unknown".to_string());

        let goroot = Command::new(&go_path)
            .args(["env", "GOROOT"])
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| PathBuf::from(s.trim()))?;

        let mut files = Vec::new();
        let target_prefix = Path::new("toolchains/go");

        // Binaries (go, gofmt)
        let go_bin = goroot.join("bin/go");
        let gofmt_bin = goroot.join("bin/gofmt");
        if go_bin.exists() {
            files.push((go_bin, target_prefix.join("bin/go")));
        } else {
            files.push((go_path.clone(), target_prefix.join("bin/go")));
        }
        if gofmt_bin.exists() {
            files.push((gofmt_bin, target_prefix.join("bin/gofmt")));
        }

        // Compiler toolchain binaries and packages in pkg/tool and src
        let pkg_tool = goroot.join("pkg/tool");
        if pkg_tool.exists() {
            Self::collect_files_recursive(
                &pkg_tool,
                &target_prefix.join("pkg/tool"),
                &mut files,
                &[],
            );
        }
        let src_dir = goroot.join("src");
        if src_dir.exists() {
            Self::collect_files_recursive(
                &src_dir,
                &target_prefix.join("src"),
                &mut files,
                &["cmd/dist"],
            );
        }

        info!(version = %version, goroot = ?goroot, "Discovered host Go toolchain");

        Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Go,
            name: "go".to_string(),
            version,
            files,
            binaries: vec!["go".into(), "gofmt".into()],
        })
    }

    /// Discovers PHP runtime and Composer CLI.
    pub fn discover_php() -> Option<DiscoveredToolchain> {
        let php_path = which::which("php").ok()?;
        let version = Command::new(&php_path)
            .arg("-v")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .and_then(|s| s.lines().next().map(|l| l.to_string()))
            .unwrap_or_else(|| "php-unknown".to_string());

        let mut files = Vec::new();
        let mut binaries = vec!["php".into()];
        let target_prefix = Path::new("toolchains/php");

        let canonical_php = fs::canonicalize(&php_path).unwrap_or(php_path);
        files.push((canonical_php, target_prefix.join("bin/php")));

        if let Ok(composer_path) = which::which("composer") {
            let canon_composer = fs::canonicalize(&composer_path).unwrap_or(composer_path);
            files.push((canon_composer, target_prefix.join("bin/composer")));
            binaries.push("composer".into());
        }

        Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Php,
            name: "php".to_string(),
            version,
            files,
            binaries,
        })
    }

    /// Discovers Ruby runtime and Bundler CLI.
    pub fn discover_ruby() -> Option<DiscoveredToolchain> {
        let ruby_path = which::which("ruby").ok()?;
        let version = Command::new(&ruby_path)
            .arg("-v")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| "ruby-unknown".to_string());

        let mut files = Vec::new();
        let mut binaries = vec!["ruby".into()];
        let target_prefix = Path::new("toolchains/ruby");

        let canonical_ruby = fs::canonicalize(&ruby_path).unwrap_or(ruby_path);
        files.push((canonical_ruby, target_prefix.join("bin/ruby")));

        if let Ok(bundle_path) = which::which("bundle").or_else(|_| which::which("bundler")) {
            let canon_bundle = fs::canonicalize(&bundle_path).unwrap_or(bundle_path);
            files.push((canon_bundle, target_prefix.join("bin/bundle")));
            binaries.push("bundle".into());
        }

        Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Ruby,
            name: "ruby".to_string(),
            version,
            files,
            binaries,
        })
    }

    /// Helper to recursively collect files while skipping excluded subdirectories.
    fn collect_files_recursive(
        source_dir: &Path,
        dest_prefix: &Path,
        collected: &mut Vec<(PathBuf, PathBuf)>,
        exclude_subdirs: &[&str],
    ) {
        let entries = match fs::read_dir(source_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let file_name = entry.file_name();
            let name_str = file_name.to_string_lossy();

            if exclude_subdirs.iter().any(|&ex| name_str == ex) {
                continue;
            }

            let rel_dest = dest_prefix.join(&file_name);

            if path.is_dir() {
                Self::collect_files_recursive(&path, &rel_dest, collected, exclude_subdirs);
            } else if path.is_file() {
                collected.push((path, rel_dest));
            }
        }
    }
}

pub struct ToolchainPackager;

impl ToolchainPackager {
    /// Appends all files for the discovered toolchains into the tar archive builder.
    pub fn package_toolchains<W: Write>(
        tar_builder: &mut TarBuilder<W>,
        toolchains: &[DiscoveredToolchain],
    ) -> Result<Vec<ToolchainManifestEntry>> {
        let mut manifest_entries = Vec::new();

        for tc in toolchains {
            info!(
                ecosystem = %tc.ecosystem.as_str(),
                version = %tc.version,
                file_count = tc.files.len(),
                "Packaging toolchain into bundle"
            );

            for (src_path, dest_path) in &tc.files {
                if !src_path.exists() {
                    continue;
                }

                // If symlink, resolve or append directly
                let metadata = match fs::metadata(src_path) {
                    Ok(m) => m,
                    Err(_) => continue,
                };

                if metadata.is_file() {
                    let mut file = match fs::File::open(src_path) {
                        Ok(f) => f,
                        Err(e) => {
                            debug!("Skipping unreadable toolchain file {:?}: {}", src_path, e);
                            continue;
                        }
                    };

                    let mut header = tar::Header::new_gnu();
                    header.set_size(metadata.len());
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let mode = metadata.permissions().mode();
                        header.set_mode(mode);
                    }
                    #[cfg(not(unix))]
                    {
                        header.set_mode(0o755);
                    }
                    header.set_cksum();

                    if let Err(e) = tar_builder.append_data(&mut header, dest_path, &mut file) {
                        warn!("Failed to pack toolchain file {:?}: {}", dest_path, e);
                    }
                }
            }

            manifest_entries.push(ToolchainManifestEntry {
                ecosystem: tc.ecosystem.as_str().to_string(),
                name: tc.name.clone(),
                version: tc.version.clone(),
                relative_path: format!("toolchains/{}", tc.ecosystem.as_str()),
                binaries: tc.binaries.clone(),
            });
        }

        Ok(manifest_entries)
    }

    /// Sets up imported toolchains in `<workspace>/.dmz/toolchains`:
    /// 1. Ensures all binaries have executable mode (`0o755`).
    /// 2. Clears macOS quarantine attributes (`xattr -cr`).
    /// 3. Generates `.dmz/env.sh` and `.dmz/env.bat` activator scripts.
    pub fn setup_imported_toolchains(workspace_root: &Path) -> Result<()> {
        let toolchains_dir = workspace_root.join(".dmz/toolchains");
        if !toolchains_dir.exists() {
            return Ok(());
        }

        info!(path = ?toolchains_dir, "Configuring imported toolchain binaries and permissions");

        // 1. Recursive executable permissions for all binaries
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fn make_exec_recursive(dir: &Path) {
                if let Ok(entries) = fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.is_dir() {
                            make_exec_recursive(&p);
                        } else if p.is_file() {
                            // Check if parent directory is "bin" or extension is empty / executable
                            let in_bin = p
                                .parent()
                                .and_then(|parent| parent.file_name())
                                .is_some_and(|name| name == "bin");
                            if in_bin {
                                let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o755));
                            }
                        }
                    }
                }
            }
            make_exec_recursive(&toolchains_dir);
        }

        // 2. Clear macOS quarantine attributes
        #[cfg(target_os = "macos")]
        {
            let _ = Command::new("xattr")
                .args(["-cr", &toolchains_dir.to_string_lossy()])
                .output();
        }

        // 3. Generate .dmz/env.sh
        let env_sh_path = workspace_root.join(".dmz/env.sh");
        let env_sh_content = r#"#!/usr/bin/env sh
# ==============================================================================
# DMZ Auto-Generated Toolchain Environment Activator
# Source this file to activate all self-contained toolchains in your current shell:
#   source .dmz/env.sh
# ==============================================================================

DMZ_WS_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DMZ_TC_DIR="$DMZ_WS_ROOT/.dmz/toolchains"

if [ -d "$DMZ_TC_DIR" ]; then
    for bin_dir in "$DMZ_TC_DIR"/*/bin "$DMZ_TC_DIR"/*; do
        if [ -d "$bin_dir" ]; then
            case ":$PATH:" in
                *":$bin_dir:"*) ;;
                *) export PATH="$bin_dir:$PATH" ;;
            esac
        fi
    done

    # Ecosystem-specific home configurations
    if [ -d "$DMZ_TC_DIR/rust" ]; then
        export RUSTUP_HOME="$DMZ_TC_DIR/rust"
        export CARGO_HOME="${CARGO_HOME:-$DMZ_WS_ROOT/.cargo}"
    fi

    if [ -d "$DMZ_TC_DIR/go" ]; then
        export GOROOT="$DMZ_TC_DIR/go"
    fi

    if [ -d "$DMZ_TC_DIR/python" ]; then
        export PYTHONHOME="$DMZ_TC_DIR/python"
    fi

    echo "DMZ: Bundled toolchains activated in current shell environment."
fi
"#;
        let _ = fs::write(&env_sh_path, env_sh_content);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&env_sh_path, fs::Permissions::from_mode(0o755));
        }

        // 4. Generate .dmz/env.bat for Windows
        let env_bat_path = workspace_root.join(".dmz/env.bat");
        let env_bat_content = r#"@echo off
set "DMZ_WS_ROOT=%~dp0.."
set "DMZ_TC_DIR=%DMZ_WS_ROOT%\.dmz\toolchains"

for /d %%D in ("%DMZ_TC_DIR%\*") do (
    if exist "%%D\bin" (
        set "PATH=%%D\bin;%PATH%"
    )
)
echo DMZ: Bundled toolchains activated in current command prompt.
"#;
        let _ = fs::write(env_bat_path, env_bat_content);

        Ok(())
    }

    /// Appends discovered toolchain bin directories to an environment map (for dmz shell / dmz sandbox).
    pub fn inject_toolchain_paths(workspace_root: &Path, envs: &mut HashMap<String, String>) {
        let toolchains_dir = workspace_root.join(".dmz/toolchains");
        if !toolchains_dir.exists() {
            return;
        }

        let mut additional_paths = Vec::new();

        if let Ok(entries) = fs::read_dir(&toolchains_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                if p.is_dir() {
                    let bin_path = p.join("bin");
                    if bin_path.exists() {
                        additional_paths.push(bin_path.to_string_lossy().to_string());
                    }
                }
            }
        }

        if !additional_paths.is_empty() {
            let current_path = envs.get("PATH").cloned().unwrap_or_else(|| {
                std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())
            });
            let combined = format!("{}:{}", additional_paths.join(":"), current_path);
            envs.insert("PATH".into(), combined);
        }

        if toolchains_dir.join("rust").exists() {
            envs.insert(
                "RUSTUP_HOME".into(),
                toolchains_dir.join("rust").to_string_lossy().to_string(),
            );
        }
        if toolchains_dir.join("go").exists() {
            envs.insert(
                "GOROOT".into(),
                toolchains_dir.join("go").to_string_lossy().to_string(),
            );
        }
        if toolchains_dir.join("python").exists() {
            envs.insert(
                "PYTHONHOME".into(),
                toolchains_dir.join("python").to_string_lossy().to_string(),
            );
        }
=======
//! Modular, SOLID-compliant toolchain discovery, packaging, and execution environment engine.
//!
//! # Architecture & SOLID Principles
//!
//! - **Single Responsibility Principle (SRP)**:
//!   - [`types`]: Canonical domain models (`ToolchainEcosystem`, `ToolchainManifestEntry`, `DiscoveredToolchain`).
//!   - [`discovery`]: Ecosystem-specific standalone detection heuristics adhering to [`discovery::ToolchainDiscoverer`].
//!   - [`packager`]: Archive streaming and filesystem permission encoding.
//!   - [`setup`]: Post-unpacking activation, permissions, and script generation.
//!   - [`env`]: Process environment variable injection for sandboxes and shells.
//!
//! - **Open/Closed Principle (OCP)** & **Dependency Inversion Principle (DIP)**:
//!   - The [`discovery::ToolchainDiscoverer`] trait allows adding or substituting language runtimes
//!     without modifying core packaging or provider orchestration logic.
//!
//! - **Interface Segregation Principle (ISP)** & **Liskov Substitution Principle (LSP)**:
//!   - Discoverers, packagers, and environment injectors are decoupled and can be used or tested independently.

pub mod discovery;
pub mod env;
pub mod packager;
pub mod setup;
pub mod types;

pub use discovery::{
    emit_non_relocatable_notice, is_non_standalone_path, DiscoveryRegistry, GoDiscoverer,
    NodeDiscoverer, PhpDiscoverer, PythonDiscoverer, RubyDiscoverer, RustDiscoverer,
    ToolchainDiscoverer,
};
pub use env::ToolchainEnv;
pub use packager::ToolchainArchivePackager;
pub use setup::ToolchainSetup;
pub use types::{DiscoveredToolchain, ToolchainEcosystem, ToolchainManifestEntry};

use std::collections::HashMap;
use std::path::Path;
use tar::Builder as TarBuilder;

/// Facade provider maintaining backward compatibility for toolchain discovery operations.
pub struct ToolchainProvider;

impl ToolchainProvider {
    /// Detects whether a path is non-standalone / non-relocatable (e.g. managed by system package managers
    /// that dynamically link against machine-specific cellar/system libraries).
    #[inline]
    pub fn is_non_standalone_path(path: &Path) -> bool {
        discovery::is_non_standalone_path(path)
    }

    /// Emits standard agnostic guidance when a non-relocatable toolchain is encountered.
    #[inline]
    pub fn emit_non_relocatable_notice(path: &Path) {
        discovery::emit_non_relocatable_notice(path);
    }

    /// Discovers all available standalone host toolchains using the default registry.
    pub fn discover_all() -> Vec<DiscoveredToolchain> {
        DiscoveryRegistry::with_default_discoverers().discover_all()
    }

    /// Discovers the host Rust toolchain, prioritizing official standalone Rustup distributions.
    #[inline]
    pub fn discover_rust() -> Result<Option<DiscoveredToolchain>, String> {
        RustDiscoverer.discover()
    }

    /// Discovers host Node.js runtime and companion package managers (npm, pnpm, yarn).
    #[inline]
    pub fn discover_node() -> Result<Option<DiscoveredToolchain>, String> {
        NodeDiscoverer.discover()
    }

    /// Discovers host Python runtime (python3, pip), prioritizing standalone environments.
    #[inline]
    pub fn discover_python() -> Result<Option<DiscoveredToolchain>, String> {
        PythonDiscoverer.discover()
    }

    /// Discovers host Go toolchain ($GOROOT, go), prioritizing official standalone GOROOT.
    #[inline]
    pub fn discover_go() -> Result<Option<DiscoveredToolchain>, String> {
        GoDiscoverer.discover()
    }

    /// Discovers host PHP runtime and Composer package manager.
    #[inline]
    pub fn discover_php() -> Result<Option<DiscoveredToolchain>, String> {
        PhpDiscoverer.discover()
    }

    /// Discovers host Ruby runtime and Bundler package manager.
    #[inline]
    pub fn discover_ruby() -> Result<Option<DiscoveredToolchain>, String> {
        RubyDiscoverer.discover()
    }
}

/// Facade packager maintaining backward compatibility for toolchain packaging and setup.
pub struct ToolchainPackager;

impl ToolchainPackager {
    /// Packages all discovered toolchains into the target tar archive under `toolchains/<eco>/`.
    pub fn package_toolchains<W: std::io::Write>(
        _workspace_root: &Path,
        tar_builder: &mut TarBuilder<W>,
    ) -> Result<Vec<ToolchainManifestEntry>, String> {
        let toolchains = ToolchainProvider::discover_all();
        ToolchainArchivePackager::package(tar_builder, &toolchains)
    }

    /// Sets up imported toolchain directory: execution permissions, quarantine clearance, and activator scripts.
    #[inline]
    pub fn setup_imported_toolchains(workspace_root: &Path) -> Result<(), String> {
        ToolchainSetup::setup_imported_toolchains(workspace_root)
    }

    /// Injects bundled toolchains into an environment variable map (for sandbox/shell executions).
    #[inline]
    pub fn inject_toolchain_paths(workspace_root: &Path, env_map: &mut HashMap<String, String>) {
        ToolchainEnv::inject_toolchain_paths(workspace_root, env_map);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct MockDiscoverer {
        ecosystem: ToolchainEcosystem,
        mock_toolchain: Option<DiscoveredToolchain>,
    }

    impl ToolchainDiscoverer for MockDiscoverer {
        fn ecosystem(&self) -> ToolchainEcosystem {
            self.ecosystem
        }

        fn discover(&self) -> Result<Option<DiscoveredToolchain>, String> {
            Ok(self.mock_toolchain.clone())
        }
    }

    #[test]
    fn test_registry_polymorphism_and_open_closed_principle() {
        let mut registry = DiscoveryRegistry::empty();

        let mock_tc = DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Rust,
            version: "rustc 1.99.0-mock".into(),
            root_dir: PathBuf::from("/mock/sysroot"),
            bin_files: vec![(PathBuf::from("/mock/bin/rustc"), "bin/rustc".into())],
            lib_files: vec![],
        };

        registry.register(Box::new(MockDiscoverer {
            ecosystem: ToolchainEcosystem::Rust,
            mock_toolchain: Some(mock_tc),
        }));

        let discovered = registry.discover_all();
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].ecosystem, ToolchainEcosystem::Rust);
        assert_eq!(discovered[0].version, "rustc 1.99.0-mock");
    }

    #[test]
    fn test_is_non_standalone_path_heuristics() {
        assert!(is_non_standalone_path(Path::new(
            "/opt/homebrew/Cellar/rust/1.96.0/bin/cargo"
        )));
        assert!(is_non_standalone_path(Path::new(
            "/usr/local/Cellar/node/20.0/bin/node"
        )));
        assert!(is_non_standalone_path(Path::new(
            "/nix/store/abc-rust/bin/rustc"
        )));

        assert!(!is_non_standalone_path(Path::new(
            "/Users/user/.rustup/toolchains/stable-aarch64-apple-darwin/bin/cargo"
        )));
        assert!(!is_non_standalone_path(Path::new(
            "/Users/user/.nvm/versions/node/v20.0.0/bin/node"
        )));
    }

    #[test]
    fn test_ecosystem_representations() {
        assert_eq!(ToolchainEcosystem::Rust.as_str(), "rust");
        assert_eq!(ToolchainEcosystem::Node.as_str(), "node");
        assert_eq!(ToolchainEcosystem::Python.as_str(), "python");
        assert_eq!(ToolchainEcosystem::Go.as_str(), "go");
        assert_eq!(ToolchainEcosystem::Php.as_str(), "php");
        assert_eq!(ToolchainEcosystem::Ruby.as_str(), "ruby");
        assert_eq!(ToolchainEcosystem::all().len(), 6);
>>>>>>> Stashed changes
    }
}
