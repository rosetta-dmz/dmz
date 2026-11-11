// Dual-Repo Umbrella Air-Gap Integration Test Suite
// Validates master workspace packaging, multi-member isolation,
// offline hydration, and sibling sidecar coordination.

mod generic_sibling_roundtrip;
mod host_venv_exclusion;
mod manifest_scanner;
mod primary_python_sidecar_tauri;
mod primary_tauri_sidecar_python;
