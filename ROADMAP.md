# Roadmap & Current Status

## Slated for Future Implementation:

### 1. Stack Compatibility & Verification
* **Automated Target VM Pre-flight Bundle Testing (Roadmap v1.2):**
  Automated spin-up of target-compatible lightweight VMs to execute pre-flight bundle compilation and verify runtime ABI/dynamic linker compatibility before deployment.
* **System Package Stack Analysis (Roadmap v1.1):**
  Discovery, capture, and verification of OS-level packages (`apt`, `brew`, `dnf`) alongside language package managers.
* **Deep ABI/Binary Header Analysis:**
  Static inspection of compiled native C/C++ addons in `node_modules` or Python wheels to check dynamic glibc/musl and architecture constraints before runtime.

### 2. Image Building
* **OCI / Docker Container Image Export (Roadmap v1.2):**
  * Converting the locked dependency closure and workspace into standard OCI layer tarballs, image manifests, and config JSON (`application/vnd.oci.image.manifest.v1+json`).
  * Creating runnable container images exportable to `docker load` or standard container registries without requiring an active Docker daemon.

### 3. Containerization & Process Sandboxing
* **True Linux Namespace Confinement (Refining Sandbox):**
  Transitioning `linux.rs` from direct process launching to unprivileged Linux User Namespaces (`unshare` with `CLONE_NEWUSER`, `CLONE_NEWNS`, `CLONE_NEWNET`) and `pivot_root`/`chroot`.
* **Windows Job Objects & Restricted Tokens:**
  Enforcing process tree memory/CPU limits and restricted access tokens natively on Windows (or bridging via lightweight WSL2).
* **Container Export:**
  Bridging daemonless sandbox closures into deployable OCI containers (see Image Building above).

---

## Status Matrix

| Capability | Current Status (v1.0.9) | Milestone Target | Primary Files |
| :--- | :--- | :--- | :--- |
| **Multi-Ecosystem Discovery** | **Implemented** (Rust, npm, pnpm, Yarn, Go, Python) | Stable | `mod.rs`, `handlers.rs` |
| **Native Ecosystem Verification** | **Implemented** (`dmz ecosystem verify`) | Stable | `handlers.rs`, `commands.rs` |
| **Target Platform Compatibility Check** | **Implemented** (Lockfile/manifest tags + import warning) | Stable | `importer.rs`, `lockfile.rs` |
| **Air-Gap Bundle Packaging (`.tar.zst`)** | **Implemented** (Target A, B, C; CLI & GUI) | Stable | `builder.rs`, `exporter.rs` |
| **Daemonless Process Sandbox** | **Implemented** (macOS Seatbelt, `dmz shell`) | Active hardening (Linux namespaces) | `macos.rs`, `linux.rs` |
| **System Package Capture (apt, brew)** | **Planned** | Roadmap v1.1 | `what-is-dmz.md` |
| **OCI / Container Image Export** | **Planned** | Roadmap v1.2 | `what-is-dmz.md` |
| **Automated Target VM Bundle Pre-flight** | **Planned** | Roadmap v1.2 | `what-is-dmz.md` |
