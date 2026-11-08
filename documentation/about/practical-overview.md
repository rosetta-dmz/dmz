# Practical Overview

DMZ handles full multi-ecosystem workspace export and import by packaging the application source code alongside its entire dependency closure into a self-contained, air-gapped `.tar.zst` archive.

---

### Supported Ecosystems & Layout Materialization

When exporting via [AirGapExporter] and restoring via [AirGapImporter], [InstallerEngine] invokes dedicated ecosystem mappers in [mappers.rs] to reconstruct native project layouts completely offline:

| Ecosystem | Manifest Detected | Offline Store Materialization | Automated Offline Verification |
| :--- | :--- | :--- | :--- |
| **Rust / Cargo** | `Cargo.toml` | [CargoMapper] unpacks crates into `.cargo/vendor/` and configures `.cargo/config.toml` + checksums | `cargo check --offline` |
| **pnpm** | `package.json` + `pnpm-lock.yaml` | [PnpmMapper] builds `.pnpm/` virtual store and symlinks to `node_modules/` | `pnpm test --offline` |
| **npm / Yarn** | `package.json` + `yarn.lock` / lockfile | [NpmMapper]flattens tarballs into `node_modules/` | `npm test --no-audit` |
| **Python** | `pyproject.toml` / `requirements.txt` | [PyPiMapper] unpacks wheels into `.dmz/venv/.../site-packages` with `.dist-info` | `python3 -m py_compile .` |
| **Go** | `go.mod` | [GoMapper] extracts packages into `vendor/` and generates `vendor/modules.txt` | `go build -mod=readonly ./...` |
| **PHP** | `composer.json` | [ComposerMapper] populates `vendor/` | `composer validate --no-check-all` |

---

### How the Pipeline Operates

1. **Deterministic Lockfile Generation:** [ResolverEngine] scans all manifests across the workspace and records exact checksums and versions into [dmz.lock].
2. **Offline Store Fetch:** All remote dependencies are fetched into `.dmz/store` and hashed against `dmz.lock`.
3. **Packaging (`dmz bundle`):** [AirGapExporter] bundles the project files, `dmz.lock`, and `.dmz/store` into a single `.tar.zst` archive with an embedded cryptographic manifest (`airgap_manifest.json`).
4. **Hermetic Extraction & Linking (`dmz import`):** [AirGapImporter] cryptographically validates the closure signature, extracts the project into the destination folder, rebuilds the package layouts (such as `node_modules` or `.cargo/vendor`), and verifies stack integrity—requiring **zero external toolchains or internet access** on the target machine.