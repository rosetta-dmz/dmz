// Shell and batch environment activator script generation

use std::fs;
use std::path::Path;

/// Generates shell and batch activator scripts in `.dmz/` for immediate developer use.
pub fn generate_activator_scripts(workspace_root: &Path) -> Result<(), String> {
    let env_sh_path = workspace_root.join(".dmz/env.sh");
    let env_bat_path = workspace_root.join(".dmz/env.bat");

    let mut env_sh_content = r#"#!/bin/sh
# DMZ Toolchain Environment Activator
# Source this script to activate bundled compilers and runtimes:
#   source .dmz/env.sh

# Ensure safe globbing behavior across shells (POSIX sh, bash, and zsh)
if [ -n "$ZSH_VERSION" ]; then
    setopt localoptions nonomatch nullglob 2>/dev/null || true
fi

# Determine DMZ workspace root reliably across bash, zsh, and dash/POSIX sh
if [ -n "$BASH_SOURCE" ]; then
    _DMZ_SELF="$BASH_SOURCE"
elif [ -n "$ZSH_VERSION" ]; then
    _DMZ_SELF="${(%):-%x}"
else
    _DMZ_SELF="$0"
fi

# Resolve physical path to handle symlinked .dmz directories in member workspaces
if [ -f "$_DMZ_SELF" ]; then
    _DMZ_DIR="$(cd -P "$(dirname "$_DMZ_SELF")" 2>/dev/null && pwd -P)"
    DMZ_WS="$(cd -P "$_DMZ_DIR/.." 2>/dev/null && pwd -P)"
elif [ -d "./.dmz" ]; then
    _DMZ_DIR="$(cd -P "./.dmz" 2>/dev/null && pwd -P)"
    DMZ_WS="$(cd -P "$_DMZ_DIR/.." 2>/dev/null && pwd -P)"
elif [ -d "../.dmz" ]; then
    _DMZ_DIR="$(cd -P "../.dmz" 2>/dev/null && pwd -P)"
    DMZ_WS="$(cd -P "$_DMZ_DIR/.." 2>/dev/null && pwd -P)"
elif [ -d "../../.dmz" ]; then
    _DMZ_DIR="$(cd -P "../../.dmz" 2>/dev/null && pwd -P)"
    DMZ_WS="$(cd -P "$_DMZ_DIR/.." 2>/dev/null && pwd -P)"
else
    DMZ_WS="$(cd "$(dirname "$0")/.." 2>/dev/null && pwd)"
fi

# Fallback parent directory traversal if .dmz/toolchains is in an ancestor directory
_CURR="$DMZ_WS"
while [ -n "$_CURR" ] && [ "$_CURR" != "/" ]; do
    if [ -d "$_CURR/.dmz/toolchains" ]; then
        DMZ_WS="$_CURR"
        break
    fi
    _CURR="$(dirname "$_CURR")"
done
export DMZ_WORKSPACE="$DMZ_WS"

# Prepend all bundled toolchain bin directories to PATH
if [ -d "$DMZ_WS/.dmz/toolchains" ]; then
    for tc_bin in "$DMZ_WS"/.dmz/toolchains/*/bin; do
        if [ -d "$tc_bin" ]; then
            case "$tc_bin" in
                *"/applesdk/bin")
                    if [ ! -d "$DMZ_WS/.dmz/toolchains/applesdk/SDKs" ] && [ ! -d "$DMZ_WS/.dmz/toolchains/applesdk/Platforms" ]; then
                        continue
                    fi
                    ;;
            esac
            case ":$PATH:" in
                *":$tc_bin:"*) ;;
                *) export PATH="$tc_bin:$PATH" ;;
            esac
        fi
    done
fi

# Prepend member node_modules/.bin directories to PATH
if [ -d "$DMZ_WS/workspaces" ]; then
    for ws_bin in "$DMZ_WS"/workspaces/*/node_modules/.bin; do
        if [ -d "$ws_bin" ]; then
            case ":$PATH:" in
                *":$ws_bin:"*) ;;
                *) export PATH="$ws_bin:$PATH" ;;
            esac
        fi
    done
fi
if [ -d "$DMZ_WS/node_modules/.bin" ]; then
    case ":$PATH:" in
        *":$DMZ_WS/node_modules/.bin:"*) ;;
        *) export PATH="$DMZ_WS/node_modules/.bin:$PATH" ;;
    esac
fi

# Configure ecosystem environments
if [ -d "$DMZ_WS/.dmz/toolchains/rust" ]; then
    export RUSTUP_HOME="$DMZ_WS/.dmz/toolchains/rust"
    export CARGO_HOME="${CARGO_HOME:-$DMZ_WS/.cargo}"
    case ":$DYLD_LIBRARY_PATH:" in
        *":$DMZ_WS/.dmz/toolchains/rust/lib:"*) ;;
        *) export DYLD_LIBRARY_PATH="$DMZ_WS/.dmz/toolchains/rust/lib${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}" ;;
    esac
    case ":$LD_LIBRARY_PATH:" in
        *":$DMZ_WS/.dmz/toolchains/rust/lib:"*) ;;
        *) export LD_LIBRARY_PATH="$DMZ_WS/.dmz/toolchains/rust/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}" ;;
    esac
fi

if [ -d "$DMZ_WS/.dmz/toolchains/go" ] && ([ -f "$DMZ_WS/.dmz/toolchains/go/bin/go" ] || [ -f "$DMZ_WS/.dmz/toolchains/go/bin/go.exe" ]); then
    export GOROOT="$DMZ_WS/.dmz/toolchains/go"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/python" ] && ([ -d "$DMZ_WS/.dmz/toolchains/python/lib" ] || [ -d "$DMZ_WS/.dmz/toolchains/python/Lib" ]); then
    export PYTHONHOME="$DMZ_WS/.dmz/toolchains/python"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/node" ]; then
    if [ -d "$DMZ_WS/.dmz/toolchains/node/lib/node_modules" ]; then
        case ":$NODE_PATH:" in
            *":$DMZ_WS/.dmz/toolchains/node/lib/node_modules:"*) ;;
            *) export NODE_PATH="$DMZ_WS/.dmz/toolchains/node/lib/node_modules${NODE_PATH:+:$NODE_PATH}" ;;
        esac
    fi
fi

if [ -d "$DMZ_WS/.dmz/toolchains/php" ] && ([ -f "$DMZ_WS/.dmz/toolchains/php/bin/php" ] || [ -f "$DMZ_WS/.dmz/toolchains/php/php.ini" ]); then
    export PHPRC="$DMZ_WS/.dmz/toolchains/php"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/ruby" ] && ([ -f "$DMZ_WS/.dmz/toolchains/ruby/bin/ruby" ] || [ -f "$DMZ_WS/.dmz/toolchains/ruby/bin/bundle" ] || [ -d "$DMZ_WS/.dmz/toolchains/ruby/gems" ]); then
    export GEM_HOME="$DMZ_WS/.dmz/toolchains/ruby"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/cc" ]; then
    if [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/clang" ]; then
        export CC="$DMZ_WS/.dmz/toolchains/cc/bin/clang"
        export CXX="$DMZ_WS/.dmz/toolchains/cc/bin/clang++"
    elif [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/gcc" ]; then
        export CC="$DMZ_WS/.dmz/toolchains/cc/bin/gcc"
        export CXX="$DMZ_WS/.dmz/toolchains/cc/bin/g++"
    elif [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/cc" ]; then
        export CC="$DMZ_WS/.dmz/toolchains/cc/bin/cc"
    fi
    if [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/llvm-ar" ]; then
        export AR="$DMZ_WS/.dmz/toolchains/cc/bin/llvm-ar"
    elif [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/ar" ]; then
        export AR="$DMZ_WS/.dmz/toolchains/cc/bin/ar"
    fi
    if [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/llvm-ranlib" ]; then
        export RANLIB="$DMZ_WS/.dmz/toolchains/cc/bin/llvm-ranlib"
    elif [ -f "$DMZ_WS/.dmz/toolchains/cc/bin/ranlib" ]; then
        export RANLIB="$DMZ_WS/.dmz/toolchains/cc/bin/ranlib"
    fi
    if [ -d "$DMZ_WS/.dmz/toolchains/cc/lib" ]; then
        export DYLD_LIBRARY_PATH="$DMZ_WS/.dmz/toolchains/cc/lib:$DYLD_LIBRARY_PATH"
        export LD_LIBRARY_PATH="$DMZ_WS/.dmz/toolchains/cc/lib:$LD_LIBRARY_PATH"
    fi
fi

if [ -f "$DMZ_WS/.dmz/toolchains/cmake/bin/cmake" ]; then
    export CMAKE="$DMZ_WS/.dmz/toolchains/cmake/bin/cmake"
    if [ -d "$DMZ_WS/.dmz/toolchains/cmake/share" ]; then
        for cm_share in "$DMZ_WS"/.dmz/toolchains/cmake/share/cmake-*; do
            if [ -d "$cm_share" ]; then
                export CMAKE_ROOT="$cm_share"
                break
            fi
        done
    fi
fi

if [ -f "$DMZ_WS/.dmz/toolchains/dmz.toolchain.cmake" ] && ([ -d "$DMZ_WS/.dmz/toolchains/cmake" ] || [ -f "$DMZ_WS/CMakeLists.txt" ]); then
    export CMAKE_TOOLCHAIN_FILE="$DMZ_WS/.dmz/toolchains/dmz.toolchain.cmake"
    export HOST_CMAKE_TOOLCHAIN_FILE="$DMZ_WS/.dmz/toolchains/dmz.toolchain.cmake"
    export TARGET_CMAKE_TOOLCHAIN_FILE="$DMZ_WS/.dmz/toolchains/dmz.toolchain.cmake"
fi

if [ -f "$DMZ_WS/.dmz/toolchains/ninja/bin/ninja" ]; then
    export NINJA="$DMZ_WS/.dmz/toolchains/ninja/bin/ninja"
    export CMAKE_GENERATOR="Ninja"
fi

if [ -f "$DMZ_WS/.dmz/toolchains/make/bin/make" ]; then
    export MAKE="$DMZ_WS/.dmz/toolchains/make/bin/make"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/juce" ]; then
    export JUCE_PATH="$DMZ_WS/.dmz/toolchains/juce"
fi

if [ -d "$DMZ_WS/.dmz/toolchains/androidndk" ]; then
    export ANDROID_NDK_HOME="$DMZ_WS/.dmz/toolchains/androidndk"
    export ANDROID_NDK_ROOT="$DMZ_WS/.dmz/toolchains/androidndk"
    if [ -f "$DMZ_WS/.dmz/toolchains/androidndk/build/cmake/android.toolchain.cmake" ]; then
        if [ ! -f "$DMZ_WS/.dmz/toolchains/dmz.toolchain.cmake" ]; then
            export CMAKE_TOOLCHAIN_FILE="$DMZ_WS/.dmz/toolchains/androidndk/build/cmake/android.toolchain.cmake"
            export HOST_CMAKE_TOOLCHAIN_FILE="$DMZ_WS/.dmz/toolchains/androidndk/build/cmake/android.toolchain.cmake"
            export TARGET_CMAKE_TOOLCHAIN_FILE="$DMZ_WS/.dmz/toolchains/androidndk/build/cmake/android.toolchain.cmake"
        fi
    fi
fi

if [ -d "$DMZ_WS/.dmz/toolchains/pkgconfig" ]; then
    if [ -f "$DMZ_WS/.dmz/toolchains/pkgconfig/bin/pkg-config" ]; then
        export PKG_CONFIG="$DMZ_WS/.dmz/toolchains/pkgconfig/bin/pkg-config"
    elif [ -f "$DMZ_WS/.dmz/toolchains/pkgconfig/bin/pkgconf" ]; then
        export PKG_CONFIG="$DMZ_WS/.dmz/toolchains/pkgconfig/bin/pkgconf"
    fi
    if [ -d "$DMZ_WS/.dmz/toolchains/pkgconfig/lib/pkgconfig" ] || [ -d "$DMZ_WS/.dmz/toolchains/pkgconfig/share/pkgconfig" ]; then
        export PKG_CONFIG_PATH="$DMZ_WS/.dmz/toolchains/pkgconfig/lib/pkgconfig:$DMZ_WS/.dmz/toolchains/pkgconfig/share/pkgconfig:$PKG_CONFIG_PATH"
    fi
fi
"#
    .to_string();

    if crate::toolchain::ToolchainEnv::is_genuine_apple_developer_dir(
        &workspace_root.join(".dmz/toolchains/applesdk"),
    ) || crate::toolchain::ToolchainEnv::find_apple_sdk_root(
        &workspace_root.join(".dmz/toolchains/applesdk"),
    )
    .is_some()
    {
        env_sh_content.push_str(
            r#"
if [ -d "$DMZ_WS/.dmz/toolchains/applesdk" ]; then
    for sdk_parent in "$DMZ_WS/.dmz/toolchains/applesdk/Platforms/MacOSX.platform/Developer/SDKs" "$DMZ_WS/.dmz/toolchains/applesdk/SDKs"; do
        if [ -d "$sdk_parent" ]; then
            for sdk_path in "$sdk_parent"/MacOSX*.sdk; do
                if [ -d "$sdk_path" ]; then
                    export SDKROOT="$sdk_path"
                    break 2
                fi
            done
        fi
    done
    if [ -f "$DMZ_WS/.dmz/toolchains/applesdk/usr/bin/xcrun" ] || [ -f "$DMZ_WS/.dmz/toolchains/applesdk/bin/xcrun" ]; then
        export DEVELOPER_DIR="$DMZ_WS/.dmz/toolchains/applesdk"
    fi
fi
"#,
        );
    }

    env_sh_content.push_str(
        "\necho \"DMZ bundled toolchains activated. PATH and runtime variables configured.\"\n",
    );

    let env_bat_content = r#"@echo off
REM DMZ Toolchain Environment Activator (Windows)
set DMZ_WORKSPACE=%~dp0..

for /d %%D in ("%DMZ_WORKSPACE%\.dmz\toolchains\*") do (
    if exist "%%D\bin" (
        echo ;%PATH%; | findstr /I /C:";%%D\bin;" >nul || set "PATH=%%D\bin;%PATH%"
    )
)

for /d %%D in ("%DMZ_WORKSPACE%\workspaces\*") do (
    if exist "%%D\node_modules\.bin" (
        echo ;%PATH%; | findstr /I /C:";%%D\node_modules\.bin;" >nul || set "PATH=%%D\node_modules\.bin;%PATH%"
    )
)

if exist "%DMZ_WORKSPACE%\node_modules\.bin" (
    echo ;%PATH%; | findstr /I /C:";%DMZ_WORKSPACE%\node_modules\.bin;" >nul || set "PATH=%DMZ_WORKSPACE%\node_modules\.bin;%PATH%"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\rust" (
    set "RUSTUP_HOME=%DMZ_WORKSPACE%\.dmz\toolchains\rust"
    if not defined CARGO_HOME set "CARGO_HOME=%DMZ_WORKSPACE%\.cargo"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\go\bin\go.exe" (
    set "GOROOT=%DMZ_WORKSPACE%\.dmz\toolchains\go"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\python\Lib" (
    set "PYTHONHOME=%DMZ_WORKSPACE%\.dmz\toolchains\python"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\python\lib" (
    set "PYTHONHOME=%DMZ_WORKSPACE%\.dmz\toolchains\python"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\node\lib\node_modules" (
    set "NODE_PATH=%DMZ_WORKSPACE%\.dmz\toolchains\node\lib\node_modules;%NODE_PATH%"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\php\bin\php.exe" (
    set "PHPRC=%DMZ_WORKSPACE%\.dmz\toolchains\php"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\ruby\bin\ruby.exe" (
    set "GEM_HOME=%DMZ_WORKSPACE%\.dmz\toolchains\ruby"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\clang.exe" (
    set "CC=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\clang.exe"
    set "CXX=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\clang++.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\gcc.exe" (
    set "CC=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\gcc.exe"
    set "CXX=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\g++.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\cc.exe" (
    set "CC=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\cc.exe"
)
if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\llvm-ar.exe" (
    set "AR=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\llvm-ar.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\ar.exe" (
    set "AR=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\ar.exe"
)
if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\llvm-ranlib.exe" (
    set "RANLIB=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\llvm-ranlib.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\ranlib.exe" (
    set "RANLIB=%DMZ_WORKSPACE%\.dmz\toolchains\cc\bin\ranlib.exe"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cmake\bin\cmake.exe" (
    set "CMAKE=%DMZ_WORKSPACE%\.dmz\toolchains\cmake\bin\cmake.exe"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake" (
    if exist "%DMZ_WORKSPACE%\.dmz\toolchains\cmake" (
        set "CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
        set "HOST_CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
        set "TARGET_CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
    ) else if exist "%DMZ_WORKSPACE%\CMakeLists.txt" (
        set "CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
        set "HOST_CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
        set "TARGET_CMAKE_TOOLCHAIN_FILE=%DMZ_WORKSPACE%\.dmz\toolchains\dmz.toolchain.cmake"
    )
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\ninja\bin\ninja.exe" (
    set "NINJA=%DMZ_WORKSPACE%\.dmz\toolchains\ninja\bin\ninja.exe"
    set "CMAKE_GENERATOR=Ninja"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\make\bin\make.exe" (
    set "MAKE=%DMZ_WORKSPACE%\.dmz\toolchains\make\bin\make.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\make\bin\mingw32-make.exe" (
    set "MAKE=%DMZ_WORKSPACE%\.dmz\toolchains\make\bin\mingw32-make.exe"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\juce" (
    set "JUCE_PATH=%DMZ_WORKSPACE%\.dmz\toolchains\juce"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\androidndk" (
    set "ANDROID_NDK_HOME=%DMZ_WORKSPACE%\.dmz\toolchains\androidndk"
    set "ANDROID_NDK_ROOT=%DMZ_WORKSPACE%\.dmz\toolchains\androidndk"
)

if exist "%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkg-config.exe" (
    set "PKG_CONFIG=%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkg-config.exe"
) else if exist "%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkgconf.exe" (
    set "PKG_CONFIG=%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\bin\pkgconf.exe"
)
if exist "%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\lib\pkgconfig" (
    set "PKG_CONFIG_PATH=%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\lib\pkgconfig;%DMZ_WORKSPACE%\.dmz\toolchains\pkgconfig\share\pkgconfig;%PKG_CONFIG_PATH%"
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

