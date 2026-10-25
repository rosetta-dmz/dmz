# DMZ

> **Daemonless Sandbox & Air-Gapped Workspace Engine**

Named after military terminology, DMZ is a fully open-source, daemonless workspace management and standalone sandbox platform. It provides isolated developer environments, secure proxy-audited AI agent execution, and deterministic air-gapped closure imports with no host system dependencies. No pre-installed Python, Node, Docker, or system Git needed.

Of course, it's still recommended to use a daemon when necessary (Docker, Podman, etc.), however DMZ provides a lightweight solution to ensure interoperability and workspace/codebase reproducibility across Windows/Linux/MacOS regardless of connectivity, programming language, or dependencies used. DMZ focuses on eliminating the need to have a ridiculous amount of tools to manage the most important aspects of software development -- reproducibility, SBOM management, and vulnerability analysis across your stack. A lot of ground work was done to ensure a simple and seamless GUI experience as well for those who prefer fast and minimal button workflows.

## Features

- **Multi-Ecosystem Support:** Native discovery, dependency resolution, and sandbox verification for Rust (`Cargo.toml`), Node.js (`pnpm`, `yarn`, `npm`), Go (`go.mod`), and Python (`pyproject.toml`, `requirements.txt`).
- **Zero Host Dependencies:** Ships as a single self-contained binary embedding deterministic package handling for offline management.
- **Air-Gap Capable:** Pack, cryptographically verify, and extract complete `.tar.zst` dependency closures for completely offline environments.
- **Native Toolchain Packaging:** Optionally packages host compiler and runtime binaries (`cargo`, `rustc`, `node`, `pnpm`, `python3`, `go`, `php`, `ruby`) directly into bundles (`--with-toolchain`) so air-gapped target machines without pre-installed compilers can compile and run out of the box.
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

```bash
chmod +x dmz-*  
```

---

### MacOS GUI Install Note:

Apple designed modern macOS (from Catalina through Sequoia and beyond) to hard-block any application downloaded from the web unless it's notarized by Apple with a Developer ID. After placing the DMZ app in Applications, run the following command:

```bash
xattr -cr /Applications/DMZ.app
```

## Multi-Ecosystem Workflow
DMZ automatically detects project ecosystems in your workspace root, handles offline resolution, and verifies builds securely via:

```bash
# Scan and discover active ecosystem stacks (Rust, Node, Go, Python, etc.)
dmz ecosystem discover

# Resolve workspace dependency closures into a deterministic lockfile
dmz resolve

# Fetch and cache locked dependencies into your secure local store
dmz fetch --store .dmz/store

# Verify workspace integrity using native engine rules
dmz ecosystem verify
```

## Air-Gap Packaging & Offline Toolchain Workflow

Export an entire workspace, its dependencies, and the active compiler toolchains in one shot:

```bash
# Package workspace code, locked dependencies, and host toolchains
dmz bundle --with-toolchain --output dist/workspace_closure.tar.zst
```

On the air-gapped target machine (even with no pre-installed Rust, Node, or Python), unpack and verify:

```bash
# Unpack workspace, local dependency store, and compiler toolchains
dmz import --archive dist/workspace_closure.tar.zst --workspace ./my-workspace

# Enter the isolated workspace with extracted toolchains automatically in PATH
cd my-workspace
source .dmz/env.sh       # Or run inside an isolated sandbox via: dmz shell

# Immediately compile and test using the bundled toolchains
cargo run -p dmz-gui
```

## License

Distributed under the MIT License. See LICENSE for more information.
