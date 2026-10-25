use std::fs;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::Command;

/// Target machine activation and setup engine for imported toolchains.
pub struct ToolchainSetup;

impl ToolchainSetup {
    /// Sets up imported toolchain directory: execution permissions, quarantine clearance, and activator scripts.
    pub fn setup_imported_toolchains(workspace_root: &Path) -> Result<(), String> {
        let toolchains_dir = workspace_root.join(".dmz/toolchains");
        if !toolchains_dir.exists() {
            return Ok(());
        }

        // 1. Ensure 0o755 execution permissions on all binary files
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

        // 2. Clear macOS quarantine attribute (xattr -cr)
        #[cfg(target_os = "macos")]
        {
            let _ = Command::new("xattr")
                .arg("-cr")
                .arg(&toolchains_dir)
                .output();
        }

        // 3. Generate .dmz/env.sh and .dmz/env.bat activator scripts
        Self::generate_activator_scripts(workspace_root)?;

        Ok(())
    }

    /// Generates shell and batch activator scripts in `.dmz/` for immediate developer use.
    pub fn generate_activator_scripts(workspace_root: &Path) -> Result<(), String> {
        let env_sh_path = workspace_root.join(".dmz/env.sh");
        let env_bat_path = workspace_root.join(".dmz/env.bat");

        let env_sh_content = r#"#!/bin/sh
# DMZ Toolchain Environment Activator
# Source this script to activate bundled compilers and runtimes:
#   source .dmz/env.sh

DMZ_WS="$(cd "$(dirname "$0")/.." && pwd)"
export DMZ_WORKSPACE="$DMZ_WS"

# Prepend all bundled toolchain bin directories to PATH
for tc_bin in "$DMZ_WS"/.dmz/toolchains/*/bin; do
    if [ -d "$tc_bin" ]; then
        export PATH="$tc_bin:$PATH"
    fi
done

# Configure ecosystem environments
if [ -d "$DMZ_WS/.dmz/toolchains/rust" ]; then
    export RUSTUP_HOME="$DMZ_WS/.dmz/toolchains/rust"
    export CARGO_HOME="${CARGO_HOME:-$DMZ_WS/.cargo}"
    export DYLD_LIBRARY_PATH="$DMZ_WS/.dmz/toolchains/rust/lib:$DYLD_LIBRARY_PATH"
    export LD_LIBRARY_PATH="$DMZ_WS/.dmz/toolchains/rust/lib:$LD_LIBRARY_PATH"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/go" ]; then
    export GOROOT="$DMZ_WS/.dmz/toolchains/go"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/python" ]; then
    export PYTHONHOME="$DMZ_WS/.dmz/toolchains/python"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/cmake" ]; then
    export CMAKE="$DMZ_WS/.dmz/toolchains/cmake/bin/cmake"
    for cm_share in "$DMZ_WS"/.dmz/toolchains/cmake/share/cmake-*; do
        if [ -d "$cm_share" ]; then
            export CMAKE_ROOT="$cm_share"
            break
        fi
    done
fi

if [ -d "$DMZ_WS/.dmz/toolchains/juce" ]; then
    export JUCE_PATH="$DMZ_WS/.dmz/toolchains/juce"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/pkgconfig" ]; then
    export PKG_CONFIG="$DMZ_WS/.dmz/toolchains/pkgconfig/bin/pkg-config"
fi

echo "DMZ bundled toolchains activated. PATH and runtime variables configured."
"#;

        let env_bat_content = r#"@echo off
REM DMZ Toolchain Environment Activator (Windows)
set DMZ_WORKSPACE=%~dp0..

for /d %%D in ("%DMZ_WORKSPACE%\.dmz\toolchains\*") do (
    if exist "%%D\bin" (
        set "PATH=%%D\bin;%PATH%"
    )
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\rust" (
    set "RUSTUP_HOME=%DMZ_WORKSPACE%\.dmz\toolchains\rust"
    if not defined CARGO_HOME set "CARGO_HOME=%DMZ_WORKSPACE%\.cargo"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\go" (
    set "GOROOT=%DMZ_WORKSPACE%\.dmz\toolchains\go"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\python" (
    set "PYTHONHOME=%DMZ_WORKSPACE%\.dmz\toolchains\python"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cmake\bin\cmake.exe" (
    set "CMAKE=%DMZ_WORKSPACE%\.dmz\toolchains\cmake\bin\cmake.exe"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\juce" (
    set "JUCE_PATH=%DMZ_WORKSPACE%\.dmz\toolchains\juce"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkg-config.exe" (
    set "PKG_CONFIG=%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkg-config.exe"
)

echo DMZ bundled toolchains activated.
"#;

        fs::create_dir_all(workspace_root.join(".dmz"))
            .map_err(|e| format!("Failed to create .dmz directory: {}", e))?;

        fs::write(&env_sh_path, env_sh_content)
            .map_err(|e| format!("Failed to write env.sh: {}", e))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&env_sh_path, fs::Permissions::from_mode(0o755));
        }

        fs::write(&env_bat_path, env_bat_content)
            .map_err(|e| format!("Failed to write env.bat: {}", e))?;

        Ok(())
    }
}
