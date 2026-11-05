# Roadmap

This document outlines the architectural roadmap, current implementation status, and upcoming milestones for the DMZ Platform.

---

## Current Platform Status (v1.3.3+)

DMZ is a zero-dependency, daemonless workspace packaging engine and environment manager designed for deterministic, air-gapped software development.

### Implemented Functionality

* **14-Ecosystem Discovery & Verification:**
  Full discovery, lockfile parsing, and offline validation across 14 ecosystems:
  * **Primary Runtimes & Languages:** Rust (`cargo`), Node.js (`npm`, `pnpm`, `yarn`), Python (`pip`, `venv`), Go, PHP (`composer`), Ruby (`bundler`).
  * **Native & Auxiliary Build Systems:** C/C++ (`clang`, `gcc`, `llvm-ar`, `ranlib`), CMake, Make, Ninja, `pkg-config`, JUCE audio framework.
  * **Platform SDKs:** Apple SDK (Darwin sysroot, framework harvesting), Android NDK (CRT objects, cross-compilers).
* **Hermetic Toolchain Packaging & Auto-Setup (`--with-toolchain`):**
  * Bundled toolchains are packaged by default on export (`dmz bundle` / `dmz airgap export`), with `--no-toolchain` opt-out.
  * Captures runtime binaries, standard libraries, header trees, and companion dynamic libraries (`libtapi.dylib`, `libLTO.dylib` for macOS Mach-O `@rpath` relocation).
  * Automatically generates Clang hermetic wrappers, unified CMake toolchain configurations (`dmz.toolchain.cmake`), environment activators (`.dmz/env.sh`, `.dmz/env.bat`), and IDE settings (`.vscode/settings.json`, `.cargo/config.toml`).
  * Enforces **Root Manifest Dominance** to eliminate foreign toolchain hallucination.
* **Air-Gap Bundle Packaging (`.tar.zst`):**
  * Target A (Dependencies), Target B (App Source), and Target C (Unified Closure).
  * Streaming zstd compression, cryptographic SHA-256 sidecar generation, and tamper detection.
  * Fail-fast store verification, path traversal prevention, and transactional rollback on import failure.
* **VCS Provenance & Delta Synchronization:**
  * Embedded Git provenance, commit tracking, and dirty-state detection in `dmz.lock` and `airgap_manifest.json`.
  * Incremental delta packaging (`--since <commit>`) with base-commit mismatch guards.
  * Deterministic Git bundle v2 packaging (`--with-git-history`).
* **Daemonless Process Sandboxing:**
  * **macOS:** Apple Seatbelt profile confinement (`sandbox-exec`) with fine-grained path allowlists and Mach-O `@rpath` dynamic linker preservation.
  * **Windows:** Win32 Job Objects (`CreateJobObjectW`, `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) ensuring clean child process termination.
  * **Linux:** Unprivileged namespaces, `chroot`/`pivot_root`, and cgroup limits.
  * **Universal Cache Isolation:** Redirects build caches across all ecosystems into `.dmz/cache`.
* **AI Agent Supervisor & Egress Proxy:**
  * Background workspace supervisor thread with cross-platform IPC (Unix domain sockets / Windows Named Pipes).
  * Ephemeral scratchpads (`EphemeralFs`) with automatic teardown on drop.
  * Dedicated network proxy (`DomainWhitelist`) with wildcard domain routing and HTTP proxy auditing.
* **Native GUI Application (`dmz-gui`):**
  * Visual inspection of airgap closures, active sandboxes, supervisor lifecycle, and version control.
  * Multi-theme support (`mac_light`, `field_ops`) and persistent settings.

---

## Active & Upcoming Milestones

### Milestone v1.4 (Short-Term Focus)

* **Modular CLI Architecture (SOLID Refactoring):**
  * Extract monolithic `command.rs` into 12 dedicated, single-responsibility subcommand handlers in `dmz-cli/src/commands/`:
    `resolve`, `fetch`, `install`, `build`, `sandbox`, `airgap`, `bundle`, `import`, `shell`, `init`, `ecosystem`, `help`.
  * Decouple argument parsing from command execution to improve testability and maintainability.
* **Deep ABI & Dynamic Linker Analysis:**
  * Static inspection of native binaries, Mach-O load commands, and ELF `.dynamic` sections.
  * Pre-flight validation of glibc/musl minimum symbol versions and C++ standard library runtime compatibility in offline wheels and npm native addons before execution.
* **System Package Stack Analysis:**
  * Discovery, capture, and verification of OS-level packages (`apt`, `brew`, `dnf`) alongside language package managers.
  * Generating host package dependency declarations for air-gapped target deployment.
* **Linux Namespace Confinement Hardening:**
  * Direct unprivileged Linux User Namespaces (`unshare` with `CLONE_NEWUSER`, `CLONE_NEWNS`, `CLONE_NEWNET`, `CLONE_NEWPID`).
  * Robust fallback handling for locked-down kernel environments (e.g., restricted `kernel.unprivileged_userns_clone`).
* **Organize Test Suite** 

### Milestone v2.0 (Long-Term Vision)

* **OCI / Docker Container Image Export:**
  * Converting locked dependency closures and workspace snapshots into standard OCI layer tarballs, image manifests, and config JSON (`application/vnd.oci.image.manifest.v1+json`).
  * Creating runnable container images exportable to `docker load` or standard container registries without requiring an active Docker daemon.
* **Automated Target VM Pre-flight Bundle Testing:**
  * Automated spin-up of target-compatible lightweight micro-VMs (e.g. Firecracker or QEMU) to execute pre-flight bundle compilation and verify runtime ABI compatibility before physical deployment.
* **Cross-Architecture Native Emulation:**
  * Embedded user-mode emulation to validate cross-compiled bundles on non-matching host architectures.

---

## Status Matrix

| Capability | Current Status (v1.3.3+) | Milestone Target | Primary Files |
| :--- | :--- | :--- | :--- |
| **Multi-Ecosystem Discovery** | **Implemented** (14 ecosystems: Rust, Node, Python, Go, PHP, Ruby, CC, CMake, Make, Ninja, Apple SDK, Android NDK, JUCE, pkg-config) | Production Hardened | `dmz-core/src/ecosystem/`, `dmz-core/src/toolchain/discovery/` |
| **Native Toolchain Packaging** | **Implemented** (Bundled by default; Clang wrappers, CMake toolchain, env generation) | Production Hardened | `dmz-core/src/toolchain/packager.rs`, `dmz-core/src/toolchain/setup.rs` |
| **Native Ecosystem Verification** | **Implemented** (`dmz ecosystem verify` across all ecosystems) | Production Hardened | `dmz-core/src/ecosystem/handlers.rs`, `dmz-cli/src/commands/` |
| **Target Platform Compatibility Check** | **Implemented** (Lockfile/manifest tags + import warning & rollback) | Production Hardened | `dmz-core/src/archive/importer.rs`, `dmz-core/src/resolver/lockfile.rs` |
| **Air-Gap Bundle Packaging (`.tar.zst`)** | **Implemented** (Target A, B, C; CLI & GUI with SHA-256 sidecar) | Production Hardened | `dmz-core/src/archive/builder.rs`, `dmz-core/src/archive/exporter.rs` |
| **Embedded Git Provenance & Status** | **Implemented** (`dmz.lock` & `airgap_manifest.json` tracking) | Production Hardened | `dmz-core/src/vcs/provider.rs`, `dmz-core/src/resolver/lockfile.rs` |
| **Incremental Delta Air-Gap Bundles** | **Implemented** (`--since` delta diffs & import baseline check) | Production Hardened | `dmz-core/src/archive/exporter.rs`, `dmz-core/src/archive/importer.rs` |
| **Deterministic Git Bundle Inclusion** | **Implemented** (`--with-git-history` Git bundle v2 packaging) | Production Hardened | `dmz-core/src/vcs/provider.rs`, `dmz-core/src/archive/importer.rs` |
| **Daemonless Process Sandbox** | **Implemented** (macOS Seatbelt, Windows Job Objects, Linux namespaces) | Active Hardening | `dmz-sandbox/src/platform/` |
| **AI Agent Supervisor & Egress Proxy** | **Implemented** (Domain whitelist, wildcard proxy, ephemeral scratchpad) | Production Hardened | `dmz-sandbox/src/agent/`, `dmz-supervisor/` |
| **GUI Version Control & Airgap UI** | **Implemented** (Dedicated airgap, sandbox, VCS, and settings tabs) | Production Hardened | `dmz-gui/src/app/pages/` |
| **Modular CLI SOLID Architecture** | **In Progress** (Decomposing `command.rs` into 12 subcommand handlers) | Roadmap v1.4 | `dmz-cli/src/commands/` |
| **Deep ABI / Binary Header Analysis** | **Planned** (Static glibc/musl & dynamic linker symbol verification) | Roadmap v1.4 | `documentation/specifications/` |
| **System Package Capture (apt, brew, dnf)** | **Planned** (OS package stack capture & declaration) | Roadmap v1.4 | `documentation/what-is-dmz.md` |
| **OCI / Container Image Export** | **Planned** (Daemonless OCI manifest & layer tarball export) | Roadmap v2.0 | `documentation/specifications/` |
| **Automated Target VM Bundle Pre-flight** | **Planned** (Lightweight micro-VM pre-flight compilation) | Roadmap v2.0 | `documentation/what-is-dmz.md` |
