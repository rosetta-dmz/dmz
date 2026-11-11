pub mod cargo;
pub mod npm;
pub mod other;
pub mod python;
pub mod types;

pub use types::{DependencySpec, Ecosystem, ManifestError, ManifestInfo};

use std::path::Path;

pub struct ManifestScanner;

impl ManifestScanner {
    pub fn scan_dir(root_path: &Path) -> Result<Vec<ManifestInfo>, ManifestError> {
        if !root_path.exists() || !root_path.is_dir() {
            return Err(ManifestError::IoError(format!(
                "Directory does not exist: {}",
                root_path.display()
            )));
        }

        let mut results = Vec::new();
        cargo::CargoManifestScanner::scan(root_path, &mut results)?;
        npm::NpmManifestScanner::scan(root_path, &mut results)?;
        python::PythonManifestScanner::scan(root_path, &mut results)?;
        other::OtherManifestScanner::scan(root_path, &mut results)?;
        Ok(results)
    }

    pub fn parse_pep508_requirement(raw_line: &str) -> Option<(String, String)> {
        python::PythonManifestScanner::parse_pep508_requirement(raw_line)
    }

    pub fn normalize_pypi_name(name: &str) -> String {
        python::PythonManifestScanner::normalize_pypi_name(name)
    }

    pub fn is_compatible_npm_platform_pkg(name: &str) -> bool {
        npm::NpmManifestScanner::is_compatible_npm_platform_pkg(name)
    }

    pub fn merge_locked_dependencies(
        direct: &mut Vec<DependencySpec>,
        locked: Vec<DependencySpec>,
    ) {
        merge_locked_dependencies(direct, locked);
    }
}

/// Merges locked transitive dependencies into direct dependencies, upgrading wildcard/range versions
/// on direct dependencies to their exact locked version and appending unseen transitive packages.
pub fn merge_locked_dependencies(direct: &mut Vec<DependencySpec>, locked: Vec<DependencySpec>) {
    for lock_dep in locked {
        if let Some(existing) = direct.iter_mut().find(|d| {
            d.ecosystem == lock_dep.ecosystem
                && (d.name == lock_dep.name
                    || (d.ecosystem == Ecosystem::PyPI
                        && python::PythonManifestScanner::normalize_pypi_name(&d.name)
                            == python::PythonManifestScanner::normalize_pypi_name(&lock_dep.name)))
        }) {
            if !lock_dep.version_req.is_empty() && lock_dep.version_req != "*" {
                existing.version_req = lock_dep.version_req;
            }
        } else {
            direct.push(lock_dep);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_cargo_manifest_scanning() {
        let temp = tempdir().unwrap();
        let cargo_toml = temp.path().join("Cargo.toml");
        fs::write(
            &cargo_toml,
            r#"
[workspace]
members = ["subpkg"]

[workspace.dependencies]
serde = "1.0.200"

[dependencies]
serde = { workspace = true }
tokio = "^1.38.0"
"#,
        )
        .unwrap();

        let sub_dir = temp.path().join("subpkg");
        fs::create_dir_all(&sub_dir).unwrap();
        fs::write(
            sub_dir.join("Cargo.toml"),
            r#"
[package]
name = "subpkg"
version = "0.1.0"

[dependencies]
serde = { workspace = true }
"#,
        )
        .unwrap();

        let manifests = ManifestScanner::scan_dir(temp.path()).unwrap();
        let cargo_manifests: Vec<_> = manifests
            .iter()
            .filter(|m| m.ecosystem == Ecosystem::Cargo)
            .collect();

        assert_eq!(cargo_manifests.len(), 2);
        let root_manifest = cargo_manifests
            .iter()
            .find(|m| m.path == cargo_toml)
            .unwrap();
        assert_eq!(root_manifest.dependencies.len(), 2);
        let serde_dep = root_manifest
            .dependencies
            .iter()
            .find(|d| d.name == "serde")
            .unwrap();
        assert_eq!(serde_dep.version_req, "1.0.200");
    }

    #[test]
    fn test_npm_manifest_and_locks() {
        let temp = tempdir().unwrap();
        let pkg_json = temp.path().join("package.json");
        fs::write(
            &pkg_json,
            r#"{
  "name": "test-app",
  "dependencies": {
    "react": "^18.3.1"
  },
  "devDependencies": {
    "typescript": "~5.4.5"
  }
}"#,
        )
        .unwrap();

        let pnpm_lock = temp.path().join("pnpm-lock.yaml");
        fs::write(
            &pnpm_lock,
            r#"
lockfileVersion: '9.0'
snapshots:
  'react@18.3.1':
    dependencies:
      loose-envify: 1.4.0
  'loose-envify@1.4.0': {}
"#,
        )
        .unwrap();

        let manifests = ManifestScanner::scan_dir(temp.path()).unwrap();
        let npm_manifest = manifests
            .iter()
            .find(|m| m.ecosystem == Ecosystem::Npm)
            .unwrap();
        assert_eq!(npm_manifest.dependencies.len(), 3);
        assert!(npm_manifest
            .dependencies
            .iter()
            .any(|d| d.name == "react" && d.version_req == "18.3.1"));
        assert!(npm_manifest
            .dependencies
            .iter()
            .any(|d| d.name == "loose-envify" && d.version_req == "1.4.0"));
    }

    #[test]
    fn test_pep508_requirement_parsing() {
        let r1 = ManifestScanner::parse_pep508_requirement("requests >= 2.28.0").unwrap();
        assert_eq!(r1.0, "requests");
        assert_eq!(r1.1, "2.28.0");

        let r2 = ManifestScanner::parse_pep508_requirement("uvicorn[standard] == 0.29.0").unwrap();
        assert_eq!(r2.0, "uvicorn");
        assert_eq!(r2.1, "0.29.0");

        let r3 = ManifestScanner::parse_pep508_requirement("pytest; extra == 'test'");
        assert!(r3.is_none());

        // Multi-constraint PEP 508 requirements (e.g. from requests requires_dist)
        let r4 = ManifestScanner::parse_pep508_requirement("charset_normalizer<4,>=2").unwrap();
        assert_eq!(r4.0, "charset_normalizer");
        assert_eq!(r4.1, "2");

        let r5 = ManifestScanner::parse_pep508_requirement("idna<4,>=2.5").unwrap();
        assert_eq!(r5.0, "idna");
        assert_eq!(r5.1, "2.5");

        let r6 = ManifestScanner::parse_pep508_requirement("urllib3<3,>=1.26").unwrap();
        assert_eq!(r6.0, "urllib3");
        assert_eq!(r6.1, "1.26");

        let r7 = ManifestScanner::parse_pep508_requirement("foo[extra]<4,>=2").unwrap();
        assert_eq!(r7.0, "foo");
        assert_eq!(r7.1, "2");

        let r8 = ManifestScanner::parse_pep508_requirement("bar < 4").unwrap();
        assert_eq!(r8.0, "bar");
        assert_eq!(r8.1, "*");

        let r9 = ManifestScanner::parse_pep508_requirement("bare-package").unwrap();
        assert_eq!(r9.0, "bare-package");
        assert_eq!(r9.1, "*");

        let r10 = ManifestScanner::parse_pep508_requirement("qux == 1.2.3, >= 1.0.0").unwrap();
        assert_eq!(r10.0, "qux");
        assert_eq!(r10.1, "1.2.3");

        let r11 = ManifestScanner::parse_pep508_requirement("package ~= 1.4.0").unwrap();
        assert_eq!(r11.0, "package");
        assert_eq!(r11.1, "1.4.0");

        let r12 =
            ManifestScanner::parse_pep508_requirement("direct @ https://example.com/foo.tar.gz");
        assert!(r12.is_none());
    }

    #[test]
    fn test_pypi_normalization() {
        assert_eq!(ManifestScanner::normalize_pypi_name("Pillow"), "pillow");
        assert_eq!(
            ManifestScanner::normalize_pypi_name("pytest_cov"),
            "pytest-cov"
        );
        assert_eq!(
            ManifestScanner::normalize_pypi_name("zope.interface"),
            "zope-interface"
        );
    }

    #[test]
    fn test_other_ecosystem_manifests() {
        let temp = tempdir().unwrap();
        fs::write(
            temp.path().join("composer.json"),
            r#"{"require": {"monolog/monolog": "3.5.0", "php": ">=8.2"}}"#,
        )
        .unwrap();

        fs::write(
            temp.path().join("Gemfile"),
            "source 'https://rubygems.org'\ngem 'rails', '~> 7.1.0'\n",
        )
        .unwrap();

        fs::write(
            temp.path().join("pom.xml"),
            r#"<project><dependencies><dependency><artifactId>guava</artifactId><version>33.0.0-jre</version></dependency></dependencies></project>"#,
        )
        .unwrap();

        let manifests = ManifestScanner::scan_dir(temp.path()).unwrap();
        assert!(manifests.iter().any(|m| m.ecosystem == Ecosystem::Composer));
        assert!(manifests.iter().any(|m| m.ecosystem == Ecosystem::Bundler));
        assert!(manifests
            .iter()
            .any(|m| m.ecosystem == Ecosystem::MavenGradle));
    }
}
