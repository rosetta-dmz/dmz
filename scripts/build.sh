#!/usr/bin/env bash
set -euo pipefail

DIST_DIR="dist"
BUILD_TARGET="${1:-host}"

echo "=========================================================="
echo "      DMZ Factory — Single Binary Release Builder         "
echo "=========================================================="

mkdir -p "${DIST_DIR}"

# 1. Detect Host Environment
HOST_OS="$(uname -s | tr '[:upper:]' '[:lower:]')"
HOST_ARCH="$(uname -m)"

echo "[+] Host Environment: ${HOST_OS} (${HOST_ARCH})"

# 2. Configure Static Compilation Flags
export RUSTFLAGS="-C target-feature=+crt-static -C link-arg=-s"

build_target() {
    local target_triple="$1"
    local output_suffix="$2"

    echo "[+] Adding rustup target: ${target_triple}"
    rustup target add "${target_triple}" || true

    echo "[+] Compiling dmz-cli for target: ${target_triple}"
    cargo build --release --target "${target_triple}" -p dmz-cli

    echo "[+] Compiling dmz-gui for target: ${target_triple}"
    cargo build --release --target "${target_triple}" -p dmz-gui

    local ext=""
    if [[ "${target_triple}" == *"windows"* ]]; then
        ext=".exe"
    fi

    cp "target/${target_triple}/release/dmz-cli${ext}" "${DIST_DIR}/dmz-${output_suffix}${ext}"
    cp "target/${target_triple}/release/dmz-gui${ext}" "${DIST_DIR}/dmz-gui-${output_suffix}${ext}"
}

# 3. Dispatch Build Pipeline
case "${BUILD_TARGET}" in
    "linux-musl")
        build_target "x86_64-unknown-linux-musl" "linux-x86_64"
        ;;
    "windows")
        build_target "x86_64-pc-windows-msvc" "windows-x86_64"
        ;;
    "macos-arm")
        build_target "aarch64-apple-darwin" "macos-arm64"
        ;;
    "host")
        echo "[+] Building release binaries for host OS..."
        cargo build --release -p dmz-cli
        cargo build --release -p dmz-gui
        cp "target/release/dmz-cli" "${DIST_DIR}/dmz"
        cp "target/release/dmz-gui" "${DIST_DIR}/dmz-gui"
        ;;
    "all")
        echo "[+] Building cross-platform distribution matrix..."
        if [[ "${HOST_OS}" == "linux" ]]; then
            build_target "x86_64-unknown-linux-musl" "linux-x86_64"
        elif [[ "${HOST_OS}" == "darwin" ]]; then
            build_target "aarch64-apple-darwin" "macos-arm64"
            build_target "x86_64-apple-darwin" "macos-x86_64"
        fi
        ;;
    *)
        echo "[-] Unknown build target: ${BUILD_TARGET}"
        echo "    Usage: $0 [host|linux-musl|windows|macos-arm|all]"
        exit 1
        ;;
esac

# 4. Verify Static Binary & Print Telemetry
echo ""
echo "=========================================================="
echo "                 Build Output Telemetry                   "
echo "=========================================================="
ls -lh "${DIST_DIR}"

if command -v ldd &> /dev/null && [[ -f "${DIST_DIR}/dmz" ]]; then
    echo ""
    echo "[+] Checking static library linkage for 'dmz':"
    ldd "${DIST_DIR}/dmz" || echo "    ✓ Statically linked (No dynamic dependencies found)"
fi

echo "=========================================================="
echo "[✓] DMZ release build completed successfully."