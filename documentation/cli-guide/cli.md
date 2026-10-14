# DMZ Command-Line Interface (CLI) Guide

DMZ provides a robust, self-contained CLI for orchestrating zero-dependency sandboxes, managing supervisor services, resolving multi-ecosystem dependencies, and executing deterministic air-gap closure transfers.

---

## Command Overview

| Command | Purpose |
| :--- | :--- |
| [`dmz resolve`](#resolving-dependencies-dmz-resolve) | Scans workspace manifests and generates a deterministic `dmz.lock` closure |
| [`dmz fetch`](#fetching-dependencies-dmz-fetch) | Downloads and caches locked package artifacts into the local secure store |
| [`dmz install`](#installing-dependencies-offline-dmz-install) | Materializes cached store artifacts into project layouts (node_modules, .cargo/vendor, etc.) offline |
| [`dmz ecosystem`](#ecosystem-management-dmz-ecosystem) | Discovers active language stacks and runs native offline engine verifications |
| [`dmz build`](#deterministic-packaging-dmz-build) | Builds deterministic package artifacts (`Target A: Deps`, `Target B: App`, `Target C: Unified`) |
| [`dmz airgap export`](#exporting-an-air-gap-bundle-dmz-airgap-export) | Bundles workspace source, lockfiles, and store items into a `.tar.zst` air-gap archive |
| [`dmz airgap import`](#importing--verifying-an-air-gap-bundle-dmz-airgap-import) | Cryptographically verifies and unpacks an air-gap archive into a target store and workspace |
| [`dmz sandbox`](#sandbox-execution-commands-dmz-sandbox) | Executes commands inside a native, daemonless OS isolation sandbox |
| [`dmz proxy`](#network-proxy-management-dmz-proxy) | Manages outbound network proxy auditing and domain whitelist enforcement |

---

## Dependency & Closure Resolution

### Resolving Dependencies (`dmz resolve`)

Scans workspace manifests across supported ecosystems (Cargo, npm, pnpm, Python, Go, Composer, Bundler, Maven) and compiles a cryptographically signed `dmz.lock` closure specification.

```bash
dmz resolve [OPTIONS]
```

#### **Options**
* `-w, --workspace <PATH>` — Path to project workspace root. *(Default: `.`)*
* `-o, --output <PATH>` — Destination path for generated lockfile. *(Default: `dmz.lock`)*
* `-v, --verbose` — Enable verbose debug logging output.

#### **Example**
```bash
dmz resolve --workspace . --output dmz.lock
```

---

### Fetching Dependencies (`dmz fetch`)

Reads `dmz.lock`, fetches genuine upstream package archives, computes cryptographic SHA-256 hashes, and populates the local content-addressable store.

```bash
dmz fetch [OPTIONS]
```

#### **Options**
* `--lockfile <PATH>` — Path to input `dmz.lock` specification. *(Default: `dmz.lock`)*
* `--store <PATH>` — Target directory for the local dependency store cache. *(Default: `.dmz/store`)*
* `-v, --verbose` — Enable verbose debug logging output.

#### **Example**
```bash
dmz fetch --lockfile dmz.lock --store .dmz/store
```

---

### Installing Dependencies Offline (`dmz install`)

Materializes dependencies from the offline store cache directly into standard project directory structures without requiring active network connectivity or host package managers.

```bash
dmz install [OPTIONS]
```

#### **Options**
* `-w, --workspace <PATH>` — Path to the destination project workspace. *(Default: `.`)*
* `-l, --lockfile <PATH>` — Path to the input `dmz.lock` specification. *(Default: `dmz.lock`)*
* `-s, --store <PATH>` — Path to the local dependency store directory. *(Default: `.dmz/store`)*
* `-v, --verbose` — Enable verbose debug logging output.

#### **Ecosystem Layout Materialization**

| Ecosystem | Destination Layout | Key Files Created |
| :--- | :--- | :--- |
| **Cargo (Rust)** | `.cargo/vendor/<pkg>-<version>/` | `.cargo/config.toml` (pointing to vendored source), `.cargo-checksum.json` |
| **npm / Classic Yarn** | `node_modules/<pkg>/` | Automatically flattens nested `package/` directories to locate `package.json` |
| **pnpm** | `node_modules/.pnpm/<pkg>@<ver>/` | Content-addressable virtual store directory and symlinked `node_modules/<pkg>` |
| **Python (PyPI)** | `.dmz/venv/lib/python3.11/site-packages/` | Fully unpacked package files preserving `.dist-info` and metadata |
| **Go** | `vendor/<pkg>/` | Unpacked module files and auto-maintained `vendor/modules.txt` |
| **PHP Composer** | `vendor/<pkg>/` | Unpacked Composer vendor directory layout |
| **Ruby Bundler** | `vendor/bundle/<pkg>/` | Unpacked gems with `.bundle/config` configuring frozen local execution |
| **Java Maven/Gradle** | `.m2/repository/<pkg>/` | Local Maven repository layout containing JAR artifacts |

#### **Example**
```bash
# Install dependencies into an imported workspace from an offline store
dmz install --workspace ./workspace --store ./offline_store --lockfile ./workspace/dmz.lock
```

---

## Ecosystem Management

### Discovering Active Stacks (`dmz ecosystem discover`)

Inspects workspace manifests to identify all active programming language stacks and reports the matching offline resolution rules.

```bash
dmz ecosystem discover [OPTIONS]
```

#### **Options**
* `-w, --workspace <PATH>` — Path to project workspace root. *(Default: `.`)*
* `-v, --verbose` — Enable verbose debug logging output.

---

### Native Engine Verification (`dmz ecosystem verify`)

Validates the workspace offline using the engine rules associated with each detected language stack (e.g., `cargo check --offline`, `pnpm test --offline`, `npm test --no-audit`, `go build -mod=readonly`).

```bash
dmz ecosystem verify [OPTIONS]
```

#### **Options**
* `-w, --workspace <PATH>` — Path to project workspace root. *(Default: `.`)*
* `-v, --verbose` — Enable verbose debug logging output.

---

## Air-Gap Packaging & Distribution

The air-gap command group (`dmz airgap export` and `dmz airgap import`) packages an entire project workspace alongside its locked dependency closures into a single, compressed, cryptographically verifiable `.tar.zst` archive.

### Exporting an Air-Gap Bundle (`dmz airgap export`)

Bundles project workspace code, lockfiles, and secure dependency stores into an air-gap archive.

```bash
dmz airgap export [OPTIONS]
```

#### **Options**
* `-w, --workspace <PATH>` — Path to workspace root directory. *(Default: `.`)*
* `-l, --lockfile <PATH>` — Path to `dmz.lock` closure specification. *(Default: `dmz.lock`)*
* `-s, --store <PATH>` — Path to local store directory. *(Default: `.dmz/store`)*
* `-o, --output <PATH>` — Destination path for generated `.tar.zst` archive. *(Default: `dist/airgap_bundle.tar.zst`)*
* `--mode <MODE>` — Export mode: `unified` (code + dependencies), `code-only`, or `deps-only`. *(Default: `unified`)*
* `--zstd-level <LEVEL>` — Zstandard compression level from `1` to `22`. *(Default: `3`)*
* `-v, --verbose` — Enable verbose debug logging output.

#### **Example**
```bash
dmz airgap export \
    --workspace . \
    --lockfile dmz.lock \
    --store .dmz/store \
    --output dist/workspace_closure.tar.zst \
    --mode unified
```

---

### Importing & Verifying an Air-Gap Bundle (`dmz airgap import`)

Cryptographically validates the archive SHA-256 and internal `dmz.lock` closure signature, unpacks store packages into the target store, and extracts project source code into the specified workspace destination directory.

```bash
dmz airgap import [OPTIONS] --archive <ARCHIVE>
```

#### **Options**
* `-a, --archive <PATH>` — Path to the `.tar.zst` air-gap bundle. **[Required]**
* `-s, --store <PATH>` — Target directory for the offline dependency store. *(Default: `.dmz/store`)*
* `-w, --workspace <PATH>` — Destination directory for extracted workspace source code and manifests. *(Default: `workspace`)*
* `--skip-verify` — Skips closure hash verification *(NOT recommended for production environments)*.
* `-v, --verbose` — Enable verbose debug logging output.

#### **Examples**

```bash
# Standard import (extracts store to ./offline_store and source to ./workspace)
dmz airgap import --archive dist/workspace_closure.tar.zst --store ./offline_store

# Custom destination workspace (e.g. testing in a fresh dmz-import directory)
dmz airgap import \
    --archive dist/workspace_closure.tar.zst \
    --store ./dmz-import/offline_store \
    --workspace ./dmz-import/workspace
```

---

## Deterministic Packaging

### Building Packaged Artifacts (`dmz build`)

Produces reproducible application archives compiled to specific build targets.

```bash
dmz build [OPTIONS]
```

#### **Options**
* `-t, --target <MODE>` — Build target mode: `unified` (code + deps), `dependencies` (deps only), or `app` (repo code only). *(Default: `unified`)*
* `-w, --workspace <PATH>` — Workspace root directory. *(Default: `.`)*
* `-l, --lockfile <PATH>` — Input lockfile path. *(Default: `dmz.lock`)*
* `-o, --output <PATH>` — Output directory for build artifacts. *(Default: `dist`)*
* `--name <NAME>` — Base name for generated artifact. *(Default: `workspace_closure`)*
* `--zstd-level <LEVEL>` — Zstd compression level (1-22). *(Default: `3`)*

---

## End-to-End Air-Gap Import & Installation Walkthrough

Here is the complete sequence for preparing, transferring, and installing an air-gapped workspace into a fresh target directory (such as `dmz-import`):

```bash
# 1. Resolve and lock workspace dependencies
dmz resolve

# 2. Pre-fetch dependencies into local cache
dmz fetch --store .dmz/store

# 3. Export unified air-gap archive
dmz airgap export \
    --output dist/workspace_closure.tar.zst \
    --store .dmz/store \
    --mode unified

# 4. Simulate isolated air-gap target environment
mkdir -p dmz-import
cd dmz-import

# 5. Import and verify archive
dmz airgap import \
    --archive ../dist/workspace_closure.tar.zst \
    --store ./offline_store

# 6. Install dependencies into project layouts without network access
cd workspace
dmz install \
    --store ../offline_store \
    --lockfile dmz.lock

# 7. Run native engine verification offline
dmz ecosystem verify
```

---

## Sandbox Execution Commands (`dmz sandbox`)

The sandbox command group executes commands within isolated, daemonless operating system sandboxes with zero host leakage.

### Running a Command in Isolation (`dmz sandbox`)

```bash
dmz sandbox [OPTIONS] -- <COMMAND> [ARGS...]
```

#### **Options**
* `-w, --workspace <PATH>` — Workspace directory context mounted R/W. *(Default: `.`)*
* `-c, --closure <PATH>` — Dependency closure store mounted R/O. *(Default: `/nix/store`)*
* `--allow-network` — Permits outbound network connections *(Default: `false` for complete isolation)*.
* `--diagnose` — Enables automated error classification to differentiate platform faults from user code faults.

#### **Examples**

```bash
# Run isolated offline build with error diagnosis
dmz sandbox -w ./my-app --diagnose /bin/sh -c "cargo check --offline"

# Run tests with network disabled
dmz sandbox -w ./my-app /bin/sh -c "npm test"
```

---

## Network Proxy Management (`dmz proxy`)

The proxy command group manages the volatile outbound network auditor, enforcing strict domain allowlists for AI agent execution.

### Starting the Proxy Auditor (`dmz proxy start`)

```bash
dmz proxy start [OPTIONS]
```

#### **Options**
* `-p, --port <PORT>` — Port for proxy auditor. *(Default: `8080`)*
* `-l, --log-file <PATH>` — Audit log destination.
* `--strict` — Blocks any unlisted outbound domains by default.

### Managing Whitelist Rules (`dmz proxy whitelist`)

```bash
# Add trusted domain
dmz proxy whitelist add crates.io

# List approved domains
dmz proxy whitelist list

# Remove domain
dmz proxy whitelist remove crates.io
```