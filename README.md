# DMZ

> **Daemonless Sandbox & Air-Gapped Workspace Engine**

Named after military terminology, DMZ is a fully open-source high-performance, standalone sandbox and workspace management platform. It provides isolated developer environments, secure proxy-audited AI agent execution, and deterministic air-gapped closure imports with no host system dependencies. No pre-installed Python, Node, Docker, or system Git needed.

Of course, it's still recommended to use a daemon when necessary, however DMZ provides a lightweight solution to ensure interoperability and workspace/codebase reproducibility across Windows/Linux/MacOS regardless of connectivity, programming language, or dependencies used. We focus on eliminating the need to have a ridiculous amount of tools to manage the most important aspect of software development -- supply chain/SBOM visibility, management, and vulnerability analysis, stack-wide.

Yes, there's a desktop client and CLI because the dev community deserves better AND the best of both worlds.

---

## Features

* **Zero Host Dependencies:** Ships as a single self-contained binary embedding `libgit2` for deterministic repository cloning and offline management.
* **Air-Gap Capable:** Pack, cryptographically verify, and extract complete `.tar.zst` dependency closures for completely offline environments.
* **Network Proxy Whitelisting:** Enforces strict domain whitelists for AI agent outbound network calls.
* **Supervisor & IPC Sockets:** Manages collateral stack services (e.g., databases) and exposes local API sockets for IDE integrations (VS Code, JetBrains).
* **Granular Pinning & Telemetry:** Precise version control and low-overhead resource monitoring via an intuitive GUI control center.

---

## Installation & Quick Start

1. Head to the **[Releases](../../releases)** page.
2. Download the pre-built binary for your platform:
   * **Linux (`musl`):** `dmz-x86_64-unknown-linux-musl`
   * **macOS (Apple Silicon):** `dmz-aarch64-apple-darwin`
   * **Windows:** `dmz-x86_64-pc-windows-msvc.exe`
3. Make the binary executable (Linux/MacOS):
   ```bash
   chmod +x dmz-*

---

## Air-Gap Import Usage

To verify and unpack an isolated closure archive offline:

```
Bash
dmz import --archive closure.tar.zst --sha256 <expected_sha256_hash> --target /path/to/workspace
```

---

## License

Distributed under the MIT License. See LICENSE for more information.
