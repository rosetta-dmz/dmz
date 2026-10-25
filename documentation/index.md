# DMZ Platform Documentation Master Index

Welcome to the official documentation hub for **DMZ**. DMZ is a high-performance, modular platform built in Rust that provides zero-dependency sandboxing, cryptographic air-gapped closure distribution, and secure AI agent supervision.

---

## 1. Core Architecture & System Blueprints

* **[Architectural Blueprint] (`Architectural Blueprint.md`)** — Deep dive into the statically compiled core engine (targeting Linux musl, macOS, and Windows), native OS sandboxing primitives (Linux User Namespaces, macOS Seatbelt profiles, and Windows Job Objects), and the ephemeral AI Agent Workspace Supervisor.


* **[Functional Requirements (FRD)] (`Functional Requirements Document (FRD).md`)** — Comprehensive coverage of zero-dependency bootstrapping, language-agnostic dependency management, cryptographic air-gap packaging targets (Targets A, B, and C), and real-time telemetry.


* **[Non-Functional Requirements (NFRD)] (`Non-Functional Requirements Document (NFRD).md`)** — Performance criteria ensuring a zero idle footprint, native bare-metal speed, least-privilege security models, and complete air-gap offline compliance.

---

## 2. Workspace Architecture (Crates Overview)

The DMZ workspace is organized into **five primary crates**:

* **`dmz-cli`** — Command-line interface orchestration, argument parsing, and user-facing terminal commands.
* **`dmz-core`** — Core resolution engines, lockfiles (`dmz.lock`), embedded `libgit2` provider, and cryptographic air-gap packaging/verification pipeline.
* **`dmz-gui`** — egui-based control center for real-time telemetry, build progress monitoring, and dependency version pinning.
* **`dmz-sandbox`** — Secure process isolation and native containerless execution engines across Linux, macOS, and Windows.
* **`dmz-supervisor`** — Volatile workspace supervisor managing collateral stack services (Postgres/Redis), localized IDE IPC sockets, AI sandboxes with ephemeral `tmpfs` scratchpads, and outbound network proxy auditors[cite: 1, 2].

---

## 3. User & Developer Guides

* **[GUI User Workflow Guide](gui-user-workflow.md)** — Step-by-step GUI workflow instructions for workspace selection (open/save locations), dependency resolution, air-gap bundle export/import, package building, and sandbox testing.
* **[CLI User Workflow Guide](dev-user-workflow.md)** — Terminal-based resolution, air-gap bundling, and sandbox execution workflow.
* **[Version Control & Provenance Guide](version-control.md)** — Embedded `libgit2` repository provenance, Git bundle export/import, and incremental delta air-gap packaging.
* **[Air-Gap Packaging & Transfer Guide]** — Complete instructions on bundling, cryptographically signing, and offline-importing `.tar.zst` closure archives with zero host dependencies.
* **[Sandbox Execution & Isolation Guide]** — Managing process isolation, resource timeouts, and network-stripped execution boundaries.
* **[Network Proxy & Whitelist Guide]** — Configuring outbound domain whitelists and reviewing proxy audit trails for AI agent workloads.

---

## 4. Developer Guidelines & Roadmap Status

* **[Contributing Guide](`CONTRIBUTING.md`)** — Developer workflows, local setup instructions, workspace test suites, and pull request standards.

### Feature Implementation Status Matrix

| Feature / Module | Status | Notes |
| --- | --- | --- |
| **`dmz-core` Air-Gap Import/Export** | **Implemented & Tested** | Fully functional `.tar.zst` packaging and SHA-256 verification. |
| **Embedded `libgit2` & Resolution** | **Implemented** | Deterministic repository cloning and lockfile generation (`dmz.lock`). |
| **`dmz-sandbox` Native Isolation** | **Partially Implemented** | Linux namespace and macOS seatbelt integrations are active; advanced Windows Job Object/WSL2 orchestrations are in progress. |
| **`dmz-supervisor` & Proxy Auditor** | **Partially Implemented** | Core socket server and domain filtering exist; ephemeral `tmpfs` wipe-on-exit routines are actively being refined. |
| **`dmz-gui` Telemetry Control Center** | **In Progress** | UI framework is established; live CPU/RAM telemetry integration is pending final polish. |

---