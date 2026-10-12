# Changelog

## [v1.0.0] - 2026-10-12

### Added
- **Multi-Ecosystem Engine:** Native discovery, resolution, and engine-driven verification for Rust (`Cargo`), Node.js (`pnpm`, `yarn`, `npm`), Go (`go.mod`), and Python (`poetry`, `pip`).
- **Engine-Driven Ecosystem Verification:** Added `dmz ecosystem verify` CLI command backed by native `verification_command` traits across all supported language handlers.
- **Air-Gap Export & Import Pipelines:** Unified `.tar.zst` bundling with cryptographic store validation and workspace extraction.
- **Daemonless OS Sandbox Execution:** Isolated process execution with automatic fault classification and diagnostic error reporting.
- **Project Documentation:** Comprehensive developer contribution guide (`CONTRIBUTING-README.md`), master index (`docs/index.md`), and updated usage guides.

### Changed
- Replaced raw shell script checks in `airgap_pipeline.sh` with platform-driven `dmz ecosystem verify` CLI execution.
- Standardized E2E integration test mocks to strictly match valid multi-ecosystem lockfile schemas.