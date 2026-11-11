//! Nix / Bazel-Inspired Hermetic Hardening Integration Test Suite
//! Validates bit-for-bit reproducibility, single-pass hashes,
//! supply chain integrity tripwires, fast-clone reflink, and Zip Slip defenses.

mod reproducibility;
mod supply_chain;
mod fast_clone;
