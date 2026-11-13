#![no_main]
use dmz_core::resolver::manifest::python::PythonManifestScanner;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = std::str::from_utf8(data) {
        for line in text.lines().take(64) {
            let _ = PythonManifestScanner::parse_pep508_requirement(line);
        }
    }
});

