//! DMZ Offline Dependency Installation Test Suite
//! Validates hermetic store extraction, layout materialization,
//! bin link synthesis, and per-member workspace containment.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod cargo_workflow;
mod go_workflow;
mod npm_workflow;
mod pnpm_workflow;
mod python_workflow;
mod sidecar_isolation;
