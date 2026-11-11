use super::types::{DependencySpec, Ecosystem, ManifestError, ManifestInfo};
use std::fs;
use std::path::Path;

pub struct OtherManifestScanner;

impl OtherManifestScanner {
    pub fn scan(root_path: &Path, results: &mut Vec<ManifestInfo>) -> Result<(), ManifestError> {
        // --- PHP Composer ---
        let composer_path = root_path.join("composer.json");
        if composer_path.exists() {
            results.push(Self::parse_composer_json(&composer_path)?);
        }

        // --- Ruby Bundler ---
        let gemfile_path = root_path.join("Gemfile");
        if gemfile_path.exists() {
            results.push(Self::parse_gemfile(&gemfile_path)?);
        }

        // --- Java Maven ---
        let pom_path = root_path.join("pom.xml");
        if pom_path.exists() {
            results.push(Self::parse_pom_xml(&pom_path)?);
        }

        Ok(())
    }

    pub fn parse_composer_json(path: &Path) -> Result<ManifestInfo, ManifestError> {
        let content = fs::read_to_string(path)
            .map_err(|e| ManifestError::IoError(format!("Failed to read composer.json: {}", e)))?;
        let value: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| ManifestError::ParseError(format!("Invalid composer.json: {}", e)))?;

        let mut dependencies = Vec::new();
        if let Some(require) = value.get("require").and_then(|r| r.as_object()) {
            for (name, val) in require {
                if name == "php" || name.starts_with("ext-") {
                    continue;
                }
                dependencies.push(DependencySpec {
                    name: name.clone(),
                    version_req: val.as_str().unwrap_or("*").to_string(),
                    ecosystem: Ecosystem::Composer,
                    is_dev: false,
                });
            }
        }

        Ok(ManifestInfo {
            ecosystem: Ecosystem::Composer,
            path: path.to_path_buf(),
            dependencies,
        })
    }

    pub fn parse_gemfile(path: &Path) -> Result<ManifestInfo, ManifestError> {
        let content = fs::read_to_string(path)
            .map_err(|e| ManifestError::IoError(format!("Failed to read Gemfile: {}", e)))?;
        let mut dependencies = Vec::new();

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with("gem ") {
                let parts: Vec<&str> = line.split(',').collect();
                if let Some(gem_part) = parts.first() {
                    let name = gem_part
                        .trim_start_matches("gem")
                        .trim()
                        .trim_matches(|c| c == '\'' || c == '"');
                    let version_req = parts
                        .get(1)
                        .map(|v| v.trim().trim_matches(|c| c == '\'' || c == '"'))
                        .unwrap_or("*")
                        .to_string();

                    dependencies.push(DependencySpec {
                        name: name.to_string(),
                        version_req,
                        ecosystem: Ecosystem::Bundler,
                        is_dev: false,
                    });
                }
            }
        }

        Ok(ManifestInfo {
            ecosystem: Ecosystem::Bundler,
            path: path.to_path_buf(),
            dependencies,
        })
    }

    pub fn parse_pom_xml(path: &Path) -> Result<ManifestInfo, ManifestError> {
        let content = fs::read_to_string(path)
            .map_err(|e| ManifestError::IoError(format!("Failed to read pom.xml: {}", e)))?;
        let mut dependencies = Vec::new();

        for chunk in content.split("<dependency>") {
            if let Some(end_idx) = chunk.find("</dependency>") {
                let dep_block = &chunk[..end_idx];
                let artifact_id = Self::extract_xml_tag(dep_block, "artifactId");
                let version =
                    Self::extract_xml_tag(dep_block, "version").unwrap_or_else(|| "*".to_string());

                if let Some(artifact) = artifact_id {
                    dependencies.push(DependencySpec {
                        name: artifact,
                        version_req: version,
                        ecosystem: Ecosystem::MavenGradle,
                        is_dev: false,
                    });
                }
            }
        }

        Ok(ManifestInfo {
            ecosystem: Ecosystem::MavenGradle,
            path: path.to_path_buf(),
            dependencies,
        })
    }

    pub fn extract_xml_tag(block: &str, tag: &str) -> Option<String> {
        let open = format!("<{}>", tag);
        let close = format!("</{}>", tag);
        let start = block.find(&open)? + open.len();
        let end = block[start..].find(&close)? + start;
        Some(block[start..end].trim().to_string())
    }
}

