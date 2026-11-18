use super::super::types::{
    DiscoveredToolchain, ToolchainDiscoveryContext, ToolchainEcosystem, ToolchainProfile,
};
use super::super::ToolchainError;
use super::{
    collect_files_recursive, emit_non_relocatable_notice, is_macos_usr_bin_stub,
    is_non_standalone_path, push_system_bin_candidates, push_unique_path, redact_home_prefix,
    resolve_prefix_root, select_strict_standalone_verified, ToolchainDiscoverer,
};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tracing::info;

const NODE_MANAGERS: &[&str] = &["npm", "npx", "pnpm", "yarn", "corepack"];

/// Discovers host Node.js runtime and companion package managers (npm, npx, pnpm, yarn).
#[derive(Debug, Default, Clone)]
pub struct NodeDiscoverer;

impl ToolchainDiscoverer for NodeDiscoverer {
    fn ecosystem(&self) -> ToolchainEcosystem {
        ToolchainEcosystem::Node
    }

    fn discover(&self) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        self.discover_with_context(&ToolchainDiscoveryContext::default())
    }

    fn discover_with_context(
        &self,
        ctx: &ToolchainDiscoveryContext,
    ) -> Result<Option<DiscoveredToolchain>, ToolchainError> {
        let node_candidates = collect_node_candidates();
        if node_candidates.is_empty() {
            return Ok(None);
        }

        let Some((node_bin, node_root)) =
            select_strict_standalone_verified(node_candidates, resolve_node_root_for_candidate)
        else {
            return Ok(None);
        };

        let version = probe_node_version(&node_bin);
        let redacted_root = redact_home_prefix(&node_root);
        info!(
            node_root = %redacted_root,
            version = %version,
            profile = %ctx.profile,
            "Discovered host Node.js toolchain"
        );

        let selected_managers = select_node_managers(ctx);
        let lib_managers = expand_managers_with_corepack(&node_bin, &selected_managers);
        let include_headers = should_include_node_headers(ctx);
        let bin_files = collect_node_binaries_for(&node_bin, &selected_managers);
        let mut lib_files =
            collect_node_libs_for(&node_root, ctx.profile, &lib_managers, include_headers);
        collect_external_manager_libs(&bin_files, &node_root, &selected_managers, &mut lib_files);

        Ok(Some(DiscoveredToolchain {
            ecosystem: ToolchainEcosystem::Node,
            version,
            root_dir: node_root,
            bin_files,
            lib_files,
        }))
    }
}

fn collect_context_node_roots(ctx: &ToolchainDiscoveryContext) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    let push_unique = |roots: &mut Vec<PathBuf>, p: PathBuf| {
        if p.is_dir() && !roots.iter().any(|existing| existing == &p) {
            roots.push(p);
        }
    };
    for root in &ctx.member_roots {
        push_unique(&mut roots, root.clone());
    }
    if let Some(ws) = ctx.workspace_root.as_deref() {
        push_unique(&mut roots, ws.to_path_buf());
    }
    let base_roots = roots.clone();
    for base in base_roots {
        for node_dir in crate::workspace::discover_node_project_roots(&base) {
            push_unique(&mut roots, node_dir);
        }
    }
    roots
}

fn read_package_manager_field(dir: &Path) -> Option<String> {
    let pkg_json = dir.join("package.json");
    let content = fs::read_to_string(pkg_json).ok()?;
    let val: serde_json::Value = serde_json::from_str(&content).ok()?;
    val.get("packageManager")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn should_include_node_headers(ctx: &ToolchainDiscoveryContext) -> bool {
    ctx.profile == ToolchainProfile::Full
        || ctx.ecosystems.contains(&ToolchainEcosystem::Cc)
        || collect_context_node_roots(ctx)
            .iter()
            .any(|w| w.join("binding.gyp").is_file())
}

fn select_node_managers(ctx: &ToolchainDiscoveryContext) -> Vec<&'static str> {
    if ctx.profile == ToolchainProfile::Full {
        return NODE_MANAGERS.to_vec();
    }

    let roots = collect_context_node_roots(ctx);
    if roots.is_empty() {
        return vec!["npm", "npx"];
    }

    let mut uses_pnpm = false;
    let mut uses_yarn = false;
    let mut uses_npm = false;

    for root in &roots {
        let pm_spec = read_package_manager_field(root);
        let pm_name = pm_spec
            .as_deref()
            .and_then(|s| s.split('@').next())
            .unwrap_or("");
        if root.join("pnpm-lock.yaml").is_file()
            || root.join("pnpm-workspace.yaml").is_file()
            || pm_name == "pnpm"
        {
            uses_pnpm = true;
        }
        if root.join("yarn.lock").is_file()
            || root.join(".yarnrc.yml").is_file()
            || root.join(".yarnrc").is_file()
            || pm_name == "yarn"
        {
            uses_yarn = true;
        }
        if root.join("package-lock.json").is_file() || pm_name == "npm" {
            uses_npm = true;
        }
    }

    if !uses_pnpm && !uses_yarn {
        uses_npm = true;
    }

    let mut managers = Vec::new();
    if uses_npm {
        managers.push("npm");
        managers.push("npx");
    }
    if uses_pnpm {
        managers.push("pnpm");
    }
    if uses_yarn {
        managers.push("yarn");
    }
    managers
}

fn path_targets_corepack(path: &Path) -> bool {
    let inspect_target = |candidate: &Path| -> bool {
        candidate
            .components()
            .any(|c| c.as_os_str() == std::ffi::OsStr::new("corepack"))
    };
    if let Ok(link_target) = fs::read_link(path) {
        if inspect_target(&link_target) {
            return true;
        }
    }
    if let Ok(canon) = fs::canonicalize(path) {
        if inspect_target(&canon) {
            return true;
        }
    }
    false
}

fn expand_managers_with_corepack<'a>(node_bin: &Path, selected: &[&'a str]) -> Vec<&'a str> {
    let mut expanded: Vec<&'a str> = selected.to_vec();
    let Some(bin_dir) = node_bin.parent().filter(|p| !p.as_os_str().is_empty()) else {
        return expanded;
    };
    let needs_corepack = selected
        .iter()
        .any(|mgr| path_targets_corepack(&bin_dir.join(mgr)));
    if needs_corepack && !expanded.contains(&"corepack") {
        expanded.push("corepack");
    }
    expanded
}

fn collect_node_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(home) = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|v| !v.is_empty())
    {
        let nvm_dir = PathBuf::from(&home).join(".nvm/versions/node");
        if let Ok(entries) = fs::read_dir(&nvm_dir) {
            for entry in entries.flatten() {
                let bin = entry.path().join("bin/node");
                if bin.is_file() {
                    push_unique_path(&mut candidates, bin);
                }
            }
        }
    }

    let mut standard_paths = vec![PathBuf::from("/usr/local/bin/node")];
    if !cfg!(target_os = "macos") {
        standard_paths.push(PathBuf::from("/usr/bin/node"));
    }
    for p in standard_paths {
        if p.is_file() {
            push_unique_path(&mut candidates, p);
        }
    }

    push_system_bin_candidates(&mut candidates, "node");
    candidates
}

fn resolve_node_root_for_candidate(node_bin: &Path) -> Option<PathBuf> {
    if is_macos_usr_bin_stub(node_bin) || !node_bin.is_file() {
        return None;
    }
    resolve_prefix_root(node_bin)
}

fn probe_node_version(node_bin: &Path) -> String {
    Command::new(node_bin)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "node".to_string())
}

fn collect_node_binaries_for(node_bin: &Path, managers: &[&str]) -> Vec<(PathBuf, String)> {
    let mut bin_files = vec![(node_bin.to_path_buf(), "bin/node".into())];
    let bin_dir = node_bin.parent().filter(|p| !p.as_os_str().is_empty());

    for mgr in managers {
        if let Some(dir) = bin_dir {
            let cp = dir.join(mgr);
            if cp.is_file() {
                let resolved = fs::canonicalize(&cp).unwrap_or_else(|_| cp.clone());
                if !is_non_standalone_path(&resolved)
                    || resolved.starts_with(dir.parent().unwrap_or(dir))
                {
                    bin_files.push((cp, format!("bin/{}", mgr)));
                    continue;
                }
                emit_non_relocatable_notice(&resolved);
            }
            if cfg!(windows) {
                let cmd_cp = dir.join(format!("{}.cmd", mgr));
                if cmd_cp.is_file() {
                    bin_files.push((cmd_cp, format!("bin/{}.cmd", mgr)));
                    continue;
                }
            }
        }
        if let Ok(found) = which::which(mgr) {
            let resolved = fs::canonicalize(&found).unwrap_or_else(|_| found.clone());
            if is_non_standalone_path(&found) || is_non_standalone_path(&resolved) {
                emit_non_relocatable_notice(&found);
                continue;
            }
            if is_macos_usr_bin_stub(&found) || is_macos_usr_bin_stub(&resolved) {
                continue;
            }
            let redacted = redact_home_prefix(&resolved);
            info!(
                manager = %mgr,
                path = %redacted,
                "Discovered standalone host Node package manager binary"
            );
            let name = found
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(mgr)
                .to_string();
            bin_files.push((resolved, format!("bin/{}", name)));
        }
    }

    bin_files
}

fn collect_external_manager_libs(
    bin_files: &[(PathBuf, String)],
    node_root: &Path,
    selected_managers: &[&str],
    lib_files: &mut Vec<(PathBuf, String)>,
) {
    let mut seen_rel: HashSet<String> = lib_files.iter().map(|(_, rel)| rel.clone()).collect();
    for (host_path, _) in bin_files {
        let canon = fs::canonicalize(host_path).unwrap_or_else(|_| host_path.clone());
        if canon.starts_with(node_root) {
            continue;
        }
        // If an external package manager shim resolves inside a standalone `lib/node_modules/<mgr>/`
        // (e.g., global pnpm or corepack install), include that module directory if allowed.
        for ancestor in canon.ancestors() {
            let Some(dir_name) = ancestor.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let is_allowed_mod = selected_managers.contains(&dir_name) || dir_name == "corepack";
            let in_node_modules = ancestor
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                == Some("node_modules");
            if is_allowed_mod && in_node_modules {
                if let Some(nm_parent) = ancestor.parent() {
                    let mut extra = Vec::new();
                    collect_files_recursive(ancestor, nm_parent, &mut extra, "lib/node_modules");
                    for (src, rel) in extra {
                        if seen_rel.insert(rel.clone()) {
                            lib_files.push((src, rel));
                        }
                    }
                }
                break;
            }
        }
    }
}

fn collect_node_libs_for(
    node_root: &Path,
    profile: ToolchainProfile,
    selected_managers: &[&str],
    include_headers: bool,
) -> Vec<(PathBuf, String)> {
    let mut lib_files = Vec::new();

    let node_modules_dir = node_root.join("lib/node_modules");
    if node_modules_dir.is_dir() {
        // Restrict `lib/node_modules` to official package managers selected for the active
        // profile (`NODE_MANAGERS` for `Full`, lockfile-matched managers for `Minimal`) so
        // arbitrary host `npm install -g` packages never leak into bundles.
        let _ = profile;
        let allowed_mods: HashSet<&str> = selected_managers.iter().copied().collect();
        if let Ok(entries) = fs::read_dir(&node_modules_dir) {
            for entry in entries.flatten() {
                let mod_name = entry.file_name().to_string_lossy().into_owned();
                let matches_mgr = allowed_mods.contains(mod_name.as_str())
                    || (mod_name == "npm" && allowed_mods.contains("npx"));
                if matches_mgr && entry.path().is_dir() {
                    collect_files_recursive(
                        &entry.path(),
                        &node_modules_dir,
                        &mut lib_files,
                        "lib/node_modules",
                    );
                }
            }
        }
    }

    if include_headers {
        let include_node = node_root.join("include/node");
        if include_node.is_dir() {
            collect_files_recursive(&include_node, node_root, &mut lib_files, "");
        }
    }

    lib_files
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_node_collects_headers_and_modules() {
        let temp = tempdir().unwrap();
        let node_root = temp.path().join("fake_node");
        let bin = node_root.join("bin");
        let inc = node_root.join("include/node");
        let nm = node_root.join("lib/node_modules/npm");

        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&inc).unwrap();
        fs::create_dir_all(&nm).unwrap();

        fs::write(bin.join("node"), "node").unwrap();
        fs::write(bin.join("npm"), "npm").unwrap();
        fs::write(bin.join("corepack"), "corepack").unwrap();
        fs::write(inc.join("node.h"), "#define NODE_H").unwrap();
        fs::write(nm.join("package.json"), "{}").unwrap();

        let mut bin_files = Vec::new();
        let mut lib_files = Vec::new();

        bin_files.push((bin.join("node"), "bin/node".into()));
        for mgr in &["npm", "npx", "pnpm", "yarn", "corepack"] {
            let cp = bin.join(mgr);
            if cp.exists() {
                bin_files.push((cp, format!("bin/{}", mgr)));
            }
        }

        let node_modules_dir = node_root.join("lib/node_modules");
        if node_modules_dir.exists() {
            collect_files_recursive(
                &node_modules_dir,
                &node_modules_dir,
                &mut lib_files,
                "lib/node_modules",
            );
        }
        let include_node = node_root.join("include/node");
        if include_node.exists() {
            collect_files_recursive(&include_node, &node_root, &mut lib_files, "");
        }

        let bin_names: Vec<String> = bin_files.into_iter().map(|(_, r)| r).collect();
        assert!(bin_names.contains(&"bin/npm".to_string()));
        assert!(bin_names.contains(&"bin/corepack".to_string()));

        let lib_names: Vec<String> = lib_files.into_iter().map(|(_, r)| r).collect();
        assert!(lib_names.contains(&"include/node/node.h".to_string()));
        assert!(lib_names.contains(&"lib/node_modules/npm/package.json".to_string()));
    }

    #[test]
    fn test_node_minimal_profile_selects_only_requested_package_manager(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let node_root = temp.path().join("node_home");
        let bin = node_root.join("bin");
        let inc = node_root.join("include/node");
        let nm = node_root.join("lib/node_modules");

        fs::create_dir_all(&bin)?;
        fs::create_dir_all(&inc)?;
        fs::create_dir_all(nm.join("npm"))?;
        fs::create_dir_all(nm.join("pnpm"))?;
        fs::create_dir_all(nm.join("corepack"))?;
        fs::create_dir_all(nm.join("global-third-party-tool"))?;
        fs::create_dir_all(nm.join("@scope/leaked-pkg"))?;

        fs::write(bin.join("node"), "node")?;
        fs::write(bin.join("npm"), "npm")?;
        fs::write(bin.join("pnpm"), "pnpm")?;
        fs::write(bin.join("corepack"), "corepack")?;
        fs::write(inc.join("node.h"), "#define NODE_H")?;
        fs::write(nm.join("npm/package.json"), "{}")?;
        fs::write(nm.join("pnpm/package.json"), "{}")?;
        fs::write(nm.join("corepack/package.json"), "{}")?;
        fs::write(nm.join("global-third-party-tool/package.json"), "{}")?;
        fs::write(nm.join("@scope/leaked-pkg/package.json"), "{}")?;

        let minimal_bins: Vec<String> = collect_node_binaries_for(&bin.join("node"), &["pnpm"])
            .into_iter()
            .map(|(_, r)| r)
            .collect();
        assert!(minimal_bins.contains(&"bin/node".to_string()));
        assert!(minimal_bins.contains(&"bin/pnpm".to_string()));
        assert!(!minimal_bins.contains(&"bin/npm".to_string()));
        assert!(!minimal_bins.contains(&"bin/corepack".to_string()));

        let minimal_libs: Vec<String> =
            collect_node_libs_for(&node_root, ToolchainProfile::Minimal, &["pnpm"], false)
                .into_iter()
                .map(|(_, r)| r)
                .collect();
        assert!(minimal_libs.contains(&"lib/node_modules/pnpm/package.json".to_string()));
        assert!(!minimal_libs.contains(&"lib/node_modules/npm/package.json".to_string()));
        assert!(!minimal_libs.contains(&"lib/node_modules/corepack/package.json".to_string()));
        assert!(!minimal_libs
            .iter()
            .any(|r| r.contains("global-third-party-tool") || r.contains("@scope/leaked-pkg")));
        assert!(!minimal_libs.contains(&"include/node/node.h".to_string()));

        let full_libs: Vec<String> =
            collect_node_libs_for(&node_root, ToolchainProfile::Full, NODE_MANAGERS, true)
                .into_iter()
                .map(|(_, r)| r)
                .collect();
        assert!(full_libs.contains(&"lib/node_modules/npm/package.json".to_string()));
        assert!(full_libs.contains(&"lib/node_modules/pnpm/package.json".to_string()));
        assert!(full_libs.contains(&"lib/node_modules/corepack/package.json".to_string()));
        assert!(full_libs.contains(&"include/node/node.h".to_string()));
        // Even under Full, arbitrary host-global `npm install -g` packages must be excluded.
        assert!(!full_libs
            .iter()
            .any(|r| r.contains("global-third-party-tool") || r.contains("@scope/leaked-pkg")));

        // Context helper includes headers when Cc or binding.gyp is active, or Full profile
        let ws_plain = temp.path().join("plain_js");
        fs::create_dir_all(&ws_plain)?;
        let ctx_plain = ToolchainDiscoveryContext {
            profile: ToolchainProfile::Minimal,
            workspace_root: Some(ws_plain.clone()),
            ..ToolchainDiscoveryContext::default()
        };
        assert!(!should_include_node_headers(&ctx_plain));

        let ctx_cc = ToolchainDiscoveryContext {
            ecosystems: [ToolchainEcosystem::Cc].into_iter().collect(),
            ..ctx_plain.clone()
        };
        assert!(should_include_node_headers(&ctx_cc));

        fs::write(ws_plain.join("binding.gyp"), "{}")?;
        assert!(should_include_node_headers(&ctx_plain));

        Ok(())
    }

    #[test]
    fn test_select_node_managers_scans_member_roots_and_corepack(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let temp = tempdir()?;
        let py_root = temp.path().join("backend-py");
        let sidecar_root = temp.path().join("desktop-shell");
        let frontend_root = sidecar_root.join("frontend");
        fs::create_dir_all(&py_root)?;
        fs::create_dir_all(&frontend_root)?;
        fs::write(py_root.join("pyproject.toml"), "[project]\nname = \"py\"\n")?;
        fs::write(frontend_root.join("package.json"), "{\"name\":\"ui\"}")?;
        fs::write(
            frontend_root.join("pnpm-lock.yaml"),
            "lockfileVersion: '9.0'\n",
        )?;

        let ctx = ToolchainDiscoveryContext {
            profile: ToolchainProfile::Minimal,
            workspace_root: Some(py_root),
            member_roots: vec![sidecar_root],
            ..ToolchainDiscoveryContext::default()
        };
        let managers = select_node_managers(&ctx);
        assert!(
            managers.contains(&"pnpm"),
            "Expected minimal profile to select pnpm from nested sidecar member root: {:?}",
            managers
        );

        #[cfg(unix)]
        {
            let node_root = temp.path().join("node_corepack");
            let bin = node_root.join("bin");
            let corepack_dist = node_root.join("lib/node_modules/corepack/dist");
            fs::create_dir_all(&bin)?;
            fs::create_dir_all(&corepack_dist)?;
            let node_bin = bin.join("node");
            fs::write(&node_bin, "#!/bin/sh\n")?;
            fs::write(corepack_dist.join("pnpm.js"), "#!/usr/bin/env node\n")?;
            std::os::unix::fs::symlink(
                "../lib/node_modules/corepack/dist/pnpm.js",
                bin.join("pnpm"),
            )?;

            let expanded = expand_managers_with_corepack(&node_bin, &["pnpm"]);
            assert!(
                expanded.contains(&"corepack"),
                "Expected Corepack-linked pnpm to include corepack module: {:?}",
                expanded
            );
        }

        Ok(())
    }
}
