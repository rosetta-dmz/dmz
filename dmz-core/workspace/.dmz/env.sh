#!/bin/sh
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

_START_DIR=""
if [ -f "$_DMZ_SELF" ]; then
    _START_DIR="$(cd "$(dirname "$_DMZ_SELF")/.." && pwd)"
elif [ -d "./.dmz" ]; then
    _START_DIR="$(pwd)"
elif [ -d "../.dmz" ]; then
    _START_DIR="$(cd .. && pwd)"
else
    _START_DIR="$(cd "$(dirname "$0")/.." 2>/dev/null && pwd)"
fi

# Walk upward to find top-level workspace root containing toolchains or dmz manifest
DMZ_WS="$_START_DIR"
_CUR="$_START_DIR"
while [ -n "$_CUR" ] && [ "$_CUR" != "/" ] && [ "$_CUR" != "." ]; do
    if [ -d "$_CUR/.dmz/toolchains" ] || [ -f "$_CUR/dmz-workspace.toml" ] || [ -f "$_CUR/dmz.toml" ]; then
        DMZ_WS="$_CUR"
        break
    fi
    _PARENT="$(cd "$_CUR/.." 2>/dev/null && pwd)"
    if [ "$_PARENT" = "$_CUR" ]; then
        break
    fi
    _CUR="$_PARENT"
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

# Explicitly unset PYTHONHOME to avoid host Python stdlib corruption ("failed to import encodings module")
unset PYTHONHOME 2>/dev/null || true

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

echo "DMZ bundled toolchains activated. PATH and runtime variables configured."
