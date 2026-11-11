// Version control integration, bundle generation, and delta tracking.
// Provides abstract VCS capabilities and Git provider

use git2::{DiffOptions, Error as GitError, ErrorClass, ErrorCode, Repository, StatusOptions};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

/// Structured representation of workspace version control provenance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VcsProvenance {
    pub commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    pub dirty: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_untracked: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncommitted_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub untracked_files: Vec<String>,
}

/// Metadata describing a generated Git/VCS bundle.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleInfo {
    pub head_commit: String,
    pub branch: Option<String>,
    pub object_count: usize,
    pub is_incremental: bool,
    pub base_commit: Option<String>,
}

/// Typed errors originating from VCS and repository operations.
#[derive(Debug)]
pub enum VcsError {
    /// Underlying libgit2 failure
    Git(GitError),
    /// Underlying filesystem I/O failure
    Io(std::io::Error),
    /// Bundle format or structural parsing error
    MalformedBundle(String),
    /// Missing prerequisite commit required for incremental restoration
    MissingPrerequisite { commit: String },
    /// Working tree or head reference checkout failure
    CheckoutFailed(String),
}

impl fmt::Display for VcsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Git(e) => write!(f, "VCS Git Error: {}", e),
            Self::Io(e) => write!(f, "VCS IO Error: {}", e),
            Self::MalformedBundle(msg) => write!(f, "Malformed VCS bundle: {}", msg),
            Self::MissingPrerequisite { commit } => {
                write!(
                    f,
                    "Missing prerequisite commit {} for incremental bundle",
                    commit
                )
            }
            Self::CheckoutFailed(msg) => write!(f, "VCS checkout failed: {}", msg),
        }
    }
}

impl std::error::Error for VcsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Git(e) => Some(e),
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}

impl From<GitError> for VcsError {
    fn from(e: GitError) -> Self {
        Self::Git(e)
    }
}

impl From<std::io::Error> for VcsError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<VcsError> for GitError {
    fn from(e: VcsError) -> Self {
        match e {
            VcsError::Git(git_err) => git_err,
            other => GitError::from_str(&other.to_string()),
        }
    }
}

/// Abstract Version Control System provider interface.
///
/// Formally required by the DMZ Autonomous Governance Standard (§2.1.2 & §2.1.5)
/// to decouple toolchain and workspace lifecycles from concrete VCS drivers.
/// Note: Provider methods perform synchronous I/O; offload via spawn_blocking in async tasks.
pub trait VcsProvider: Send + Sync {
    /// Identifier of this VCS provider (e.g. "git")
    fn name(&self) -> &'static str;

    /// Extract current commit hash of an existing local workspace repository
    fn get_current_commit(&self, workspace_path: &Path) -> Result<String, VcsError>;

    /// Retrieve the author/commit unix timestamp (seconds since epoch) of HEAD, if available
    fn get_commit_timestamp(&self, workspace_path: &Path) -> Result<Option<u64>, VcsError>;

    /// Inspect workspace repository provenance: active commit, active branch, dirty status,
    /// with full breakdown of uncommitted modified files and untracked files.
    fn get_provenance(&self, workspace_path: &Path) -> Result<Option<VcsProvenance>, VcsError>;

    /// Generate deterministic VCS bundle file containing HEAD (and optionally incremental commits)
    fn create_bundle(
        &self,
        workspace_path: &Path,
        output_bundle_path: &Path,
        since_commit: Option<&str>,
    ) -> Result<BundleInfo, VcsError>;

    /// Restore VCS bundle into the workspace repository offline
    fn restore_bundle(&self, workspace_path: &Path, bundle_path: &Path) -> Result<(), VcsError>;

    /// Compute list of modified or newly added files in the workspace since a baseline commit or ref
    fn get_changed_files_since(
        &self,
        workspace_path: &Path,
        since_commit_or_ref: &str,
    ) -> Result<Vec<PathBuf>, VcsError>;
}

/// Production Git provider backed by statically linked `libgit2`.
#[derive(Debug, Default, Clone, Copy)]
pub struct GitProvider;

impl GitProvider {
    /// Clone repository offline/directly into a target workspace using embedded libgit2
    pub fn clone_repository(url: &str, target_path: &Path) -> Result<String, GitError> {
        let repo = Repository::clone(url, target_path)?;
        let head = repo.head()?;
        let commit_oid = head
            .target()
            .ok_or_else(|| GitError::from_str("Failed to resolve HEAD commit OID"))?;
        Ok(commit_oid.to_string())
    }

    /// Extract current commit hash of an existing local workspace repository
    pub fn get_current_commit(workspace_path: &Path) -> Result<String, GitError> {
        let repo = Repository::open(workspace_path)?;
        let head = repo.head()?;
        let commit = head.peel_to_commit()?;
        Ok(commit.id().to_string())
    }

    /// Retrieve the author/commit unix timestamp (seconds since epoch) of HEAD, if available
    pub fn get_commit_timestamp(workspace_path: &Path) -> Result<Option<u64>, GitError> {
        let repo = match Repository::open(workspace_path) {
            Ok(r) => r,
            Err(e) if e.class() == ErrorClass::Repository || e.code() == ErrorCode::NotFound => {
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        let head = match repo.head() {
            Ok(h) => h,
            Err(_) => return Ok(None),
        };
        let commit = match head.peel_to_commit() {
            Ok(c) => c,
            Err(_) => return Ok(None),
        };
        let time_secs = commit.time().seconds();
        let timestamp = if time_secs > 0 { time_secs as u64 } else { 0 };
        Ok(Some(timestamp))
    }

    /// Inspect workspace repository provenance: active commit, active branch, dirty status,
    /// with full breakdown of uncommitted modified files and untracked files.
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

        let branch = head.shorthand().ok().map(|s| s.to_string());

        let mut status_opts = StatusOptions::new();
        status_opts.include_untracked(true);
        status_opts.recurse_untracked_dirs(true);
        status_opts.renames_head_to_index(true);
        status_opts.renames_index_to_workdir(true);

        let mut uncommitted_files = Vec::new();
        let mut untracked_files = Vec::new();

        if let Ok(statuses) = repo.statuses(Some(&mut status_opts)) {
            for entry in statuses.iter() {
                let st = entry.status();
                if let Ok(path) = entry.path() {
                    if st.intersects(
                        git2::Status::INDEX_NEW
                            | git2::Status::INDEX_MODIFIED
                            | git2::Status::INDEX_DELETED
                            | git2::Status::INDEX_RENAMED
                            | git2::Status::INDEX_TYPECHANGE
                            | git2::Status::WT_MODIFIED
                            | git2::Status::WT_DELETED
                            | git2::Status::WT_RENAMED
                            | git2::Status::WT_TYPECHANGE,
                    ) {
                        uncommitted_files.push(path.to_string());
                    } else if st.contains(git2::Status::WT_NEW) {
                        untracked_files.push(path.to_string());
                    }
                }
            }
        }

        let dirty = !uncommitted_files.is_empty();
        let has_untracked = !untracked_files.is_empty();

        Ok(Some(VcsProvenance {
            commit,
            branch,
            dirty,
            has_untracked,
            uncommitted_files,
            untracked_files,
        }))
    }

    /// Return list of uncommitted (modified, deleted, or staged) files in the workspace
    pub fn get_uncommitted_files(workspace_path: &Path) -> Result<Vec<String>, GitError> {
        let prov = Self::get_provenance(workspace_path)?;
        Ok(prov.map(|p| p.uncommitted_files).unwrap_or_default())
    }

    /// Return list of untracked files in the workspace
    pub fn get_untracked_files(workspace_path: &Path) -> Result<Vec<String>, GitError> {
        let prov = Self::get_provenance(workspace_path)?;
        Ok(prov.map(|p| p.untracked_files).unwrap_or_default())
    }

    /// Generate deterministic Git bundle v2 file containing HEAD (and optionally incremental commits)
    /// atomically using an ephemeral staging file to eliminate corrupted writes.
    pub fn create_bundle(
        workspace_path: &Path,
        output_bundle_path: &Path,
        since_commit: Option<&str>,
    ) -> Result<BundleInfo, GitError> {
        let repo = Repository::open(workspace_path)?;
        let head = repo.head()?;
        let head_commit = head.peel_to_commit()?;
        let head_oid = head_commit.id();
        let branch = head.shorthand().ok().map(|s| s.to_string());

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
        let parent = output_bundle_path
            .parent()
            .unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|e| {
            GitError::from_str(&format!(
                "Failed to create parent directory for bundle: {}",
                e
            ))
        })?;

        // Atomic writing pattern: write to secure temporary file and persist atomically
        let mut temp_file = tempfile::Builder::new()
            .prefix(".bundle-tmp-")
            .tempfile_in(parent)
            .map_err(|e| {
                GitError::from_str(&format!("Failed to create atomic temp bundle file: {}", e))
            })?;

        // Write Git Bundle v2 header
        writeln!(temp_file, "# v2 git bundle")
            .map_err(|e| GitError::from_str(&format!("Failed to write bundle header: {}", e)))?;

        if let Some(base_oid) = base_oid_opt {
            writeln!(temp_file, "-{} base", base_oid).map_err(|e| {
                GitError::from_str(&format!("Failed to write bundle prerequisite: {}", e))
            })?;
        }

        writeln!(temp_file, "{} HEAD", head_oid)
            .map_err(|e| GitError::from_str(&format!("Failed to write bundle HEAD ref: {}", e)))?;

        if let Some(ref br) = branch {
            writeln!(temp_file, "{} refs/heads/{}", head_oid, br).map_err(|e| {
                GitError::from_str(&format!("Failed to write bundle branch ref: {}", e))
            })?;
        }

        writeln!(temp_file)
            .map_err(|e| GitError::from_str(&format!("Failed to write bundle separator: {}", e)))?;

        temp_file
            .write_all(&pack_buffer)
            .map_err(|e| GitError::from_str(&format!("Failed to write bundle packfile: {}", e)))?;

        temp_file
            .flush()
            .map_err(|e| GitError::from_str(&format!("Failed to flush bundle packfile: {}", e)))?;

        temp_file.persist(output_bundle_path).map_err(|e| {
            GitError::from_str(&format!("Failed to persist bundle file atomically: {}", e))
        })?;

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

    /// Compute list of modified or newly added files in the workspace since a baseline commit or ref
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

    /// Restore Git bundle into the workspace repository offline.
    /// Strictly verifies bundle format, prerequisite commits, and enforces working tree checkout.
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

        // Stage bundle into .git/workspace.bundle for audit record (non-fatal if logging directory locked)
        let git_dir = repo.path().to_path_buf();
        let target_bundle_path = git_dir.join("workspace.bundle");
        if let Err(e) = std::fs::copy(bundle_path, &target_bundle_path) {
            debug!(
                "Failed to stage audit bundle to {:?}: {}",
                target_bundle_path, e
            );
        }

        // Parse v2 git bundle header and index embedded PACK stream directly into repository ODB
        let raw_bytes = std::fs::read(bundle_path)
            .map_err(|e| GitError::from_str(&format!("Failed to read bundle file: {}", e)))?;

        if !raw_bytes.starts_with(b"# v2 git bundle\n") {
            return Err(GitError::from_str(
                "Unsupported or corrupted bundle header (expected '# v2 git bundle')",
            ));
        }

        // Locate the blank separator line (\n\n) before the PACK payload
        let pack_offset = raw_bytes
            .windows(2)
            .position(|w| w == b"\n\n")
            .map(|idx| idx.saturating_add(2))
            .ok_or_else(|| {
                GitError::from_str(
                    "Malformed git bundle: missing header separator (expected double newline)",
                )
            })?;

        let header_slice = raw_bytes
            .get(..pack_offset.saturating_sub(1))
            .ok_or_else(|| {
                GitError::from_str("Malformed git bundle: header bounds calculation error")
            })?;

        let header_str = std::str::from_utf8(header_slice)
            .map_err(|e| GitError::from_str(&format!("Invalid UTF-8 in bundle header: {}", e)))?;

        let mut prerequisites: Vec<git2::Oid> = Vec::new();
        let mut advertised_refs: Vec<(git2::Oid, String)> = Vec::new();

        for line in header_str.lines().skip(1) {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                break;
            }
            if let Some(rest) = trimmed.strip_prefix('-') {
                if let Some(oid_str) = rest.split_whitespace().next() {
                    let oid = git2::Oid::from_str(oid_str)?;
                    prerequisites.push(oid);
                }
            } else if let Some((oid_str, ref_name)) = trimmed.split_once(' ') {
                let oid = git2::Oid::from_str(oid_str.trim())?;
                advertised_refs.push((oid, ref_name.trim().to_string()));
            }
        }

        let odb = repo.odb()?;
        for prereq in prerequisites {
            if !odb.exists(prereq) {
                return Err(GitError::from_str(&format!(
                    "Missing prerequisite commit {} required by incremental bundle",
                    prereq
                )));
            }
        }

        let pack_bytes = raw_bytes.get(pack_offset..).ok_or_else(|| {
            GitError::from_str("Malformed git bundle: packfile bounds calculation error")
        })?;

        if pack_bytes.starts_with(b"PACK") {
            let mut packwriter = odb.packwriter()?;
            packwriter.write_all(pack_bytes).map_err(|e| {
                GitError::from_str(&format!("Failed to stream bundle packfile into ODB: {}", e))
            })?;
            packwriter.commit()?;
        }

        let mut head_target_ref: Option<String> = None;
        let mut head_oid_opt: Option<git2::Oid> = None;

        for (oid, ref_name) in &advertised_refs {
            if ref_name == "HEAD" {
                head_oid_opt = Some(*oid);
            } else if ref_name.starts_with("refs/") {
                repo.reference(ref_name, *oid, true, "dmz restore_bundle")?;
                if head_target_ref.is_none() && ref_name.starts_with("refs/heads/") {
                    head_target_ref = Some(ref_name.clone());
                }
            }
        }

        if head_target_ref.is_none() {
            if let Some(head_oid) = head_oid_opt {
                repo.reference("refs/heads/main", head_oid, true, "dmz restore_bundle")?;
                head_target_ref = Some("refs/heads/main".to_string());
            }
        }

        // Hardened: strictly enforce set_head and checkout_head without silent suppression
        if let Some(ref target_ref) = head_target_ref {
            repo.set_head(target_ref)?;
            repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))?;
        }

        Ok(())
    }
}

impl VcsProvider for GitProvider {
    fn name(&self) -> &'static str {
        "git"
    }

    fn get_current_commit(&self, workspace_path: &Path) -> Result<String, VcsError> {
        Self::get_current_commit(workspace_path).map_err(VcsError::from)
    }

    fn get_commit_timestamp(&self, workspace_path: &Path) -> Result<Option<u64>, VcsError> {
        Self::get_commit_timestamp(workspace_path).map_err(VcsError::from)
    }

    fn get_provenance(&self, workspace_path: &Path) -> Result<Option<VcsProvenance>, VcsError> {
        Self::get_provenance(workspace_path).map_err(VcsError::from)
    }

    fn create_bundle(
        &self,
        workspace_path: &Path,
        output_bundle_path: &Path,
        since_commit: Option<&str>,
    ) -> Result<BundleInfo, VcsError> {
        Self::create_bundle(workspace_path, output_bundle_path, since_commit)
            .map_err(VcsError::from)
    }

    fn restore_bundle(&self, workspace_path: &Path, bundle_path: &Path) -> Result<(), VcsError> {
        Self::restore_bundle(workspace_path, bundle_path).map_err(VcsError::from)
    }

    fn get_changed_files_since(
        &self,
        workspace_path: &Path,
        since_commit_or_ref: &str,
    ) -> Result<Vec<PathBuf>, VcsError> {
        Self::get_changed_files_since(workspace_path, since_commit_or_ref).map_err(VcsError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Signature;

    fn init_test_repo(path: &Path) -> (Repository, String) {
        let repo = Repository::init(path).expect("failed to init repo");
        let sig = Signature::now("DMZ Test", "test@dmz.local").expect("signature failed");

        let test_file = path.join("README.md");
        std::fs::write(&test_file, "# DMZ Test Repository\n").expect("write test file");

        let mut index = repo.index().expect("index failed");
        index.add_path(Path::new("README.md")).expect("add path");
        index.write().expect("index write");

        let tree_id = index.write_tree().expect("write tree");
        let tree = repo.find_tree(tree_id).expect("find tree");

        let commit_oid = repo
            .commit(Some("HEAD"), &sig, &sig, "Initial test commit", &tree, &[])
            .expect("commit failed");

        drop(tree);
        (repo, commit_oid.to_string())
    }

    #[test]
    fn test_git_provider_implements_vcs_provider_trait() {
        let provider: Box<dyn VcsProvider> = Box::new(GitProvider);
        assert_eq!(provider.name(), "git");
    }

    #[test]
    fn test_atomic_bundle_creation_and_restoration_via_trait() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let src_path = temp_dir.path().join("src_repo");
        let dst_path = temp_dir.path().join("dst_repo");
        let bundle_file = temp_dir.path().join("bundles").join("test.bundle");

        let (_repo, initial_commit) = init_test_repo(&src_path);

        let provider: Box<dyn VcsProvider> = Box::new(GitProvider);

        // Verify get_current_commit
        let commit = provider
            .get_current_commit(&src_path)
            .expect("get_current_commit");
        assert_eq!(commit, initial_commit);

        // Verify create_bundle (atomic)
        let info = provider
            .create_bundle(&src_path, &bundle_file, None)
            .expect("create_bundle");
        assert_eq!(info.head_commit, initial_commit);
        assert!(bundle_file.exists());

        // Verify restore_bundle into new workspace
        provider
            .restore_bundle(&dst_path, &bundle_file)
            .expect("restore_bundle");

        let restored_commit = provider
            .get_current_commit(&dst_path)
            .expect("get restored commit");
        assert_eq!(restored_commit, initial_commit);

        // Verify checked out working tree file exists
        assert!(dst_path.join("README.md").exists());
        let content = std::fs::read_to_string(dst_path.join("README.md")).expect("read file");
        assert_eq!(content, "# DMZ Test Repository\n");
    }

    #[test]
    fn test_restore_bundle_corrupt_header_rejected() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let dst_path = temp_dir.path().join("dst_repo");
        let bundle_file = temp_dir.path().join("corrupt.bundle");

        std::fs::write(&bundle_file, "INVALID BUNDLE HEADER\n\nPACK...").expect("write file");

        let provider = GitProvider;
        let res = provider.restore_bundle(&dst_path, &bundle_file);
        assert!(res.is_err(), "Must reject corrupted bundle header");
    }

    #[test]
    fn test_restore_bundle_missing_separator_rejected() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let dst_path = temp_dir.path().join("dst_repo");
        let bundle_file = temp_dir.path().join("no_separator.bundle");

        std::fs::write(
            &bundle_file,
            "# v2 git bundle\nsingle line with no separator",
        )
        .expect("write file");

        let provider = GitProvider;
        let res = provider.restore_bundle(&dst_path, &bundle_file);
        assert!(res.is_err(), "Must reject missing separator");
    }

    #[test]
    fn test_vcs_error_display_and_conversions() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let vcs_err = VcsError::from(io_err);
        assert!(vcs_err.to_string().contains("VCS IO Error"));

        let git_err = GitError::from(vcs_err);
        assert!(git_err.to_string().contains("VCS IO Error"));
    }

    #[test]
    fn test_get_provenance_non_git_directory() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let provider = GitProvider;
        let prov = provider
            .get_provenance(temp_dir.path())
            .expect("non-git dir must return Ok(None)");
        assert!(prov.is_none());
    }
}
