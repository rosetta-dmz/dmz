# DMZ Platform Documentation Master Index

Welcome to the official documentation hub for **DMZ** (Universal Daemonless Environment & Packaging Engine). DMZ is a high-performance, modular platform built in Rust that provides zero-dependency sandboxing, cryptographic air-gapped closure distribution, and secure AI agent supervision.

---

## 1. Core Architecture & System Blueprints

* **[Architectural Blueprint](architecture.md)** — Deep dive into the statically compiled core engine (targeting Linux musl, macOS, and Windows), native OS sandboxing primitives (Linux User Namespaces, macOS Seatbelt profiles, and Windows Job Objects), and the ephemeral AI Agent Workspace Supervisor.


* **[Functional Requirements (FRD)](https://www.google.com/search?q=frd.md)** — Comprehensive coverage of zero-dependency bootstrapping, language-agnostic dependency management, cryptographic air-gap packaging targets (Targets A, B, and C), and real-time telemetry.


* **[Non-Functional Requirements (NFRD)](https://www.google.com/search?q=nfrd.md)** — Performance criteria ensuring a zero idle footprint, native bare-metal speed, least-privilege security models, and complete air-gap offline compliance[cite: 3].

---

## 2. Workspace Architecture (Crates Overview)

The DMZ workspace is organized into **five primary crates**:

* **`dmz-cli`** — Command-line interface orchestration, argument parsing, and user-facing terminal commands.
* **`dmz-core`** — Core resolution engines, lockfiles (`dmz.lock`), embedded `libgit2` provider, and cryptographic air-gap packaging/verification pipeline.
* **`dmz-gui`** — egui-based control center for real-time telemetry, build progress monitoring, and dependency version pinning.
* **`dmz-sandbox`** — Secure process isolation and native containerless execution engines across Linux, macOS, and Windows.
* **`dmz-supervisor`** — Volatile workspace supervisor managing collateral stack services (Postgres/Redis), localized IDE IPC sockets, AI sandboxes with ephemeral `tmpfs` scratchpads, and outbound network proxy auditors[cite: 1, 2].

---

## 3. Command-Line Interface (CLI) Guides

* **[Air-Gap Packaging & Transfer Guide](https://www.google.com/search?q=cli-airgap.md)** — Complete instructions on bundling, cryptographically signing, and offline-importing `.tar.zst` closure archives with zero host dependencies[cite: 1, 2].
* **[Sandbox Execution & Isolation Guide](https://www.google.com/search?q=cli-sandbox.md)** — Managing process isolation, resource timeouts, and network-stripped execution boundaries.
* **[Network Proxy & Whitelist Guide](https://www.google.com/search?q=cli-proxy.md)** — Configuring outbound domain whitelists and reviewing proxy audit trails for AI agent workloads[cite: 1, 2].

---

## 4. Developer Guidelines & Roadmap Status

* **[Contributing Guide (`CONTRIBUTING.md`)](https://www.google.com/search?q=CONTRIBUTING.md)** — Developer workflows, local setup instructions, workspace test suites, and pull request standards.

### Feature Implementation Status Matrix

| Feature / Module | Status | Notes |
| --- | --- | --- |
| **`dmz-core` Air-Gap Import/Export** | **Implemented & Tested** | Fully functional `.tar.zst` packaging and SHA-256 verification. |
| **Embedded `libgit2` & Resolution** | **Implemented** | Deterministic repository cloning and lockfile generation (`dmz.lock`). |
| **`dmz-sandbox` Native Isolation** | **Partially Implemented** | Linux namespace and macOS seatbelt integrations are active; advanced Windows Job Object/WSL2 orchestrations are in progress. |
| **`dmz-supervisor` & Proxy Auditor** | **Partially Implemented** | Core socket server and domain filtering exist; ephemeral `tmpfs` wipe-on-exit routines are actively being refined. |
| **`dmz-gui` Telemetry Control Center** | **In Progress** | UI framework is established; live CPU/RAM telemetry integration is pending final polish. |

---