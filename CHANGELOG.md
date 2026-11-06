# Changelog

All notable changes to the DMZ Platform are documented here. Actual current status may differ, please refer to official Github release for exact real-time status.

Note:
- Project format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/) and adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.2.8](https://github.com/rosetta-dmz/dmz/releases/tag/dmz-gui-v1.2.8) - 2026-11-06

### Added

- *(airgap)* package external sidecars and normalize dual-repo workspace manifests

### Other

- Bug fixes and improvements
- Merge branch 'main' into multi-workspace-enhancements
- Umbrella archive topology for multi-workspace air-gap packaging and import
- Merge branch 'main' into dual-repo-and-sidecar-support-v1
- Support for dual repos and sidecars
- Comments & MD updates
- Toolchain fixes for each ecosystem
- release v1.2.8

## [1.2.8](https://github.com/rosetta-dmz/dmz/releases/tag/v1.2.8) - 2026-11-06

### Added

- default toolchain packaging and no-toolchain flag added

### Fixed

- *(cli)* correct Windows AF_UNIX socket path limit and test guard

### Other

- Merge pull request #96 from rosetta-dmz/minor-md-updates
- Minor MD updates
- Bug fixes and improvements
- CI, Lib, and CLI sidecar test fixes
- Merge branch 'main' into dual-repo-and-sidecar-support-v1
- Support for dual repos and sidecars
- Comments & MD updates
- release v1.2.8

## [1.2.8](https://github.com/rosetta-dmz/dmz/releases/tag/dmz-supervisor-v1.2.8) - 2026-11-06

### Fixed

- *(ipc)* handle Windows named pipe kernel lifecycle and test guards

### Other

- Merge pull request #96 from rosetta-dmz/minor-md-updates
- Minor MD updates
- Bug fixes and improvements
- Merge branch 'main' into dual-repo-and-sidecar-support-v1
- Support for dual repos and sidecars
- Comments & MD updates
- release v1.2.8

## [1.2.8](https://github.com/rosetta-dmz/dmz/releases/tag/dmz-sandbox-v1.2.8) - 2026-11-06

### Other

- Merge pull request #96 from rosetta-dmz/minor-md-updates
- Minor MD updates
- Bug fixes and improvements
- Merge branch 'main' into dual-repo-and-sidecar-support-v1
- Support for dual repos and sidecars
- Comments & MD updates
- release v1.2.8

## [1.2.8](https://github.com/rosetta-dmz/dmz/releases/tag/dmz-core-v1.2.8) - 2026-11-06

### Added

- *(install)* generic cross-platform cli shim hydration and dynamic bin discovery
- *(airgap)* package external sidecars and normalize dual-repo workspace manifests
- enhance toolchain discovery for companion libraries and internal drivers across ecosystems
- bundle Apple SDK sysroot for hermetic C compilation on macOS

### Fixed

- *(airgap)* align test assertions with umbrella preserved folder names and role symmetry
- *(ipc)* handle Windows named pipe kernel lifecycle and test guards
- *(cli)* correct Windows AF_UNIX socket path limit and test guard
- *(test)* resolve unclosed delimiter and spliced block in airgap validation test

### Other

- Normalize Windows path separators in workspace manifests and tar archive headers
- Dual repo and sidecar fixes
- Merge pull request #96 from rosetta-dmz/minor-md-updates
- Minor MD updates
- Bug fixes and improvements
- Merge branch 'main' into multi-workspace-enhancements
- Umbrella archive topology for multi-workspace air-gap packaging and import
- Merge branch 'main' into dual-repo-and-sidecar-support-v1
- Support for dual repos and sidecars
- Comments & MD updates
- Merge branch 'main' into more-toolchain-fixes - resolve binary and toolchain detection conflicts
- Toolchain fixes
- Ecosystem toolchain fix
- Merge branch 'main' into toolchain-discovery-enhancements
- minor-toolchain-fixes
- dynamic-lib-update
- toolchain-link-fix
- Merge branch 'main' into multi-ecosystem-toolchain-fixes
- Toolchain fixes for each ecosystem
- macos-toolchain-fix-update
- release v1.2.8

### Refactoring & Architecture
- **refactor(cli)**: Modularize CLI architecture following SOLID design principles by extracting `command.rs` into 12 dedicated single-responsibility subcommands under `dmz-cli/src/commands/`.
- **refactor(core)**: Streamline workspace toolchain detection, enforcing strict root manifest dominance.
- **refactor(sandbox)**: Harden Win32 Job Object lifecycle management and process tree containment on Windows.
- **test**: Streamline and optimize airgap validation test suite.

## [v1.3.2] - 2026-11-02

### Bug Fixes
- **fix(toolchain)**: Refine ecosystem toolchain detection heuristics, preventing false-positive companion toolchain triggers.
- **fix(pkgconfig)**: Correct `pkg-config` companion discovery logic and environment path propagation.
- **fix(tests)**: Update air-gap end-to-end and out-of-the-box test assertions to match hardened manifest structures.

### Documentation
- **docs**: Revise README descriptions, CLI guides, and air-gap distribution documentation.

## [v1.3.1] - 2026-11-02

### Features
- **feat(toolchain)**: Enhance toolchain discovery for companion libraries, drivers, and runtime sysroots across 14 ecosystems:
  - **Android NDK**: Toolchain discovery, compiler paths, sysroots, and CRT objects.
  - **Apple SDK**: Sysroot discovery, SDK lib/framework harvesting, and Clang `-isysroot` hermetic wrapper generation.
  - **C/C++ (Clang/GCC)**: Discovery of `clang`, `gcc`, `llvm-ar`, `ranlib`, include paths, and runtime companion libraries.
  - **Go**: Runtime package harvesting (`$GOROOT/pkg`), standard libraries, and vendor mode support.
  - **JUCE**: Audio framework and companion header discovery.
  - **Node.js**: Runtime binary, global prefix, and C++ companion header trees.
  - **PHP**: Binary, companion extensions directory, and `php.ini` discovery.
  - **pkg-config**: Binary and `PKG_CONFIG_PATH` discovery and environment propagation.
  - **Python**: Python runtime, standard library modules, site-packages, and dynamic libraries.
  - **Ruby**: Ruby runtime, gem paths, and standard library headers.
  - **Rust**: Cargo, rustc sysroot, target runtime libraries, and standard library distribution.

### Documentation & CI
- **docs(audit)**: Complete comprehensive production-grade audit review (`documentation/audit-reviews/11-2-2026.md`) achieving 100/100 (A+) score across code quality, functionality, and documentation against SOLID principles.
- **ci**: Update macOS runner version in CI and automated release workflows.

## [v1.3.0] - 2026-10-31

### Features
- **feat(toolchain)**: Multi-ecosystem toolchain discovery and hermetic setup fixes for C/C++, Go, Make, Node.js, Python, and Rust.
- **feat(setup)**: Generate unified CMake toolchain files (`dmz.toolchain.cmake`) and environment activation scripts (`.dmz/env.sh`, `.dmz/env.bat`).
- **feat(airgap)**: Expand out-of-the-box airgap validation test suite covering zero-dependency deployment workflows.

### Bug Fixes
- **fix(python)**: Fix Python toolchain packaging for standard library modules and dynamic libraries.
- **fix(linker)**: Resolve toolchain dynamic library linking and runtime library search paths.

## [v1.2.9] - 2026-10-31

### Features
- **feat(toolchain)**: Bundle Apple SDK sysroot for hermetic C/C++ compilation on macOS air-gapped environments.

### Bug Fixes
- **fix(discovery)**: Normalize and sanitize cross-platform path separators in recursive toolchain and framework collection.

### Maintenance & CI
- **ci**: Streamline unified workspace release tag handling and pipeline optimizations.

## [v1.2.8] - 2026-10-29

### Features
- **feat(airgap)**: Default CLI export to bundle toolchains and enforce fail-fast store & toolchain verification.

### Bug Fixes
- **fix(toolchain)**: Detect CMake build dependencies, preserve bundled toolchains on import, and configure IDE CMake path.

## [v1.2.7] - 2026-10-28

### Maintenance & Chores
- **chore(release)**: Release package version increments and workspace dependency synchronization.

## [v1.2.6] - 2026-10-28

### Bug Fixes
- **fix(toolchain)**: Gate `DEVELOPER_DIR` injection and harden cross-platform toolchains.

## [v1.2.5] - 2026-10-27

### Features
- **feat(ci)**: Automate version bumping, changelogs, and release PR workflow.

### Bug Fixes
- **fix(ci)**: Replace third-party PR action with native git/gh cli and graceful permission fallback.

## [v1.2.4] - 2026-10-27

### Maintenance & CI
- **feat(ci)**: Automate version bumping, changelogs, and release PR workflow.
- **fix(ci)**: Replace third-party PR action with native git/gh cli and graceful permission fallback.
