# Developer Contributing Guide

## Contributing to DMZ

Thank you for your interest in contributing to the DMZ platform! We strive to maintain a secure, deterministic, and high-performance daemonless workspace ecosystem.

---

## Development Workflow & Standards

1. **Zero Host Dependencies:** All core functionality must remain self-contained. Do not introduce requirements for external toolchains unless strictly isolated via cargo features.
2. **Code Quality:** All code must compile cleanly without warnings under stable Rust.
3. **Testing:** Every new feature or bug fix must include corresponding unit or integration tests.

---

## Workspace Architecture

The workspace is organized into five primary crates:

* `dmz-cli:` Command-line interface orchestration, argument parsing, and user-facing terminal commands.

* `dmz-core:` Core resolution engines, lockfiles (dmz.lock), embedded libgit2 provider, and cryptographic air-gap packaging/verification pipeline.

* `dmz-gui:` egui-based control center for real-time telemetry and dependency version pinning.

* `dmz-sandbox:` Secure process isolation and sandbox execution engines.

* `dmz-supervisor:` Supervisor stack services, Unix/Named IPC socket server, and volatile network proxy auditor.

---

## Getting Started Locally

### Prerequisites
* **Rust Toolchain:** Stable channel (`rustup default stable`)

### Clone & Build
```bash
git clone [https://github.com/your-username/dmz.git](https://github.com/your-username/dmz.git)
cd dmz
cargo build --workspace --release
```

## Run the Test Suite

### Ensure all unit and integration tests pass successfully before submitting changes:

```
Bash
cargo test --workspace
```

### To run the air-gap packaging integration test specifically:

```
Bash
cargo test -p dmz-core --test air_gap_integration
```

## Submitting a Pull Request

1. Create a descriptive feature branch (git checkout -b feature/my-new-feature).
2. Commit your changes using conventional commit standards (feat: add custom proxy rule).
3. Push to your fork and open a Pull Request against main.
4. Ensure all CI checks and GitHub Actions workflows pass successfully.