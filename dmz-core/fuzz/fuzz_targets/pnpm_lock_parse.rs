#![no_main]
use dmz_core::resolver::manifest::npm::NpmManifestScanner;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(dir) = tempfile::tempdir() {
        let lock_path = dir.path().join("pnpm-lock.yaml");
        if std::fs::write(&lock_path, data).is_ok() {
            let _ = NpmManifestScanner::parse_pnpm_lock(&lock_path);
        }
    }
});
