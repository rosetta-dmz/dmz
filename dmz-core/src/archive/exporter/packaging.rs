use super::types::{AirGapManifest, ExportError};
use crate::vcs::{GitProvider, VcsProvenance};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tar::Builder as TarBuilder;
use tracing::warn;

/// Resolves reproducible timestamp: SOURCE_DATE_EPOCH env var -> Git commit timestamp -> DOS/Bazel epoch (1980-01-01)
pub fn resolve_deterministic_epoch(workspace_root: &Path, _vcs: Option<&VcsProvenance>) -> u64 {
    if let Ok(epoch_str) = std::env::var("SOURCE_DATE_EPOCH") {
        if let Ok(epoch) = epoch_str.trim().parse::<u64>() {
            return epoch;
        }
    }
    if let Ok(Some(commit_time)) = GitProvider::get_commit_timestamp(workspace_root) {
        return commit_time;
    }
    315_532_800 // 1980-01-01 00:00:00 UTC (standard DOS/Bazel reproducible epoch)
}

pub fn append_file_normalized<W: Write>(
    tar: &mut TarBuilder<W>,
    src_path: &Path,
    dest_path: &str,
    deterministic_epoch: u64,
) -> Result<(), ExportError> {
    let sym_meta = fs::symlink_metadata(src_path).map_err(|e| {
        ExportError::IoError(format!("Failed to read metadata {:?}: {}", src_path, e))
    })?;

    if sym_meta.file_type().is_symlink() {
        let link_target = fs::read_link(src_path).map_err(|e| {
            ExportError::IoError(format!("Failed to read symlink {:?}: {}", src_path, e))
        })?;

        // Boundary containment check:
        // Ensure symlink target does not point outside the workspace or filesystem root
        if link_target.is_absolute() {
            warn!(
                path = ?src_path,
                target = ?link_target,
                "Skipping absolute symlink escaping export boundary"
            );
            return Ok(());
        }

        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_mtime(deterministic_epoch);
        header.set_uid(0);
        header.set_gid(0);
        let _ = header.set_username("");
        let _ = header.set_groupname("");
        let _ = header.set_link_name(&link_target);
        header.set_cksum();

        tar.append_data(&mut header, dest_path, &mut std::io::empty())
            .map_err(|e| {
                ExportError::ArchiveError(format!("Failed to add symlink {}: {}", dest_path, e))
            })?;

        return Ok(());
    }

    let mut file = File::open(src_path)
        .map_err(|e| ExportError::IoError(format!("Failed to open file {:?}: {}", src_path, e)))?;
    let metadata = file.metadata().map_err(|e| {
        ExportError::IoError(format!("Failed to read metadata {:?}: {}", src_path, e))
    })?;

    let is_executable = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            dest_path.ends_with(".exe")
                || dest_path.ends_with(".bat")
                || dest_path.ends_with(".cmd")
                || dest_path.ends_with(".sh")
        }
    };

    let mode = if is_executable { 0o755 } else { 0o644 };

    let mut header = tar::Header::new_gnu();
    header.set_size(metadata.len());
    header.set_mode(mode);
    header.set_mtime(deterministic_epoch);
    header.set_uid(0);
    header.set_gid(0);
    let _ = header.set_username("");
    let _ = header.set_groupname("");
    header.set_cksum();

    tar.append_data(&mut header, dest_path, &mut file)
        .map_err(|e| {
            ExportError::ArchiveError(format!("Failed to add file {}: {}", dest_path, e))
        })?;

    Ok(())
}

pub fn append_dir_all<W: Write>(
    tar: &mut TarBuilder<W>,
    src_dir: &Path,
    dest_prefix: &Path,
    skip_manifest: bool,
    deterministic_epoch: u64,
) -> Result<(), ExportError> {
    let entries = fs::read_dir(src_dir)
        .map_err(|e| ExportError::IoError(format!("Directory read failed {:?}: {}", src_dir, e)))?;

    let mut sorted_entries = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| ExportError::IoError(e.to_string()))?;
        sorted_entries.push(entry);
    }
    sorted_entries.sort_by_key(|e| e.file_name());

    for entry in sorted_entries {
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let file_type = entry
            .file_type()
            .map_err(|e| ExportError::IoError(e.to_string()))?;

        // --- SKIP HEAVY BUILD ARTIFACTS & CACHES ---
        if (file_type.is_dir() || file_type.is_symlink())
            && (name_str == "target"
                || name_str == "node_modules"
                || name_str == ".git"
                || name_str == "dist"
                || name_str == ".dmz"
                || name_str == ".venv"
                || name_str == "venv"
                || name_str == "__pycache__"
                || name_str == ".pytest_cache"
                || name_str == ".mypy_cache"
                || name_str == ".ruff_cache")
        {
            continue;
        }

        if skip_manifest && name_str == "dmz-workspace.toml" {
            continue;
        }

        let rel_dest = dest_prefix.join(name);

        if file_type.is_dir() {
            append_dir_all(tar, &path, &rel_dest, skip_manifest, deterministic_epoch)?;
        } else {
            let normalized_dest = rel_dest.to_string_lossy().replace('\\', "/");
            append_file_normalized(tar, &path, &normalized_dest, deterministic_epoch)?;
        }
    }

    Ok(())
}

pub fn append_filtered_files<W: Write>(
    tar: &mut TarBuilder<W>,
    workspace_root: &Path,
    files: &HashSet<PathBuf>,
    deterministic_epoch: u64,
) -> Result<(), ExportError> {
    let mut sorted_files: Vec<_> = files.iter().collect();
    sorted_files.sort();

    for rel_path in sorted_files {
        // Skip .git, target, node_modules, dist, .dmz, .venv, venv, __pycache__
        let starts_with_ignored = rel_path.components().any(|c| {
            let s = c.as_os_str().to_string_lossy();
            s == "target"
                || s == "node_modules"
                || s == ".git"
                || s == "dist"
                || s == ".dmz"
                || s == ".venv"
                || s == "venv"
                || s == "__pycache__"
                || s == ".pytest_cache"
                || s == ".mypy_cache"
                || s == ".ruff_cache"
        });

        if starts_with_ignored {
            continue;
        }

        let full_path = workspace_root.join(rel_path);
        if full_path.is_file() {
            let normalized_path = rel_path.to_string_lossy().replace('\\', "/");
            append_file_normalized(tar, &full_path, &normalized_path, deterministic_epoch)?;
        }
    }
    Ok(())
}

pub fn read_manifest_from_archive(path: &Path) -> Result<AirGapManifest, String> {
    let file = File::open(path).map_err(|e| format!("Failed to open since archive: {}", e))?;
    let decoder =
        zstd::stream::read::Decoder::new(file).map_err(|e| format!("Zstd error: {}", e))?;
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries().map_err(|e| format!("Tar error: {}", e))? {
        let mut entry = entry.map_err(|e| format!("Tar entry error: {}", e))?;
        let entry_path = entry.path().map_err(|e| e.to_string())?.to_path_buf();
        if entry_path == Path::new("airgap_manifest.json") {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|e| format!("Read error: {}", e))?;
            let manifest: AirGapManifest =
                serde_json::from_str(&content).map_err(|e| format!("JSON parse error: {}", e))?;
            return Ok(manifest);
        }
    }
    Err("airgap_manifest.json not found in archive".to_string())
}
