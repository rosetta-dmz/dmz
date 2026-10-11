#!/usr/bin/env bash
set -euo pipefail

echo "=================================================="
echo "=== Cleaning previous build & test artifacts ==="
echo "=================================================="
rm -rf dist/ .dmz/ store/ dmz.lock airgap-test-env/

echo "=================================================="
echo "=== Resolving workspace dependencies ==="
echo "=================================================="
dmz resolve

echo "=================================================="
echo "=== Fetching and hashing offline dependencies ==="
echo "=================================================="
dmz fetch --store .dmz/store

echo "=================================================="
echo "=== Exporting the air-gap bundle (Unified) ==="
echo "=================================================="
dmz airgap export \
    --output dist/workspace_closure.tar.zst \
    --store .dmz/store

echo "=================================================="
echo "=== Simulating isolated air-gap import ==="
echo "=================================================="
mkdir -p airgap-test-env
cd airgap-test-env

# Import the archive (unpacks store and places source code into 'workspace/')
dmz airgap import \
    --archive ../dist/workspace_closure.tar.zst \
    --store ./offline_store

echo "=================================================="
echo "=== Verifying offline compilation (Cargo Check) ==="
echo "=================================================="
cd workspace
cargo check --offline

echo "=================================================="
echo "End-to-end air-gap pipeline completed successfully!"
echo "=================================================="