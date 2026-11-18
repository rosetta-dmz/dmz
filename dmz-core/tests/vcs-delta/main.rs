//! VCS Provenance, Git Bundles, & Delta Synchronization Test Suite
//! Validates git repository inspection, bundle packing/restoration,
//! historical sync, and mismatch fail-fast verification.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod common;
mod delta_sync;
mod git_bundle;
