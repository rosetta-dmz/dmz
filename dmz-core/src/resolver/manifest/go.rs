use super::types::{DependencySpec, Ecosystem, ManifestError, ManifestInfo};
use std::fs;
use std::path::Path;

pub struct GoManifestScanner;

impl GoManifestScanner {
    /// Scans `root_path` for `go.mod` (and optional `go.sum`) to extract Go module dependencies.
    pub fn scan(root_path: &Path, results: &mut Vec<ManifestInfo>) -> Result<(), ManifestError> {
        let go_mod_path = root_path.join("go.mod");
        if go_mod_path.is_file() {
            if let Some(graph_info) = Self::resolve_go_list_closure(root_path, &go_mod_path) {
                results.push(graph_info);
                return Ok(());
            }

            let mut info = Self::parse_go_mod(&go_mod_path)?;
            let go_sum_path = root_path.join("go.sum");
            if go_sum_path.is_file() {
                let sum_deps = Self::parse_go_sum(&go_sum_path);
                if !info.dependencies.is_empty() {
                    // Only allow go.sum to refine/confirm modules already declared in go.mod `require`,
                    // preventing historical or test-only entries in go.sum from bloating the closure.
                    let required_modules: std::collections::HashSet<(String, String)> = info
                        .dependencies
                        .iter()
                        .map(|d| (d.name.clone(), d.version_req.clone()))
                        .collect();
                    let filtered_sum: Vec<DependencySpec> = sum_deps
                        .into_iter()
                        .filter(|d| {
                            required_modules.contains(&(d.name.clone(), d.version_req.clone()))
                        })
                        .collect();
                    if !filtered_sum.is_empty() {
                        super::merge_locked_dependencies(&mut info.dependencies, filtered_sum);
                    }
                }
            }
            results.push(info);
        }
        Ok(())
    }

    /// Invokes `go list -m -json all` with `GOPROXY=off` when cached modules are available
    /// to obtain the exact MVS module graph closure without network calls.
    fn resolve_go_list_closure(root_path: &Path, go_mod_path: &Path) -> Option<ManifestInfo> {
        let go_bin = {
            let bundled = root_path.join(".dmz/toolchains/go/bin/go");
            if bundled.is_file() {
                bundled
            } else {
                which::which("go").ok()?
            }
        };

        let output = std::process::Command::new(go_bin)
            .args(["list", "-m", "-json", "all"])
            .env("GOPROXY", "off")
            .current_dir(root_path)
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let mut dependencies = Vec::new();
        let stream =
            serde_json::Deserializer::from_slice(&output.stdout).into_iter::<serde_json::Value>();
        for item in stream {
            let Ok(val) = item else {
                return None;
            };
            if val.get("Main").and_then(|v| v.as_bool()) == Some(true) {
                continue;
            }
            let effective = val.get("Replace").unwrap_or(&val);
            if effective
                .get("Path")
                .and_then(|v| v.as_str())
                .is_some_and(|p| p.starts_with('.'))
            {
                continue;
            }
            let Some(name) = val.get("Path").and_then(|v| v.as_str()) else {
                continue;
            };
            let Some(version) = val.get("Version").and_then(|v| v.as_str()) else {
                continue;
            };
            if !dependencies
                .iter()
                .any(|d: &DependencySpec| d.name == name && d.version_req == version)
            {
                dependencies.push(DependencySpec {
                    name: name.to_string(),
                    version_req: version.to_string(),
                    ecosystem: Ecosystem::Go,
                    is_dev: false,
                    dependencies: Vec::new(),
                });
            }
        }

        Some(ManifestInfo {
            ecosystem: Ecosystem::Go,
            path: go_mod_path.to_path_buf(),
            dependencies,
        })
    }

    /// Parses `go.mod` `require` directives (both single-line and parenthesized blocks).
    pub fn parse_go_mod(path: &Path) -> Result<ManifestInfo, ManifestError> {
        let content = fs::read_to_string(path)
            .map_err(|e| ManifestError::IoError(format!("Failed to read go.mod: {}", e)))?;
        let mut dependencies = Vec::new();
        let mut in_require_block = false;

        for raw_line in content.lines() {
            let without_comment = raw_line.split("//").next().unwrap_or("").trim();
            if without_comment.is_empty() {
                continue;
            }

            if in_require_block {
                if without_comment == ")" {
                    in_require_block = false;
                    continue;
                }
                Self::push_require_tokens(without_comment, &mut dependencies);
                continue;
            }

            if let Some(after_req) = without_comment.strip_prefix("require") {
                let rest = after_req.trim();
                if rest == "(" {
                    in_require_block = true;
                } else if let Some(inner) = rest.strip_prefix('(') {
                    if let Some(block_body) = inner.strip_suffix(')') {
                        Self::push_require_tokens(block_body.trim(), &mut dependencies);
                    } else {
                        in_require_block = true;
                    }
                } else if !rest.is_empty() {
                    Self::push_require_tokens(rest, &mut dependencies);
                }
            }
        }

        Ok(ManifestInfo {
            ecosystem: Ecosystem::Go,
            path: path.to_path_buf(),
            dependencies,
        })
    }

    /// Extracts module/version pairs from `go.sum` while stripping `/go.mod` metadata suffixes.
    pub fn parse_go_sum(path: &Path) -> Vec<DependencySpec> {
        let mut deps = Vec::new();
        let Ok(content) = fs::read_to_string(path) else {
            return deps;
        };

        for line in content.lines() {
            let mut parts = line.split_whitespace();
            let Some(module) = parts.next() else {
                continue;
            };
            let Some(raw_version) = parts.next() else {
                continue;
            };
            let version = raw_version.strip_suffix("/go.mod").unwrap_or(raw_version);
            if module.is_empty() || version.is_empty() {
                continue;
            }
            if !deps
                .iter()
                .any(|d: &DependencySpec| d.name == module && d.version_req == version)
            {
                deps.push(DependencySpec {
                    name: module.to_string(),
                    version_req: version.to_string(),
                    ecosystem: Ecosystem::Go,
                    is_dev: false,
                    dependencies: Vec::new(),
                });
            }
        }

        deps
    }

    fn push_require_tokens(spec: &str, out: &mut Vec<DependencySpec>) {
        let mut parts = spec.split_whitespace();
        let Some(module) = parts.next() else {
            return;
        };
        let Some(version) = parts.next() else {
            return;
        };
        if module.is_empty() || version.is_empty() {
            return;
        }
        if !out
            .iter()
            .any(|d| d.name == module && d.version_req == version)
        {
            out.push(DependencySpec {
                name: module.to_string(),
                version_req: version.to_string(),
                ecosystem: Ecosystem::Go,
                is_dev: false,
                dependencies: Vec::new(),
            });
        }
    }
}
