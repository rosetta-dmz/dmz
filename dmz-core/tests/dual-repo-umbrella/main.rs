// Dual-Repo Umbrella Air-Gap Integration Test Suite
// Validates master workspace packaging, multi-member isolation,
// offline hydration, and sibling sidecar coordination.

mod generic_sibling_roundtrip;
mod host_venv_exclusion;
mod manifest_scanner;
mod primary_python_sidecar_tauri;
mod primary_tauri_sidecar_python;

/// Writes a minimal valid tar.gz crate archive to `<store_dir>/<pkg>/<pkg>-<ver>.archive`
/// and returns its real SHA-256 hex digest.
pub(crate) fn write_mock_cargo_store_pkg(
    store_dir: &std::path::Path,
    pkg_name: &str,
    version: &str,
) -> String {
    let mut tar_builder = tar::Builder::new(Vec::new());
    let crate_toml = format!(
        "[package]\nname = \"{}\"\nversion = \"{}\"\n",
        pkg_name, version
    );
    let mut hdr = tar::Header::new_gnu();
    hdr.set_size(crate_toml.len() as u64);
    hdr.set_mode(0o644);
    hdr.set_cksum();
    tar_builder
        .append_data(
            &mut hdr,
            format!("{}-{}/Cargo.toml", pkg_name, version),
            crate_toml.as_bytes(),
        )
        .unwrap();
    let tar_bytes = tar_builder.into_inner().unwrap();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut gz, &tar_bytes).unwrap();
    let payload = gz.finish().unwrap();

    let pkg_dir = store_dir.join(pkg_name);
    std::fs::create_dir_all(&pkg_dir).unwrap();
    std::fs::write(
        pkg_dir.join(format!("{}-{}.archive", pkg_name, version)),
        &payload,
    )
    .unwrap();
    dmz_core::resolver::lockfile::DmzLockfile::hash_bytes(&payload)
}
