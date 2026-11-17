# macOS Build Troubleshooting & Release Stripping Notes

1. **Troubleshooting — `rust-objcopy` `@rpath/libLLVM.dylib` warning (non-fatal):**
   - Root [`Cargo.toml`](../../Cargo.toml#L55) sets `[profile.release] strip = "debuginfo"`. On affected `rustup` macOS toolchains (e.g. `1.99.x`), `rustc` invokes `lib/rustlib/<host>/bin/rust-objcopy`, which can abort (`signal: 6 (SIGABRT)`, `Library not loaded: @rpath/libLLVM.dylib`) because `libLLVM.dylib` sits under top-level `lib/` rather than `lib/rustlib/<host>/lib/`. Release binaries still link and install cleanly (`exit code 0`, with unstripped debuginfo). To silence locally: run `rustup update stable` once patched upstream, set `DYLD_FALLBACK_LIBRARY_PATH="$(rustc --print sysroot)/lib"`, or symlink `lib/libLLVM.dylib` into `lib/rustlib/<host>/lib/`.

2. **Future Implementation (Nix-style `fixupPhase`-style post-build stripping):**
   - Decouple compilation from binary stripping rather than relying on `rustc`'s `rust-objcopy` (`-C strip=debuginfo`): set `strip = "none"` in [`Cargo.toml`](../../Cargo.toml) `[profile.release]` and run native OS strip tools (`strip -S` on macOS/Linux) during packaging in [`.github/workflows/release.yml`](../../.github/workflows/release.yml). This removes reliance on `rust-objcopy` and the CI `DYLD_FALLBACK_LIBRARY_PATH` workaround while keeping DMZ a pure-Cargo workspace.
