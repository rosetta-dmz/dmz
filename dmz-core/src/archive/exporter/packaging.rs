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
    append_dir_all_excluding(
        tar,
        src_dir,
        dest_prefix,
        skip_manifest,
        deterministic_epoch,
        &[],
    )
}

pub fn append_dir_all_excluding<W: Write>(
    tar: &mut TarBuilder<W>,
    src_dir: &Path,
    dest_prefix: &Path,
    skip_manifest: bool,
    deterministic_epoch: u64,
    excluded_canon: &[PathBuf],
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
        if !excluded_canon.is_empty() {
            let entry_canon = fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
            if excluded_canon
                .iter()
                .any(|ex| entry_canon == *ex || entry_canon.starts_with(ex))
            {
                continue;
            }
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let file_type = entry
            .file_type()
            .map_err(|e| ExportError::IoError(e.to_string()))?;

        // --- SKIP HEAVY BUILD ARTIFACTS & CACHES ---
        if (file_type.is_dir() || file_type.is_symlink())
            && crate::workspace::should_skip_workspace_artifact_dir(&path, &name_str, None)
        {
            continue;
        }

        if skip_manifest && name_str == "dmz-workspace.toml" {
            continue;
        }

        let rel_dest = dest_prefix.join(name);

        if file_type.is_dir() {
            append_dir_all_excluding(
                tar,
                &path,
                &rel_dest,
                skip_manifest,
                deterministic_epoch,
                excluded_canon,
            )?;
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
    append_filtered_files_prefixed(
        tar,
        workspace_root,
        Path::new(""),
        files,
        deterministic_epoch,
    )
}

pub fn append_filtered_files_prefixed<W: Write>(
    tar: &mut TarBuilder<W>,
    workspace_root: &Path,
    dest_prefix: &Path,
    files: &HashSet<PathBuf>,
    deterministic_epoch: u64,
) -> Result<(), ExportError> {
    let mut sorted_files: Vec<_> = files.iter().collect();
    sorted_files.sort();

    for rel_path in sorted_files {
        if crate::workspace::is_ignored_workspace_rel_path(rel_path) {
            continue;
        }

        let full_path = workspace_root.join(rel_path);
        if full_path.is_file() {
            let dest_rel = if dest_prefix.as_os_str().is_empty() {
                rel_path.to_path_buf()
            } else {
                dest_prefix.join(rel_path)
            };
            let normalized_path = dest_rel.to_string_lossy().replace('\\', "/");
            append_file_normalized(tar, &full_path, &normalized_path, deterministic_epoch)?;
        }
    }
    Ok(())
}

pub fn read_manifest_from_archive(path: &Path) -> Result<AirGapManifest, ExportError> {
    let file = File::open(path)
        .map_err(|e| ExportError::IoError(format!("Failed to open since archive: {}", e)))?;
    let decoder = zstd::stream::read::Decoder::new(file)
        .map_err(|e| ExportError::ArchiveError(format!("Zstd error: {}", e)))?;
    let mut archive = tar::Archive::new(decoder);
    for entry in archive
        .entries()
        .map_err(|e| ExportError::ArchiveError(format!("Tar error: {}", e)))?
    {
        let mut entry =
            entry.map_err(|e| ExportError::ArchiveError(format!("Tar entry error: {}", e)))?;
        let entry_path = entry
            .path()
            .map_err(|e| ExportError::ArchiveError(e.to_string()))?
            .to_path_buf();
        if entry_path == Path::new("airgap_manifest.json") {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|e| ExportError::IoError(format!("Read error: {}", e)))?;
            let manifest: AirGapManifest = serde_json::from_str(&content)
                .map_err(|e| ExportError::ArchiveError(format!("JSON parse error: {}", e)))?;
            return Ok(manifest);
        }
    }
    Err(ExportError::ArchiveError(
        "airgap_manifest.json not found in archive".to_string(),
    ))
}
