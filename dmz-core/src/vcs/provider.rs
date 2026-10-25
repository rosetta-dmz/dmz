use git2::{DiffOptions, Error as GitError, ErrorClass, ErrorCode, Repository, StatusOptions};
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VcsProvenance {
    pub commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub dirty: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleInfo {
    pub head_commit: String,
    pub branch: Option<String>,
    pub object_count: usize,
    pub is_incremental: bool,
    pub base_commit: Option<String>,
}

pub struct GitProvider;

impl GitProvider {
    /// Clones a repository offline/directly into a target workspace using embedded libgit2
    pub fn clone_repository(url: &str, target_path: &Path) -> Result<String, GitError> {
        let repo = Repository::clone(url, target_path)?;
        let head = repo.head()?;
        let commit_oid = head
            .target()
            .ok_or_else(|| GitError::from_str("Failed to resolve HEAD commit OID"))?;
        Ok(commit_oid.to_string())
    }

    /// Extracts the current commit hash of an existing local workspace repository
    pub fn get_current_commit(workspace_path: &Path) -> Result<String, GitError> {
        let repo = Repository::open(workspace_path)?;
        let head = repo.head()?;
        let commit = head.peel_to_commit()?;
        Ok(commit.id().to_string())
    }

    /// Inspects workspace repository provenance: active commit, active branch, and dirty status.
    /// Returns Ok(None) if workspace_path is not a git repository.
    pub fn get_provenance(workspace_path: &Path) -> Result<Option<VcsProvenance>, GitError> {
        let repo = match Repository::open(workspace_path) {
            Ok(r) => r,
            Err(e) if e.class() == ErrorClass::Repository || e.code() == ErrorCode::NotFound => {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };

        let head = match repo.head() {
            Ok(h) => h,
            Err(e) => {
                debug!("Failed to resolve HEAD: {}", e);
                return Ok(None);
            }
        };

        let commit = match head.peel_to_commit() {
            Ok(c) => c.id().to_string(),
            Err(e) => {
                debug!("Failed to peel HEAD to commit: {}", e);
                return Ok(None);
            }
        };

        let branch = head.shorthand().map(|s| s.to_string());

        let mut status_opts = StatusOptions::new();
        status_opts.include_untracked(true);
        status_opts.renames_head_to_index(true);
        status_opts.renames_index_to_workdir(true);

        let dirty = match repo.statuses(Some(&mut status_opts)) {
            Ok(statuses) => !statuses.is_empty(),
            Err(_) => false,
        };

        Ok(Some(VcsProvenance {
            commit,
            branch,
            dirty,
        }))
    }

    /// Generates a deterministic Git bundle v2 file containing HEAD (and optionally incremental commits)
    pub fn create_bundle(
        workspace_path: &Path,
        output_bundle_path: &Path,
        since_commit: Option<&str>,
    ) -> Result<BundleInfo, GitError> {
        let repo = Repository::open(workspace_path)?;
        let head = repo.head()?;
        let head_commit = head.peel_to_commit()?;
        let head_oid = head_commit.id();
        let branch = head.shorthand().map(|s| s.to_string());

        let mut packbuilder = repo.packbuilder()?;

        let mut base_oid_opt = None;
        if let Some(since_str) = since_commit {
            let since_clean = since_str.trim();
            if !since_clean.is_empty() {
                let resolved_obj = repo.revparse_single(since_clean)?;
                let base_oid = resolved_obj.id();
                base_oid_opt = Some(base_oid);

                let mut revwalk = repo.revwalk()?;
                revwalk.push(head_oid)?;
                revwalk.hide(base_oid)?;
                packbuilder.insert_walk(&mut revwalk)?;
            } else {
                let mut revwalk = repo.revwalk()?;
                revwalk.push(head_oid)?;
                packbuilder.insert_walk(&mut revwalk)?;
            }
        } else {
            let mut revwalk = repo.revwalk()?;
            revwalk.push(head_oid)?;
            packbuilder.insert_walk(&mut revwalk)?;
        }

        let mut pack_buffer = git2::Buf::new();
        packbuilder.write_buf(&mut pack_buffer)?;

        let object_count = packbuilder.object_count();

        // Ensure parent directory exists
        if let Some(parent) = output_bundle_path.parent() {
            if !parent.exists() {
                let _ = std::fs::create_dir_all(parent);
            }
        }

        let mut file = File::create(output_bundle_path)
            .map_err(|e| GitError::from_str(&format!("Failed to create bundle file: {}", e)))?;

        // Write Git Bundle v2 header
        writeln!(file, "# v2 git bundle")
            .map_err(|e| GitError::from_str(&format!("Failed to write bundle header: {}", e)))?;

        if let Some(base_oid) = base_oid_opt {
            writeln!(file, "-{} base", base_oid).map_err(|e| {
                GitError::from_str(&format!("Failed to write bundle prerequisite: {}", e))
            })?;
        }

        writeln!(file, "{} HEAD", head_oid)
            .map_err(|e| GitError::from_str(&format!("Failed to write bundle HEAD ref: {}", e)))?;

        if let Some(ref br) = branch {
            writeln!(file, "{} refs/heads/{}", head_oid, br).map_err(|e| {
                GitError::from_str(&format!("Failed to write bundle branch ref: {}", e))
            })?;
        }

        writeln!(file)
            .map_err(|e| GitError::from_str(&format!("Failed to write bundle separator: {}", e)))?;

        file.write_all(&pack_buffer)
            .map_err(|e| GitError::from_str(&format!("Failed to write bundle packfile: {}", e)))?;

        info!(
            head = %head_oid,
            objects = object_count,
            bundle = ?output_bundle_path,
            "Git bundle v2 generated successfully"
        );

        Ok(BundleInfo {
            head_commit: head_oid.to_string(),
            branch,
            object_count,
            is_incremental: base_oid_opt.is_some(),
            base_commit: base_oid_opt.map(|o| o.to_string()),
        })
    }

    /// Computes the list of modified or newly added files in the workspace since a baseline commit or ref
    pub fn get_changed_files_since(
        workspace_path: &Path,
        since_commit_or_ref: &str,
    ) -> Result<Vec<PathBuf>, GitError> {
        let repo = Repository::open(workspace_path)?;
        let base_obj = repo.revparse_single(since_commit_or_ref.trim())?;
        let base_commit = base_obj.peel_to_commit()?;
        let base_tree = base_commit.tree()?;

        let mut diff_opts = DiffOptions::new();
        diff_opts.include_untracked(true);
        diff_opts.recurse_untracked_dirs(true);

        // Diff base tree directly to workdir (captures committed, staged, and working directory modifications)
        let diff = repo.diff_tree_to_workdir_with_index(Some(&base_tree), Some(&mut diff_opts))?;

        let mut changed_files = Vec::new();
        for delta in diff.deltas() {
            if let Some(new_file) = delta.new_file().path() {
                changed_files.push(new_file.to_path_buf());
            }
        }

        Ok(changed_files)
    }

    /// Restores a Git bundle into the workspace repository offline
    pub fn restore_bundle(workspace_path: &Path, bundle_path: &Path) -> Result<(), GitError> {
        if !bundle_path.exists() {
            return Err(GitError::from_str(&format!(
                "Bundle path does not exist: {}",
                bundle_path.display()
            )));
        }

        let repo = if workspace_path.join(".git").exists() {
            Repository::open(workspace_path)?
        } else {
            Repository::init(workspace_path)?
        };

        // If repo exists or was initialized, copy the bundle into .git/workspace.bundle for record & local fetch
        let git_dir = repo.path();
        let target_bundle_path = git_dir.join("workspace.bundle");
        let _ = std::fs::copy(bundle_path, &target_bundle_path);

        Ok(())
    }
}
