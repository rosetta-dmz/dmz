# What is DMZ?

**DMZ** is an open-source Portable Workspace Packager (PWP) designed for air-gapped software development, deterministic offline distribution, and daemonless sandbox execution.

Point it at a working project and it captures the workspace source + its dependency closure into a self-contained, cryptographically signed bundle that can be restored and materialized offline on target machines—with zero active network connectivity.

---

## What DMZ Does

- **Multi-Ecosystem Detection:** Automatically scans workspaces to detect manifests and dependency graphs across Rust (Cargo), Node.js (npm, pnpm, Yarn), Python (pip/PyPI), Go modules, PHP (Composer), and Ruby (Bundler).
- **Offline Store & Closure Fetching:** Downloads and caches locked package archives into a local, content-addressable store (`.dmz/store`).
- **Deterministic Cryptographic Lockfile (`dmz.lock`):** Automatically records and verifies closure contents, authentic SHA-256 artifact digests, closure signatures, and target platform tags (`macos-arm64`, `linux-x86_64`, etc.).
- **High-Performance Air-Gap Packaging:** Packs workspace source and dependency closures into compact `.tar.zst` archives using embedded Zstandard compression.
- **Offline Materialization:** On the target machine, DMZ restores the workspace and unpacks store dependencies directly into standard project layouts (`node_modules`, `.cargo/vendor`, `.dmz/venv`, `vendor/`) without requiring upstream package registries or internet access.

---

## Sandboxed Execution

DMZ provides daemonless, process-level isolation directly through host operating system primitives without requiring a Docker daemon or virtual machine:

- **macOS:** Enforces strict Seatbelt sandbox profiles restricting write access to the workspace directory and dependency store.
- **Linux:** Utilizes process namespaces and unshare boundaries for filesystem and network isolation.
- **Windows:** Orchestrates execution using restricted tokens and Job Objects.

```bash
# Open an interactive shell inside the sandbox (sandboxed & network-blocked by default)
dmz shell

# Execute a one-off command string directly inside the sandbox
dmz shell -c "cargo test"

# Launch unconfined on the host with DMZ environment variables loaded
dmz shell --no-sandbox

# Run any build or test command sandboxed with network access enabled
dmz sandbox --allow-network -- cargo build
```

---

## Two Workflows

- **CLI:** Complete control over resolution, offline fetching, bundling, and sandboxed execution.
- **GUI:** Cross-platform desktop interface (`dmz-gui`) built with egui for single-click air-gap bundle export/import, workspace sandbox isolation toggles, interactive command execution, dependency lifecycle resolution, and theme customization.

---

## How Does DMZ Work?

### 1. On the source machine (with internet access):

```bash
cd your-project

# Initialize DMZ capture profile and detect project ecosystems
dmz init --target macos-arm64

# Package workspace and resolved dependency closure into a portable bundle
dmz bundle --target macos-arm64 --output ../your-project-macos-arm64.bundle.tar.zst
```

`dmz init` initializes `.dmz/store` and generates the initial `dmz.lock`. `dmz bundle` fetches genuine package artifacts, updates `dmz.lock` with verified SHA-256 checksums, and exports the compressed `.tar.zst` bundle.

### 2. On the air-gapped target machine (completely offline):

```bash
# Unpack bundle, verify cryptographic signatures, and materialize dependencies
dmz import --archive ../your-project-macos-arm64.bundle.tar.zst --to ~/workspaces/your-project

cd ~/workspaces/your-project

# Run tests or launch your build inside the daemonless sandbox
dmz shell
```

---

## Platform Roadmap & Milestones

| Capability | Current Status (v1.0.9) | Milestone Target |
|---|---|---|
| **Air-Gap Packaging (`.tar.zst`)** | **Implemented & Verified** | Stable |
| **Multi-Ecosystem Dependency Materialization** | **Implemented & Verified** (Rust, npm, pnpm, Python, Go) | Stable |
| **Target Platform Tagging & Hash Verification** | **Implemented & Verified** in `dmz.lock` & manifest | Stable |
| **CLI Ergonomics (`init`, `bundle`, `import`, `shell`)** | **Implemented & Verified** | Stable |
| **Daemonless Sandbox Process Isolation** | **Implemented** (macOS Seatbelt, Linux, Windows) | Refining capability fallbacks |
| **AI Agent Supervisor & Ephemeral Scratchpads** | **Implemented** in `dmz-supervisor` | In active hardening |
| **System Package Capture (apt, brew, dnf)** | **Planned** | Roadmap v1.1 |
| **OCI / Docker Container Image Export** | **Planned** | Roadmap v1.2 |
| **Automated Target VM Pre-flight Bundle Testing** | **Planned** | Roadmap v1.2 |