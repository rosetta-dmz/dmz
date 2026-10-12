# DMZ

> **Daemonless Sandbox & Air-Gapped Workspace Engine**

Named after military terminology, DMZ is a fully open-source high-performance, standalone sandbox and workspace management platform. It provides isolated developer environments, secure proxy-audited AI agent execution, and deterministic air-gapped closure imports with no host system dependencies. No pre-installed Python, Node, Docker, or system Git needed.

Of course, it's still recommended to use a daemon when necessary, however DMZ provides a lightweight solution to ensure interoperability and workspace/codebase reproducibility across Windows/Linux/MacOS regardless of connectivity, programming language, or dependencies used. We focus on eliminating the need to have a ridiculous amount of tools to manage the most important aspect of software development -- supply chain/SBOM visibility, management, and vulnerability analysis, stack-wide.

Yes, there's a desktop client and CLI because the dev community deserves better AND the best of both worlds.

---

## Features

- **Multi-Ecosystem Support:** Native discovery, dependency resolution, and sandbox verification for Rust (`Cargo.toml`), Node.js (`pnpm`, `yarn`, `npm`), Go (`go.mod`), and Python (`pyproject.toml`, `requirements.txt`).
- **Zero Host Dependencies:** Ships as a single self-contained binary embedding deterministic package handling for offline management.
- **Air-Gap Capable:** Pack, cryptographically verify, and extract complete `.tar.zst` dependency closures for completely offline environments.
- **Engine-Driven Verification:** Programmatically validates imported workspaces using native ecosystem rules via `dmz ecosystem verify`.
- **Supervisor & IPC Sockets:** Manages collateral stack services and exposes local API sockets for tool and IDE integrations.

---

## Installation & Quick Start

1. Head to the **[Releases](../../releases)** page.
2. Download the pre-built binary for your platform:
   * **Linux (`musl`):** `dmz-x86_64-unknown-linux-musl`
   * **macOS (Apple Silicon):** `dmz-aarch64-apple-darwin`
   * **Windows:** `dmz-x86_64-pc-windows-msvc.exe`
3. Make the binary executable (Linux/macOS):

   ```
   chmod +x dmz-*

DMZ's multi-ecosystem workflow automatically detects project ecosystems in your workspace root, handles offline resolution, and verifies builds securely via:

```
# Scan and discover active ecosystem stacks (Rust, Node, Go, Python, etc.)
dmz ecosystem discover

# Resolve workspace dependency closures into a deterministic lockfile
dmz resolve

# Fetch and cache locked dependencies into your secure local store
dmz fetch --store .dmz/store

# Verify workspace integrity using native engine rules
dmz ecosystem verify
```

## Air-Gap Import & Pipeline Usage

To unpack and verify an isolated closure archive offline:

```
dmz airgap import --archive dist/workspace_closure.tar.zst --store ./offline_store
```

To run the complete end-to-end multi-ecosystem air-gap pipeline script:

```
./airgap_pipeline.sh
```

## License

Distributed under the MIT License. See LICENSE for more information.
