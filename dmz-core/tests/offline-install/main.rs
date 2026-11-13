//! DMZ Offline Dependency Installation Test Suite
//! Validates hermetic store extraction, layout materialization,
//! bin link synthesis, and per-member workspace containment.

mod cargo_workflow;
mod go_workflow;
mod npm_workflow;
mod pnpm_workflow;
mod python_workflow;
mod sidecar_isolation;
