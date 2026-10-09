# DMZ Workspace Build Automation
SHELL := /bin/bash
CARGO := cargo
TARGET_DIR := target
DIST_DIR := dist
VERSION := $(shell grep -m 1 'version =' Cargo.toml | cut -d '"' -f 2)

# Default Rust compilation flags for static linking
export RUSTFLAGS := -C target-feature=+crt-static -C link-arg=-s

.PHONY: all check build-cli build-gui linux-musl macos windows clean help

all: check build-cli build-gui

## check: Validate workspace compilation and run tests
check:
	@echo "==> Validating DMZ workspace crates..."
	$(CARGO) check --workspace --all-targets
	$(CARGO) test --workspace

## build-cli: Build release binary for the current host architecture (CLI)
build-cli:
	@echo "==> Compiling statically-linked dmz-cli (Release)..."
	mkdir -p $(DIST_DIR)
	$(CARGO) build --release -p dmz-cli
	cp $(TARGET_DIR)/release/dmz-cli $(DIST_DIR)/dmz
	@echo "==> Output written to $(DIST_DIR)/dmz"

## build-gui: Build release binary for the current host architecture (GUI)
build-gui:
	@echo "==> Compiling statically-linked dmz-gui (Release)..."
	mkdir -p $(DIST_DIR)
	$(CARGO) build --release -p dmz-gui
	cp $(TARGET_DIR)/release/dmz-gui $(DIST_DIR)/dmz-gui
	@echo "==> Output written to $(DIST_DIR)/dmz-gui"

## linux-musl: Cross-compile fully static Linux binaries against musl
linux-musl:
	@echo "==> Building static Linux x86_64 musl target..."
	rustup target add x86_64-unknown-linux-musl
	mkdir -p $(DIST_DIR)
	RUSTFLAGS="$(RUSTFLAGS)" $(CARGO) build --release --target x86_64-unknown-linux-musl -p dmz-cli
	RUSTFLAGS="$(RUSTFLAGS)" $(CARGO) build --release --target x86_64-unknown-linux-musl -p dmz-gui
	cp $(TARGET_DIR)/x86_64-unknown-linux-musl/release/dmz-cli $(DIST_DIR)/dmz-linux-x86_64
	cp $(TARGET_DIR)/x86_64-unknown-linux-musl/release/dmz-gui $(DIST_DIR)/dmz-gui-linux-x86_64

## macos: Build optimized macOS binaries (Apple Silicon / Intel)
macos:
	@echo "==> Building macOS release binaries..."
	mkdir -p $(DIST_DIR)
	$(CARGO) build --release -p dmz-cli
	$(CARGO) build --release -p dmz-gui
	cp $(TARGET_DIR)/release/dmz-cli $(DIST_DIR)/dmz-macos
	cp $(TARGET_DIR)/release/dmz-gui $(DIST_DIR)/dmz-gui-macos

## windows: Build static Windows binaries (.exe)
windows:
	@echo "==> Building static Windows x86_64 MSVC target..."
	rustup target add x86_64-pc-windows-msvc
	mkdir -p $(DIST_DIR)
	RUSTFLAGS="$(RUSTFLAGS)" $(CARGO) build --release --target x86_64-pc-windows-msvc -p dmz-cli
	RUSTFLAGS="$(RUSTFLAGS)" $(CARGO) build --release --target x86_64-pc-windows-msvc -p dmz-gui
	cp $(TARGET_DIR)/x86_64-pc-windows-msvc/release/dmz-cli.exe $(DIST_DIR)/dmz-windows-x86_64.exe
	cp $(TARGET_DIR)/x86_64-pc-windows-msvc/release/dmz-gui.exe $(DIST_DIR)/dmz-gui-windows-x86_64.exe

## bundle: Package release binaries into tar.zst air-gap archives
bundle: all
	@echo "==> Packaging DMZ binaries into release archives..."
	tar --zstd -cvf $(DIST_DIR)/dmz-v$(VERSION)-$(shell uname -s)-$(shell uname -m).tar.zst -C $(DIST_DIR) dmz dmz-gui

## clean: Wipe build targets and release directory
clean:
	@echo "==> Cleaning build directory..."
	$(CARGO) clean
	rm -rf $(DIST_DIR)

help:
	@echo "DMZ Engine Build Targets:"
	@grep -E '^## ' $(MAKEFILE_LIST) | sed -e 's/## //'