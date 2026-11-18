//! Workspace Resolution & Topology Configuration Test Suite
//! Validates manifest scanning, dependency inheritance, range normalization,
//! multi-workspace manifests, socket boundaries, and ABI verification.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod minimal_closure;
mod multi_workspace_manifest;
mod single_workspace;
mod socket_and_abi;
