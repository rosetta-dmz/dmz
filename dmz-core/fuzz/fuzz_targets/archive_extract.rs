#![no_main]
use dmz_core::install::archive::SafeArchiveExtractor;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(dir) = tempfile::tempdir() {
        let archive_path = dir.path().join("fuzz.archive");
        let dest_dir = dir.path().join("out");
        if std::fs::write(&archive_path, data).is_ok() && std::fs::create_dir_all(&dest_dir).is_ok()
        {
            let _ = SafeArchiveExtractor::extract(&archive_path, &dest_dir);
        }
    }
});

