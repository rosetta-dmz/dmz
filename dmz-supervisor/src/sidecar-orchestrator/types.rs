use dmz_core::workspace::WorkspaceProjectConfig;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

use super::child::ManagedChild;

#[derive(Debug, Error)]
pub enum SidecarOrchestratorError {
    #[error("A sidecar workspace pair is already active in state: {0:?}")]
    AlreadyActive(SidecarLifecycleState),

    #[error(
        "No sidecar or service member found in workspace '{workspace}' (has {member_count} members). At least one member must have role 'sidecar' or 'service'."
    )]
    NoSidecarMember {
        workspace: String,
        member_count: usize,
    },

    #[error(
        "No primary member found in workspace '{0}'. At least one member must have role 'primary'."
    )]
    NoPrimaryMember(String),

    #[error("Failed to prepare safe socket: {0}")]
    SocketPreparation(String),

    #[error(
        "Sidecar readiness probe TIMED OUT after {timeout_ms}ms waiting for socket {socket_path:?}"
    )]
    ReadinessTimeout {
        timeout_ms: u64,
        socket_path: PathBuf,
    },

    #[error("Companion service process exited prematurely with status {status}{stderr_info}")]
    CompanionExitedPrematurely {
        id: String,
        status: std::process::ExitStatus,
        stderr_info: String,
    },

    #[error("Process spawn failed for '{member}': {source}")]
    SpawnFailed {
        member: String,
        #[source]
        source: std::io::Error,
    },

    #[error("build_command '{command}' for '{member}' exited with {status}: {stderr}")]
    BuildCommandFailed {
        command: String,
        member: String,
        status: std::process::ExitStatus,
        stderr: String,
    },

    #[error(
        "Unable to determine execution command for workspace member at {path}. Specify \"exec_command\" and \"exec_args\" in dmz-workspace.toml"
    )]
    ExecutionInferenceFailed { path: String },

    #[error("Workspace member directory does not exist: {0:?}")]
    MemberDirectoryNotFound(PathBuf),

    #[error("No active primary child to wait on")]
    NoPrimaryChild,

    #[error("Workspace pair stopped")]
    PairStopped,

    #[error("Watchdog channel closed before primary exit")]
    WatchdogChannelClosed,

    #[error("{0}")]
    Generic(String),
}

impl SidecarOrchestratorError {
    pub fn contains(&self, needle: &str) -> bool {
        self.to_string().contains(needle)
    }
}

impl From<SidecarOrchestratorError> for String {
    fn from(err: SidecarOrchestratorError) -> Self {
        err.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SidecarLifecycleState {
    #[default]
    Idle,
    CleaningStaleSocket,
    SpawningSidecar,
    ProbingSocketReadiness,
    SidecarReady,
    SpawningPrimary,
    Running,
    DrainingPrimary,
    TerminatingSidecar,
    Stopped,
    Failed,
}

#[derive(Debug, Clone)]
pub struct SidecarPairConfig {
    pub workspace_root: PathBuf,
    pub project: WorkspaceProjectConfig,
    pub allow_network: bool,
}

pub struct ActivePair {
    pub config: SidecarPairConfig,
    pub socket_path: PathBuf,
    pub sidecar_children: Vec<(String, ManagedChild)>,
    pub primary_child: Option<ManagedChild>,
}
