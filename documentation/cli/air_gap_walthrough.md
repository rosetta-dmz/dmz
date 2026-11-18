# End-to-End Air-Gap Import & Installation Walkthrough

Sequence for preparing, transferring, and installing an air-gapped workspace into a fresh target directory:

```bash
# Resolve and lock minimal runtime/build workspace dependencies (pass `--include-dev` if test/lint deps are needed)
dmz resolve

# Pre-fetch dependencies into local cache
dmz fetch --store .dmz/store

# Export unified air-gap archive (defaults to minimal toolchain closure: --toolchain-profile minimal)
dmz airgap export --output dist/workspace_closure.tar.zst --store .dmz/store --mode unified --toolchain-profile minimal

# Simulate isolated air-gap target environment
mkdir -p dmz-import
cd dmz-import

# Import and verify archive (prunes unreferenced store files & enforces Root Manifest Dominance)
dmz airgap import --archive ../dist/workspace_closure.tar.zst --store ./offline_store

# Install dependencies into project layouts offline (Python uses single-copy .dmz/venv linked via .venv/.../dmz_store.pth)
cd workspace
dmz install --store ../offline_store --lockfile dmz.lock

# Run native engine verification offline (cargo check --offline, npm/pnpm/yarn ls --offline, python3 -m compileall -q ., go build -mod=vendor ./...)
dmz ecosystem verify
```