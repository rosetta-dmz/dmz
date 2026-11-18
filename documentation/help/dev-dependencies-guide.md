# Development Dependencies Guide (`--include-dev`)

> **Reference for understanding DMZ's "Include dev dependencies (`devDependencies`, `dev-dependencies`, test/lint extras)" option—how it works across ecosystems, and when to enable or ignore it.**

---

## What Are Dev Dependencies?

In most software ecosystems, dependencies are split into two categories:

1. **Runtime / Production Dependencies:** Libraries and packages required for your application to run in its deployed environment (for example, `express`, `tokio`, `serde`, `fastapi`).
2. **Development Dependencies:** Tooling and libraries needed only while developing, testing, linting, formatting, benchmarking, or building from source (for example, `pytest`, `eslint`, `prettier`, `ruff`, `vitest`, `criterion`, `typescript`, `vite`).

In DMZ, dev dependencies are controlled by the `include_dev` option, which defaults to **`false`** (disabled).

---

## Where the Setting Appears in DMZ

- **DMZ GUI (`Resolver` page):** The **"Include dev dependencies (devDependencies, dev-dependencies, test/lint extras)"** checkbox (`ResolverState::include_dev`, unchecked by default).
- **DMZ CLI:** The `--include-dev` flag (defaults to `false`), supported across:
  - `dmz resolve --include-dev` — includes dev dependencies in `dmz.lock`.
  - `dmz init --include-dev` — includes dev dependencies during workspace initialization.
  - `dmz bundle --include-dev` — includes dev dependencies when resolving, caching, and detecting compiler toolchains for an air-gap bundle.
  - `dmz airgap export --include-dev` — includes dev dependencies during toolchain detection when exporting.
  - `dmz airgap import --include-dev` — includes dev dependencies during toolchain validation when importing.

---

## Ecosystem Behavior

When `include_dev` is **`false`** (default), DMZ keeps production/runtime dependencies and prunes dev-only edges and their dev-only transitive closures. When `include_dev` is **`true`**, DMZ includes both production and dev dependency trees:

| Ecosystem | Always Included (`include_dev = false` & `true`) | Included Only When `include_dev = true` |
| :--- | :--- | :--- |
| **Rust (Cargo)** | `dependencies` and `build-dependencies` reachable from workspace members via `cargo metadata` | `[dev-dependencies]` edges and crates reachable only through `kind = "dev"` edges |
| **Node.js (npm / pnpm / yarn)** | `dependencies` and `optionalDependencies` in `package.json`, plus their production closure in `package-lock.json` / `pnpm-lock.yaml` / `yarn.lock` | `devDependencies` and dev-only subtrees in lockfiles |
| **Python (PyPI / uv / Poetry)** | `requirements.txt`, PEP 621 `[project.dependencies]`, `[tool.poetry.dependencies]`, and `main` group packages in `uv.lock` / `poetry.lock` | `[project.optional-dependencies]`, PEP 735 `[dependency-groups]`, `[tool.poetry.dev-dependencies]`, non-`main` `[tool.poetry.group.*]` tables, and dev/optional closures in `uv.lock` / `poetry.lock` |
| **PHP (Composer)** | `require` in `composer.json` | `require-dev` in `composer.json` |
| **Ruby (Bundler)** | Default gems in `Gemfile` | Gems in `:development` or `:test` groups (block or inline) in `Gemfile` |
| **Java (Maven)** | Compile/runtime dependencies in `pom.xml` | `<dependency>` entries with `<scope>test</scope>` in `pom.xml` |
| **Go** | `go.mod` / `go list -m -json all` closure | *(Not separated)* Go modules do not distinguish prod vs. dev sections in `go.mod`, so `is_dev` is always `false` |

> **Note on shared dependencies:** If a package is required by **both** a production dependency and a dev dependency, it stays in the production closure even when `include_dev` is `false`. Only packages that are exclusively reachable via dev dependencies are excluded.

---

## When to Ignore Dev Dependencies (Keep Unchecked — Default)

Leave **Include dev dependencies** unchecked (and omit `--include-dev`) when packaging for **production runtime or minimal air-gap delivery**:

- **Smaller bundles & faster transfers:** Test runners, linters, formatters, mock libraries, and documentation generators are excluded from `dmz.lock`, `.dmz/store`, and `.tar.zst` archives.
- **Reduced attack surface:** Fewer third-party packages are downloaded, hashed, stored, and audited inside the restricted environment.
- **Fewer cross-platform resolution issues:** Dev tools frequently pull in platform-specific native binaries (file watchers, headless browsers, test harnesses) that are unnecessary when targeting a different runtime OS/architecture.
- **Prebuilt artifacts:** If the target environment only runs compiled binaries or prebuilt static assets (for example, a compiled Rust binary or pre-bundled frontend `dist/`), dev dependencies are not needed.

### Example: Production Resolution & Bundle

```bash
# Default (include_dev = false): resolves runtime closure only
dmz resolve --workspace . --output dmz.lock
dmz bundle --workspace . --output prod-bundle.tar.zst
```

---

## When to Select Dev Dependencies (`--include-dev`)

Check **Include dev dependencies** (or pass `--include-dev`) when the **offline or DMZ environment needs to build from source, run tests, or run quality checks**:

- **Offline CI, testing, and linting:** Required if you will run `cargo test`, `npm test`, `pytest`, `eslint`, `ruff`, `vitest`, or similar commands inside an air-gapped sandbox.
- **Building from source in JS/TS projects:** Frameworks and build tools such as `typescript`, `vite`, `webpack`, `esbuild`, `tailwindcss`, and `@types/*` are conventionally listed under `devDependencies` in `package.json`. If the offline target compiles or bundles frontend/TypeScript code from source rather than running prebuilt output, you **must** include dev dependencies.
- **Air-gapped developer workstations:** Needed when provisioning an offline machine where engineers actively write, compile, test, and lint code.
- **Complete repository SBOM / audit:** Useful when you want `dmz.lock` to capture every declared dependency in the repository, including test and lint extras.

### Example: Offline CI / Source-Build Resolution & Bundle

```bash
# Include dev dependencies in the lockfile and air-gap bundle
dmz resolve --workspace . --output dmz.lock --include-dev
dmz bundle --workspace . --output dev-bundle.tar.zst --include-dev
```

On the air-gapped target:

```bash
dmz airgap import --archive dev-bundle.tar.zst --store ./offline_store --include-dev
```

---

## Practical Checklist

| Target Environment Activity | Recommended Setting |
| :--- | :--- |
| Running a prebuilt app or runtime service only | **Ignore** (unchecked / default) |
| Compiling a Rust/Go/Python service without running test suites | **Ignore** (unchecked / default) |
| Compiling or bundling JS/TS from source (`tsc`, `vite build`, `webpack`) | **Select** (`--include-dev`) |
| Running unit/integration tests or linters (`cargo test`, `pytest`, `eslint`) | **Select** (`--include-dev`) |
| Setting up an offline developer workstation | **Select** (`--include-dev`) |

> **Tip:** Keep the setting consistent across your pipeline (`resolve` $\rightarrow$ `bundle`/`export` $\rightarrow$ `import`). If you resolve a lockfile with `--include-dev`, pass `--include-dev` to `bundle`, `export`, and `import` as well so toolchain detection and validation align with the locked packages.

## Toolchains (`--toolchain-profile`) vs. Project Packages (`dmz.lock` / `--include-dev`)

DMZ enforces a strict supply-chain boundary aligned with reproducible hermetic build systems (such as Nix and Bazel) and SLSA provenance guarantees:

- **`ToolchainProfile` (`Minimal` vs. `Full`) controls first-party toolchain components only:**
  - `Minimal` *(default)* bundles only the core compiler/runtime, host/target sysroots, and the workspace's active package manager. Recommended for CI builds, reproducible verification, and production air-gap bundles.
  - `Full` adds first-party toolchain distribution components (`rust-analyzer`, `clippy`, `rustdoc`, `ctest`, `cpack`, headers, stdlib extras, cross-targets, and official Node package managers) for offline developer workstations or multi-target build servers.
  - **Host-global package caches are always excluded in both profiles** (Python `site-packages`/`dist-packages`, third-party global `node_modules`, Ruby `gems`/`site_ruby`, and PHP PEAR/PECL state) so builds never silently capture unhashed, non-reproducible machine state.
- **`dmz.lock` + `--include-dev` controls third-party packages:** Every third-party runtime or dev/test/lint dependency (`pytest`, `eslint`, `ruff`, `typescript`, `vite`, etc.) must be declared in your workspace manifest/lockfile and resolved through `dmz.lock` (checked via SHA-256 into `.dmz/store`).

### GUI Workflow Mapping

1. **Resolver tab (Third-party & dev packages):** Check **Include dev dependencies (devDependencies, dev-dependencies, test/lint extras)** when dev/test/lint packages are needed, then click **Scan & Resolve** to write `dmz.lock` and **Fetch Dependencies** to cache verified artifacts into `.dmz/store`.
2. **Air-Gap Manager tab — Export (First-party toolchains):** Enable **Package compiler toolchains into bundle** and choose **Minimal** or **Full**. Remember that **Bundle (Fetch)** only auto-resolves `dmz.lock` when `dmz.lock` does not yet exist; if `dmz.lock` already exists, both **Bundle (Fetch)** and **Export (Local)** reuse that existing `dmz.lock` for packages and apply **Include dev dependencies** only to toolchain detection. Re-run **Scan & Resolve** in **Resolver** first whenever you want to add or remove dev packages in `dmz.lock`.
3. **Air-Gap Manager tab — Import (Offline target):** Keep **Cryptographically verify closure hash** and **Install offline dependencies into workspace** enabled, and enable **Include dev dependencies** if the archive was built with dev dependencies so post-import validation matches.
