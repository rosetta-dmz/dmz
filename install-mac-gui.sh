#!/usr/bin/env bash
set -e

REPO="rosetta-dmz/dmz"
DOWNLOAD_URL="https://github.com/${REPO}/releases/latest/download/DMZ-GUI-macOS.zip"

echo "==> Downloading DMZ GUI for macOS..."
TMP_DIR=$(mktemp -d)
ZIP_PATH="$TMP_DIR/DMZ-GUI-macOS.zip"

if ! curl -L -s "$DOWNLOAD_URL" -o "$ZIP_PATH"; then
    echo "Error: Failed to download DMZ GUI from GitHub Releases."
    exit 1
fi

echo "==> Extracting archive..."
unzip -q "$ZIP_PATH" -d "$TMP_DIR"

if [ ! -d "$TMP_DIR/DMZ.app" ]; then
    echo "Error: DMZ.app could not be found in the downloaded archive."
    exit 1
fi

echo "==> Installing DMZ.app to /Applications..."
if [ -d "/Applications/DMZ.app" ]; then
    echo "Removing existing installation..."
    rm -rf "/Applications/DMZ.app"
fi

mv "$TMP_DIR/DMZ.app" /Applications/DMZ.app

echo "==> Clearing macOS quarantine attributes..."
xattr -cr /Applications/DMZ.app

rm -rf "$TMP_DIR"

echo ""
echo "==> Successfully installed DMZ.app to /Applications"