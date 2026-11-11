use dmz_core::resolver::manifest::ManifestScanner;
use std::fs;
use tempfile::tempdir;

#[test]
fn test_manifest_scanner_parses_pnpm_lock_uv_lock_and_pep508_extras() {
    let temp = tempdir().unwrap();
    let dir = temp.path();

    fs::write(
        dir.join("package.json"),
        r#"{"name": "app", "dependencies": {"clsx": "^2.0.0"}}"#,
    )
    .unwrap();
    fs::write(
        dir.join("pnpm-lock.yaml"),
        r#"lockfileVersion: '9.0'
packages:
  '@tauri-apps/cli@2.1.0':
    resolution: {integrity: sha512-aaa}
  clsx@2.1.1(react@18.3.1):
    resolution: {integrity: sha512-bbb}
"#,
    )
    .unwrap();
    fs::write(
        dir.join("requirements.txt"),
        "uvicorn[standard]>=0.29.0\npywin32>=300; sys_platform == 'win32'\n",
    )
    .unwrap();
    fs::write(
        dir.join("uv.lock"),
        r#"version = 1

[[package]]
name = "httpx"
version = "0.27.2"
"#,
    )
    .unwrap();

    let manifests = ManifestScanner::scan_dir(dir).unwrap();
    let deps: Vec<_> = manifests.into_iter().flat_map(|m| m.dependencies).collect();
    assert!(
        deps.iter()
            .any(|d| d.name == "@tauri-apps/cli" && d.version_req == "2.1.0"),
        "Expected @tauri-apps/cli@2.1.0 from pnpm-lock.yaml, got: {:?}",
        deps
    );
    assert!(
        deps.iter()
            .any(|d| d.name == "clsx" && d.version_req == "2.1.1"),
        "Expected clsx@2.1.1 with peer suffix stripped from pnpm-lock.yaml, got: {:?}",
        deps
    );
    assert!(
        deps.iter().any(|d| d.name == "uvicorn"),
        "Expected PEP 508 extras stripped from uvicorn[standard], got: {:?}",
        deps
    );
    assert!(
        deps.iter()
            .any(|d| d.name == "httpx" && d.version_req == "0.27.2"),
        "Expected httpx@0.27.2 from uv.lock, got: {:?}",
        deps
    );
    #[cfg(not(target_os = "windows"))]
    assert!(
        !deps.iter().any(|d| d.name == "pywin32"),
        "Expected sys_platform == 'win32' marker to be filtered on non-Windows"
    );
}

