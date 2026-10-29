# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.2.8](https://github.com/rosetta-dmz/dmz/releases/tag/dmz-core-v1.2.8) - 2026-10-29

### Added

- default toolchain packaging and no-toolchain flag added
- project documentation + GUI version control provenance tracking
- add ecosystems & introduce build toolchain discovery handlers
- implement toolchain discovery and packaging system
- implementation of dependency installation functionality and streamlined CLI commands
- implement multi-ecosystem engine verification and prepare v1.0.0 release
- Added multi-ecosystem support with ecosystem registry and handlers.
- Implemented air-gap export and import functionality

### Fixed

- *(toolchain)* gate DEVELOPER_DIR injection and harden cross-platform toolchains
- *(ci)* add linux gui deps, fix clippy lint, and resolve windows test compatibility
- *(toolchain)* deduplicate toolchains field and resolve merge conflicts with main
- *(core)* sanitize semver ranges, support scoped npm packages, and fix store export paths

### Other

- CHANGELOG & release updates
- *(release)* bump workspace version to 1.2.7
- *(release)* bump workspace version to 1.2.6
- *(release)* bump workspace version to 1.2.5
- *(release)* bump workspace version to 1.2.4
- *(release)* bump workspace version to 1.2.3
- v1.2.2 - airgap assurance, sandboxed verification & multi-OS CI
- Merge branch 'main' into toolchain-detection-refactor
- Extract toolchain detection logic into dedicated detection module
- toolchain detection logic fix + e2e shell tests
- Merge pull request #45 from rosetta-dmz/toolchain-discovery-and-resolution-fixes
- Support for CMake, C/C++ compiler and Apple/Android toolchain discovery
- Merge branch 'main' into toolchain-and-gui-resolution
- Toolchain packaging and GUI updates
- Dependency toolchain implementation
- Comment & .md updates
- Version Control and UI Polishing
- Workspace import fixes and UI polishing
- Import/Export Fixes
- Path resolution and md fixes
- Cargo TOML Update
- CLI and GUI Updates
- Linting fixes
- Sandbox & Install functionality updates
- *(core)* added semver sanitization and operator unit tests, and update pipeline script
- Added air-gap export modes and update CLI documentation
- Implement fetch command to download and cache dependencies from lockfile
- UI updates and diagnostic functionality
- V1 Build
