//! Nix / Bazel-Inspired Hermetic Hardening Integration Test Suite
//! Validates bit-for-bit reproducibility, single-pass hashes,
//! supply chain integrity tripwires, fast-clone reflink, and Zip Slip defenses.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod fast_clone;
mod reproducibility;
mod supply_chain;
