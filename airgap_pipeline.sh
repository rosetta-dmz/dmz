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
echo "=== Discovering active ecosystem stacks ==="
echo "=================================================="
dmz ecosystem discover

echo "=================================================="
echo "=== Fetching and hashing offline dependencies ==="
echo "=================================================="
dmz fetch --store .dmz/store

echo "=================================================="
echo "=== Building unified package artifacts ==="
echo "=================================================="
dmz build --target unified --output dist --name workspace_closure

echo "=================================================="
echo "=== Exporting the air-gap bundle (Unified) ==="
echo "=================================================="
dmz airgap export \
    --output dist/workspace_closure.tar.zst \
    --store .dmz/store \
    --mode unified

echo "=================================================="
echo "=== Simulating isolated air-gap import ==="
echo "=================================================="
mkdir -p airgap-test-env
cd airgap-test-env

# Import the archive (unpacks store, places source code into 'workspace/')
dmz airgap import \
    --archive ../dist/workspace_closure.tar.zst \
    --store ./offline_store

echo "=================================================="
echo "=== Verifying offline workspace via DMZ engine ==="
echo "=================================================="
cd workspace

# Execute engine-driven ecosystem verification natively
dmz ecosystem verify

echo "=================================================="
echo "End-to-end multi-ecosystem air-gap pipeline completed!"
echo "=================================================="