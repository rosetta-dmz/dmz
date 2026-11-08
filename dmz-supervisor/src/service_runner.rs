use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpStream;
use tokio::process::{Child, Command};
use tokio::sync::RwLock;
use tokio::time::{sleep, timeout, Duration};
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct ServiceConfig {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub envs: HashMap<String, String>,
    pub working_dir: PathBuf,
    pub health_check_addr: Option<SocketAddr>,
    pub health_check_timeout_secs: u64,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ServiceStatus {
    Stopped,
    Starting,
    Healthy,
    Unhealthy(String),
    Failed(String),
}

pub struct ManagedService {
    pub config: ServiceConfig,
    pub status: ServiceStatus,
    child: Option<Child>,
    #[cfg(unix)]
    pgid: Option<i32>,
}

impl Drop for ManagedService {
    fn drop(&mut self) {
        if let Some(ref mut child) = self.child {
            if let Ok(None) = child.try_wait() {
                #[cfg(unix)]
                if let Some(pid) = self.pgid {
                    // SAFETY: `pid` is a positive process group ID created via `cmd.process_group(0)` for this service.
                    unsafe {
                        libc::killpg(pid, libc::SIGKILL);
                    }
                }
                let _ = child.start_kill();
            }
        }
    }
}

#[derive(Clone, Default)]
pub struct ServiceRunner {
    services: Arc<RwLock<HashMap<String, ManagedService>>>,
}

impl ServiceRunner {
    pub fn new() -> Self {
        Self {
            services: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Register & start a background collateral service
    pub async fn start_service(&self, config: ServiceConfig) -> Result<(), String> {
        let name = config.name.clone();

        {
            let services = self.services.read().await;
            if let Some(existing) = services.get(&name) {
                if existing.status == ServiceStatus::Healthy
                    || existing.status == ServiceStatus::Starting
                {
                    return Err(format!("Service '{}' is already running", name));
                }
            }
        }

        info!(service = %name, cmd = %config.command, "Spawning collateral stack service");

        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args)
            .envs(&config.envs)
            .current_dir(&config.working_dir)
            .kill_on_drop(true);

        #[cfg(unix)]
        cmd.process_group(0);

        let child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn service '{}': {}", name, e))?;

        #[cfg(unix)]
        let pgid = child.id().map(|id| id as i32);

        let managed = ManagedService {
            config: config.clone(),
            status: ServiceStatus::Starting,
            child: Some(child),
            #[cfg(unix)]
            pgid,
        };

        {
            let mut services = self.services.write().await;
            services.insert(name.clone(), managed);
        }

        if let Some(addr) = config.health_check_addr {
            let services_ref = self.services.clone();
            let name_clone = name.clone();
            let timeout_secs = config.health_check_timeout_secs;

            tokio::spawn(async move {
                Self::poll_health(services_ref, name_clone, addr, timeout_secs).await;
            });
        } else {
            let mut services = self.services.write().await;
            if let Some(service) = services.get_mut(&name) {
                service.status = ServiceStatus::Healthy;
            }
        }

        Ok(())
    }

    async fn poll_health(
        services: Arc<RwLock<HashMap<String, ManagedService>>>,
        name: String,
        addr: SocketAddr,
        timeout_secs: u64,
    ) {
        let start = std::time::Instant::now();
        let total_timeout = Duration::from_secs(timeout_secs);

        while start.elapsed() < total_timeout {
            if matches!(
                timeout(Duration::from_millis(500), TcpStream::connect(addr)).await,
                Ok(Ok(_))
            ) {
                info!(service = %name, addr = %addr, "Service TCP health check PASSED");
                let mut guard = services.write().await;
                if let Some(service) = guard.get_mut(&name) {
                    service.status = ServiceStatus::Healthy;
                }
                return;
            }
            sleep(Duration::from_millis(250)).await;
        }

        warn!(service = %name, addr = %addr, "Service TCP health check TIMED OUT");
        let mut guard = services.write().await;
        if let Some(service) = guard.get_mut(&name) {
            service.status = ServiceStatus::Unhealthy("Health check timeout".into());
        }
    }

    pub async fn stop_service(&self, name: &str) -> Result<(), String> {
        let mut services = self.services.write().await;
        if let Some(mut service) = services.remove(name) {
            if let Some(mut child) = service.child.take() {
                info!(service = %name, "Terminating background collateral service");

                #[cfg(windows)]
                {
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    return Ok(());
                }

                #[cfg(unix)]
                {
                    if let Some(pid) = service.pgid.or_else(|| child.id().map(|id| id as i32)) {
                        // SAFETY: `pid` is a positive process group ID created via `cmd.process_group(0)` for this service.
                        unsafe {
                            libc::killpg(pid, libc::SIGTERM);
                        }
                    }
                }

                tokio::select! {
                    _ = child.wait() => {
                        info!(service = %name, "Service process exited gracefully");
                    }
                    _ = sleep(Duration::from_secs(3)) => {
                        warn!(service = %name, "Service failed to exit gracefully; sending SIGKILL");
                        #[cfg(unix)]
                        if let Some(pid) = service.pgid.or_else(|| child.id().map(|id| id as i32)) {
                            // SAFETY: `pid` is a positive process group ID created via `cmd.process_group(0)` for this service.
                            unsafe {
                                libc::killpg(pid, libc::SIGKILL);
                            }
                        }
                        let _ = child.kill().await;
                        let _ = child.wait().await;
                    }
                }
            }
            Ok(())
        } else {
            Err(format!("Service '{}' not found", name))
        }
    }

    pub async fn get_status_all(&self) -> HashMap<String, ServiceStatus> {
        let services = self.services.read().await;
        services
            .iter()
            .map(|(k, v)| (k.clone(), v.status.clone()))
            .collect()
    }
}
