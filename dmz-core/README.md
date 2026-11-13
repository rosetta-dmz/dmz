# `dmz-core`

Core library for DMZ deterministic workspace dependency resolution, hermetic store acquisition, single-pass zstd bundle export/import, offline ecosystem installation, bundled toolchain provisioning, and error diagnostics.

## Architecture

- **`resolver`** (`manifest`, `lockfile`, `fetch`):
  - Multi-ecosystem manifest and lockfile scanners for Rust (`Cargo.toml` / `Cargo.lock`), JavaScript (`package.json` / `package-lock.json` / `yarn.lock` / `pnpm-lock.yaml`), Python (`pyproject.toml` / `requirements.txt` / `uv.lock` / `poetry.lock`), Go (`go.mod` / `go.sum`), Java (`pom.xml` / `build.gradle` with `groupId:artifactId` coordinates), PHP (`composer.json`), and Ruby (`Gemfile` / `Gemfile.lock`).
  - Scans manifests via `ManifestScanner::scan_dir` and produces canonical `dmz.lock` (`DmzLockfile`) with SHA-256 package entries and deterministic closure hashes (`DmzLockfile::compute_closure_hash`).
  - Streams artifacts from upstream or custom registries (`DMZ_REGISTRY_CRATES_API`, `DMZ_REGISTRY_CRATES_DL`, `DMZ_REGISTRY_NPM`, `DMZ_REGISTRY_PYPI`, `DMZ_REGISTRY_GOLANG`) with real SHA-256 computation.

- **`archive`** (`exporter`, `importer`):
  - Single-pass reproducible `.tar.zst` export with symlink containment, deterministic timestamps (`SOURCE_DATE_EPOCH`), and multi-workspace/delta git history bundling.
  - Bounded extraction quotas (`unpack_tar_entry_contained_bounded`) preventing zip-bombs and unbounded in-memory JSON reads, path/symlink traversal rejection, closure SHA-256 verification, and fail-fast post-extraction offline installation via `InstallerEngine::install`.

- **`install`** (`archive`, `fast_clone`, `mappers`, `rust_vendor`):
  - Enforces strict 64-character lowercase hex SHA-256 verification in `InstallerEngine::verify_artifact_integrity` before unpacking store artifacts (rejecting placeholders like `"dummy"` or `"pending"`).
  - Bounded archive extractor (`SafeArchiveExtractor`) enforcing entry count, file size, and total extraction quotas.
  - Offline layout materializers:
    - **Cargo**: Extracts into `.cargo/vendor/<pkg>-<ver>`, writes per-file SHA-256 checksums into `.cargo-checksum.json`, and generates idempotent `.cargo/config.toml` vendoring configuration.
    - **npm / yarn**: Extracts/flattens into `node_modules/<pkg>` with store cache scoping and cross-platform `.bin` wrappers.
    - **pnpm**: Isolates versions in `node_modules/.pnpm/<sanitized>@<ver>/node_modules/<pkg>`, preserves multi-version root links, and wires dependency symlinks via `PnpmMapper::wire_pnpm_dependency_links`.
    - **PyPI**: Unpacks wheels and sdist archives into offline virtual environments and synthesizes console script shims and activation hooks.
    - **Go**: Strips `<module>@<version>/` archive prefixes into `vendor/<module>/` and maintains deduplicated `vendor/modules.txt` with `## explicit`.
    - **Maven/Gradle**: Materializes `<groupId>:<artifactId>` coordinates into `.m2/repository/<group/path>/<artifact>/<version>/`.
    - **Bundler / Composer**: Provisions `vendor/bundle` + `.bundle/config` and `vendor/<vendor>/<pkg>`.

- **`toolchain`** (`discovery`, `packager`, `setup`):
  - Discovers host compilers and runtimes (`rust`, `node`, `python`, `go`, `php`, `ruby`, `cc`, `cmake`, `ninja`, `make`, `pkgconfig`, `juce`, `android`, `apple`) using shared candidate ranking helpers (`select_preferred_candidate`, `select_preferred_verified`).
  - Packages relocatable bundles and configures offline `.dmz/toolchains/` permissions, activators, CMake toolchains, compiler wrappers, and IDE settings.

- **`workspace`**, **`vcs`**, & **`diagnostics`**:
  - Multi-workspace configuration (`dmz-workspace.toml`), inferred member command resolution (`WorkspaceError`), atomic git bundle VCS snapshots (`VcsError`), and structured typed error classification (`DiagnosticError::classify`) covering `ResolverError`, `ExportError`, `InstallError`, `ToolchainError`, `ImportError`, `VcsError`, and `WorkspaceError`.

## Verification & Testing

Run the hermetic test suite offline (live `pypi.org` integration tests are marked `#[ignore]` and replaced by local `TcpListener` fixtures for deterministic offline testing):

```bash
cargo test --workspace --no-fail-fast
```

Run live registry tests when online:

```bash
cargo test -p dmz-core -- --ignored
```

Run formatting and strict clippy checks:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings -D clippy::unwrap_used -D clippy::expect_used -D clippy::undocumented_unsafe_blocks -D clippy::indexing_slicing -D clippy::panic
```

## Fuzzing

Fuzz targets are located in `dmz-core/fuzz/fuzz_targets/`:

- `dmz_lockfile_parse`: `dmz.lock` JSON parsing & closure hashing
- `archive_extract`: `SafeArchiveExtractor::extract` against malformed `.tar.gz` / `.zip` streams
- `pep508_parse`: PEP 508 Python dependency requirement parser
- `pnpm_lock_parse`: `pnpm-lock.yaml` parser across `packages:` and `snapshots:`
- `pom_xml_parse`: Maven `pom.xml` coordinate extractor

Run with `cargo-fuzz` (nightly):

```bash
cargo +nightly fuzz run archive_extract --manifest-path dmz-core/Cargo.toml
```
