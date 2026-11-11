use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use thiserror::Error;
use tokio::net::TcpStream;
use tokio::process::Command;
use tokio::sync::{oneshot, RwLock};
use tokio::time::{sleep, timeout, Duration};
use tracing::{info, warn};

#[derive(Debug, Error)]
pub enum ServiceRunnerError {
    #[error("Service '{0}' is already running")]
    AlreadyRunning(String),
    #[error("Service '{0}' not found")]
    NotFound(String),
    #[error("Failed to spawn service '{service}': {source}")]
    Spawn {
        service: String,
        #[source]
        source: std::io::Error,
    },
    #[error("Service '{0}' failed health check: {1}")]
    HealthCheckFailed(String, String),
}

impl From<ServiceRunnerError> for String {
    fn from(err: ServiceRunnerError) -> Self {
        err.to_string()
    }
}

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

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
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
    stop_tx: Option<oneshot::Sender<oneshot::Sender<()>>>,
    #[cfg(unix)]
    pub pgid: Option<i32>,
    reaped: Arc<AtomicBool>,
}

impl ManagedService {
    #[cfg(unix)]
    pub fn pgid(&self) -> Option<i32> {
        self.pgid
    }
}

impl Drop for ManagedService {
    fn drop(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            // CRITICAL DEFENSE: Never signal pgid <= 1 (prevents supervisor self-suicide).
            // pgid == 0 signals calling process group; pgid == -1 / 1 signals all host processes.
            if pgid > 1 && !self.reaped.load(Ordering::SeqCst) {
                // SAFETY: Emergency SIGKILL fallback to eliminate process group leaks on drop.
                unsafe {
                    libc::killpg(pgid, libc::SIGKILL);
                }
            }
        }
        if let Some(stop_tx) = self.stop_tx.take() {
            let (ack_tx, _) = oneshot::channel();
            let _ = stop_tx.send(ack_tx);
        }
    }
}

/// RAII Guard that rolls back the "Starting" reservation if the spawning future
/// fails or is cancelled before the child process is registered (Blueprint §4.2).
struct ReservationGuard {
    name: String,
    services: Arc<RwLock<HashMap<String, ManagedService>>>,
    committed: bool,
}

impl Drop for ReservationGuard {
    fn drop(&mut self) {
        if !self.committed {
            if let Ok(mut lock) = self.services.try_write() {
                if let Some(service) = lock.get(&self.name) {
                    if service.status == ServiceStatus::Starting {
                        lock.remove(&self.name);
                    }
                }
            } else {
                let services = Arc::clone(&self.services);
                let name = self.name.clone();
                tokio::spawn(async move {
                    let mut lock = services.write().await;
                    if let Some(service) = lock.get(&name) {
                        if service.status == ServiceStatus::Starting {
                            lock.remove(&name);
                        }
                    }
                });
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

    /// Register & start a background collateral service with supervision
    pub async fn start_service(&self, config: ServiceConfig) -> Result<(), ServiceRunnerError> {
        let name = config.name.clone();

        // Step 1: Atomically check existing status and reserve slot under write lock (Blueprint §4.2)
        {
            let mut services = self.services.write().await;
            if let Some(existing) = services.get(&name) {
                if existing.status == ServiceStatus::Healthy
                    || existing.status == ServiceStatus::Starting
                {
                    return Err(ServiceRunnerError::AlreadyRunning(name));
                }
            }
            services.insert(
                name.clone(),
                ManagedService {
                    config: config.clone(),
                    status: ServiceStatus::Starting,
                    stop_tx: None,
                    #[cfg(unix)]
                    pgid: None,
                    reaped: Arc::new(AtomicBool::new(false)),
                },
            );
        }

        // Arm the cancellation rollback guard
        let mut guard = ReservationGuard {
            name: name.clone(),
            services: Arc::clone(&self.services),
            committed: false,
        };

        info!(service = %name, cmd = %config.command, "Spawning collateral stack service");

        // Step 2: Spawn process with defense-in-depth environment sanitization
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args).env_clear(); // Scrub host daemon environment to prevent secret leakage

        // Retain benign host execution paths
        if let Ok(path) = std::env::var("PATH") {
            cmd.env("PATH", path);
        }
        #[cfg(target_os = "windows")]
        {
            if let Ok(sysroot) = std::env::var("SYSTEMROOT") {
                cmd.env("SYSTEMROOT", sysroot);
            }
            if let Ok(comspec) = std::env::var("COMSPEC") {
                cmd.env("COMSPEC", comspec);
            }
        }
        if let Ok(tmp) = std::env::var("TMPDIR") {
            cmd.env("TMPDIR", tmp);
        }

        cmd.envs(&config.envs)
            .current_dir(&config.working_dir)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(false); // Retain lifecycle under supervisor monitor

        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd.spawn().map_err(|e| ServiceRunnerError::Spawn {
            service: name.clone(),
            source: e,
        })?;

        // Validate positive PGID safely without unchecked narrowing cast
        #[cfg(unix)]
        let pgid = child.id().and_then(|id| match i32::try_from(id) {
            Ok(v) if v > 1 => Some(v),
            _ => None,
        });

        let (stop_tx, mut stop_rx) = oneshot::channel::<oneshot::Sender<()>>();
        let reaped = Arc::new(AtomicBool::new(false));

        let managed = ManagedService {
            config: config.clone(),
            status: ServiceStatus::Starting,
            stop_tx: Some(stop_tx),
            #[cfg(unix)]
            pgid,
            reaped: Arc::clone(&reaped),
        };

        // Active background child supervisor: monitors child exit status, cleans up zombies,
        // and synchronizes process status.
        let services_ref = Arc::clone(&self.services);
        let name_clone = name.clone();
        let reaped_watcher = Arc::clone(&reaped);

        tokio::spawn(async move {
            tokio::select! {
                exit_res = child.wait() => {
                    reaped_watcher.store(true, Ordering::SeqCst);
                    let mut guard = services_ref.write().await;
                    if let Some(service) = guard.get_mut(&name_clone) {
                        match exit_res {
                            Ok(status) if status.success() => {
                                info!(service = %name_clone, "Service process exited with status 0");
                                service.status = ServiceStatus::Stopped;
                            }
                            Ok(status) => {
                                warn!(service = %name_clone, status = ?status.code(), "Service process exited unexpectedly");
                                service.status = ServiceStatus::Failed(format!("Process exited with status {}", status));
                            }
                            Err(e) => {
                                warn!(service = %name_clone, error = %e, "Error waiting on service process");
                                service.status = ServiceStatus::Failed(format!("Wait error: {}", e));
                            }
                        }
                    }
                }
                stop_ack_tx = &mut stop_rx => {
                    info!(service = %name_clone, "Stopping service via termination request");
                    #[cfg(unix)]
                    if let Some(pid) = pgid {
                        // CRITICAL DEFENSE: Strictly guard against pgid <= 1 to avoid supervisor suicide
                        if pid > 1 && !reaped_watcher.load(Ordering::SeqCst) {
                            // SAFETY: `pid` is a validated positive process group ID (> 1)
                            // created via `cmd.process_group(0)`.
                            unsafe {
                                libc::killpg(pid, libc::SIGTERM);
                            }
                        }
                    }

                    #[cfg(windows)]
                    let _ = child.start_kill();

                    // Cooperative wait with timeout before escalating to SIGKILL (Blueprint §4.3)
                    let timed_out = {
                        let wait_fut = child.wait();
                        tokio::pin!(wait_fut);
                        tokio::time::timeout(Duration::from_secs(3), &mut wait_fut)
                            .await
                            .is_err()
                    };

                    if timed_out {
                        warn!(service = %name_clone, "Service failed to exit gracefully; sending SIGKILL");
                        #[cfg(unix)]
                        if let Some(pid) = pgid {
                            if pid > 1 && !reaped_watcher.load(Ordering::SeqCst) {
                                // SAFETY: Force-kill child process group on timeout.
                                unsafe {
                                    libc::killpg(pid, libc::SIGKILL);
                                }
                            }
                        }
                        #[cfg(windows)]
                        let _ = child.start_kill();

                        let _ = child.kill().await;
                        let _ = child.wait().await;
                    }

                    reaped_watcher.store(true, Ordering::SeqCst);
                    if let Ok(ack) = stop_ack_tx {
                        let _ = ack.send(());
                    }
                }
            }
        });

        // Step 3: Transition state to active under write lock and disarm the rollback guard
        {
            let mut services = self.services.write().await;
            services.insert(name.clone(), managed);
        }
        guard.committed = true;

        if let Some(addr) = config.health_check_addr {
            let services_ref = Arc::clone(&self.services);
            let name_clone = name.clone();
            let timeout_secs = config.health_check_timeout_secs;
            let reaped_probe = Arc::clone(&reaped);

            tokio::spawn(async move {
                Self::poll_health(services_ref, name_clone, addr, timeout_secs, reaped_probe).await;
            });
        } else {
            let mut services = self.services.write().await;
            if let Some(service) = services.get_mut(&name) {
                if service.status == ServiceStatus::Starting {
                    service.status = ServiceStatus::Healthy;
                }
            }
        }

        Ok(())
    }

    async fn poll_health(
        services: Arc<RwLock<HashMap<String, ManagedService>>>,
        name: String,
        addr: SocketAddr,
        timeout_secs: u64,
        reaped: Arc<AtomicBool>,
    ) {
        let start = std::time::Instant::now();
        let total_timeout = Duration::from_secs(timeout_secs);

        while start.elapsed() < total_timeout {
            // Abort polling immediately if the child process has already terminated
            if reaped.load(Ordering::SeqCst) {
                return;
            }

            if matches!(
                timeout(Duration::from_millis(500), TcpStream::connect(addr)).await,
                Ok(Ok(_))
            ) {
                info!(service = %name, addr = %addr, "Service TCP health check PASSED");
                let mut guard = services.write().await;
                if let Some(service) = guard.get_mut(&name) {
                    if service.status == ServiceStatus::Starting {
                        service.status = ServiceStatus::Healthy;
                    }
                }
                return;
            }
            sleep(Duration::from_millis(250)).await;
        }

        warn!(service = %name, addr = %addr, "Service TCP health check TIMED OUT");
        let mut guard = services.write().await;
        if let Some(service) = guard.get_mut(&name) {
            if service.status == ServiceStatus::Starting {
                service.status = ServiceStatus::Unhealthy("Health check timeout".into());
            }
        }
    }

    pub async fn stop_service(&self, name: &str) -> Result<(), ServiceRunnerError> {
        let mut services = self.services.write().await;
        if let Some(mut service) = services.remove(name) {
            if let Some(stop_tx) = service.stop_tx.take() {
                info!(service = %name, "Terminating background collateral service");
                let (ack_tx, ack_rx) = oneshot::channel();
                if stop_tx.send(ack_tx).is_ok() {
                    let _ = ack_rx.await;
                }
            }
            service.reaped.store(true, Ordering::SeqCst);
            Ok(())
        } else {
            Err(ServiceRunnerError::NotFound(name.to_string()))
        }
    }

    pub async fn get_status_all(&self) -> HashMap<String, ServiceStatus> {
        let services = self.services.read().await;
        services
            .iter()
            .map(|(k, v)| (k.clone(), v.status.clone()))
            .collect()
    }

    pub async fn get_status(&self, name: &str) -> Option<ServiceStatus> {
        let services = self.services.read().await;
        services.get(name).map(|s| s.status.clone())
    }

    pub async fn is_running(&self, name: &str) -> bool {
        let services = self.services.read().await;
        match services.get(name) {
            Some(s) => matches!(s.status, ServiceStatus::Starting | ServiceStatus::Healthy),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_service_duplicate_name_rejected() {
        let runner = ServiceRunner::new();
        let name = "dup-service-test".to_string();

        let (cmd, args) = if cfg!(windows) {
            (
                "cmd.exe".to_string(),
                vec!["/C".to_string(), "ping 127.0.0.1 -n 5".to_string()],
            )
        } else {
            ("sleep".to_string(), vec!["5".to_string()])
        };

        let config = ServiceConfig {
            name: name.clone(),
            command: cmd,
            args,
            envs: HashMap::new(),
            working_dir: std::env::temp_dir(),
            health_check_addr: None,
            health_check_timeout_secs: 1,
        };

        let res1 = runner.start_service(config.clone()).await;
        assert!(res1.is_ok());

        // Spawning again with identical name while starting/healthy must fail atomically with AlreadyRunning
        let res2 = runner.start_service(config).await;
        assert!(matches!(res2, Err(ServiceRunnerError::AlreadyRunning(_))));

        // Clean up
        let _ = runner.stop_service(&name).await;
    }

    #[tokio::test]
    async fn test_stop_nonexistent_service_returns_not_found() {
        let runner = ServiceRunner::new();
        let res = runner.stop_service("non-existent-service").await;
        assert!(matches!(res, Err(ServiceRunnerError::NotFound(_))));
    }

    #[tokio::test]
    async fn test_service_reservation_guard_rollback_on_drop() {
        let runner = ServiceRunner::new();
        let name = "service-rollback-test".to_string();

        {
            let mut services = runner.services.write().await;
            services.insert(
                name.clone(),
                ManagedService {
                    config: ServiceConfig {
                        name: name.clone(),
                        command: "test".to_string(),
                        args: vec![],
                        envs: HashMap::new(),
                        working_dir: std::env::temp_dir(),
                        health_check_addr: None,
                        health_check_timeout_secs: 1,
                    },
                    status: ServiceStatus::Starting,
                    stop_tx: None,
                    #[cfg(unix)]
                    pgid: None,
                    reaped: Arc::new(AtomicBool::new(false)),
                },
            );
        }

        assert_eq!(
            runner.get_status(&name).await,
            Some(ServiceStatus::Starting)
        );

        // Simulating reservation guard drop before commitment
        {
            let _guard = ReservationGuard {
                name: name.clone(),
                services: Arc::clone(&runner.services),
                committed: false,
            };
            // _guard drops here
        }

        // Service must be rolled back and removed
        assert_eq!(runner.get_status(&name).await, None);
    }

    #[test]
    fn test_service_pgid_safety_conversion() {
        let validate_pgid = |id: u32| -> Option<i32> {
            match i32::try_from(id) {
                Ok(v) if v > 1 => Some(v),
                _ => None,
            }
        };

        // IDs <= 1 must NEVER be accepted as signal targets (prevents supervisor self-suicide)
        assert_eq!(validate_pgid(0), None);
        assert_eq!(validate_pgid(1), None);
        assert_eq!(validate_pgid(u32::MAX), None);
        assert_eq!(validate_pgid(i32::MAX as u32 + 1), None);

        // Valid positive PIDs > 1 must pass
        assert_eq!(validate_pgid(2), Some(2));
        assert_eq!(validate_pgid(12345), Some(12345));
    }
}
