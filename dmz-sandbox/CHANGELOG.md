# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.2.8](https://github.com/rosetta-dmz/dmz/releases/tag/dmz-sandbox-v1.2.8) - 2026-10-29

### Added

- implement toolchain discovery and packaging system
- implementation of dependency installation functionality and streamlined CLI commands

### Fixed

- *(toolchain)* gate DEVELOPER_DIR injection and harden cross-platform toolchains
- *(sandbox)* add unprivileged fallback for Linux agent scratchpad mount
- *(sandbox)* add WindowsError variant to PlatformError enum, clean
- *(sandbox)* add WindowsError variant to PlatformError enum

### Other

- v1.2.2 - airgap assurance, sandboxed verification & multi-OS CI
- Dependency toolchain implementation
- Comment & .md updates
- Workspace import fixes and UI polishing
- Cargo TOML Update
- Linting fixes
- Sandbox & Install functionality updates
- V1 Build
