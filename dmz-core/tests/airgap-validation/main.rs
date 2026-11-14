//! Out-Of-The-Box Air-Gap Validation Integration Test Suite
//! Validates Root Manifest Dominance and ensures exported bundles are strictly complete
//! and ready out-of-the-box.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod cmake_toolchain;
mod common;
mod compilation_proof;
mod integrity_and_rollback;
mod minimal_closure_pruning;
mod stripped_path_verification;
