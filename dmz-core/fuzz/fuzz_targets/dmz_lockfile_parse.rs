#![no_main]
use dmz_core::resolver::lockfile::DmzLockfile;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        if let Ok(lockfile) = serde_json::from_str::<DmzLockfile>(text) {
            let _ = DmzLockfile::compute_closure_hash(&lockfile.packages);
        }
    }
});

