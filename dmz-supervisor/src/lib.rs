pub mod agent_runner;
pub mod ipc;
pub mod service_runner;
#[path = "sidecar-orchestrator/mod.rs"]
pub mod sidecar_orchestrator;

pub use agent_runner::{AgentConfig, AgentRunner, AgentRunnerError, AgentState};
pub use ipc::server::{
    IpcServer, IpcServerConfig, IpcServerError, MAX_IPC_FRAME_BYTES, MAX_UNIX_SOCKET_PATH,
};
pub use ipc::{IpcClient, IpcClientError, IpcRequest, IpcResponse};
pub use service_runner::{ServiceConfig, ServiceRunner, ServiceRunnerError, ServiceStatus};
pub use sidecar_orchestrator::{
    SidecarLifecycleState, SidecarOrchestrator, SidecarOrchestratorError, SidecarPairConfig,
};
