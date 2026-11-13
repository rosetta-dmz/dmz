#![no_main]
use dmz_core::resolver::manifest::other::OtherManifestScanner;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(dir) = tempfile::tempdir() {
        let pom_path = dir.path().join("pom.xml");
        if std::fs::write(&pom_path, data).is_ok() {
            let _ = OtherManifestScanner::parse_pom_xml(&pom_path);
        }
    }
});
