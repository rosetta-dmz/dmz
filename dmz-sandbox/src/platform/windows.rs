use crate::platform::{PlatformError, SandboxConfig, SandboxIsolationMode, SandboxPolicyOptions};
use std::fs;
use std::os::windows::io::AsRawHandle;
use std::process::{Command, ExitStatus, Stdio};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Threading::GetExitCodeProcess;

/// RAII handle guarding a Win32 Job Object configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
pub struct WindowsJobGuard(HANDLE);

// SAFETY: A Win32 kernel Job Object HANDLE can safely be sent across threads and closed from another thread.
unsafe impl Send for WindowsJobGuard {}
// SAFETY: Shared references do not mutate the underlying kernel handle value.
unsafe impl Sync for WindowsJobGuard {}

impl Drop for WindowsJobGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` is a valid non-null Win32 Job Object handle owned exclusively by `WindowsJobGuard`.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

impl WindowsJobGuard {
    pub fn create_kill_on_close() -> Result<Self, PlatformError> {
        // SAFETY: Passing null security attributes and null name creates an unnamed Win32 Job Object.
        let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if h.is_null() {
            return Err(PlatformError::WindowsError(format!(
                "Failed to create Win32 Job Object: {}",
                std::io::Error::last_os_error()
            )));
        }
        let guard = Self(h);

        // SAFETY: `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` is plain-old-data valid when zero-initialized,
        // and `guard.0` is a verified non-null Job Object handle.
        unsafe {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let res = SetInformationJobObject(
                guard.0,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .unwrap_or(u32::MAX),
            );
            if res == 0 {
                return Err(PlatformError::WindowsError(format!(
                    "Failed to set JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE on Job Object: {}",
                    std::io::Error::last_os_error()
                )));
            }
        }
        Ok(guard)
    }

    pub fn assign_raw_handle(&self, process_handle: HANDLE) -> Result<(), PlatformError> {
        // SAFETY: `self.0` is a valid Job Object handle and `process_handle` is a valid Win32 process handle.
        unsafe {
            if AssignProcessToJobObject(self.0, process_handle) == 0 {
                let os_err = std::io::Error::last_os_error();
                let mut exit_code: u32 = 0;
                let still_active =
                    GetExitCodeProcess(process_handle, &mut exit_code) != 0 && exit_code == 259;
                if still_active {
                    return Err(PlatformError::WindowsError(format!(
                        "Failed to assign process to Win32 Job Object: {}",
                        os_err
                    )));
                }
            }
        }
        Ok(())
    }
}

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    run_sandboxed_with_options(config, &SandboxPolicyOptions::default())
}

pub fn build_command(
    config: &SandboxConfig,
    options: &SandboxPolicyOptions,
) -> Result<Command, PlatformError> {
    if !config.workspace_path.exists() {
        return Err(PlatformError::WindowsError(format!(
            "Workspace path does not exist: {:?}",
            config.workspace_path
        )));
    }

    let canonical_workspace =
        fs::canonicalize(&config.workspace_path).unwrap_or_else(|_| config.workspace_path.clone());

    if let Some(ref sock_path) = config.ipc_socket_path {
        let sock_str = sock_path.to_string_lossy();
        if !sock_str.starts_with(r"\\.\pipe\") && !sock_str.starts_with(r"\\?\pipe\") {
            return Err(PlatformError::WindowsError(format!(
                "Invalid Windows IPC socket path: {:?}. Must use Named Pipe format (e.g. \\\\.\\pipe\\dmz-<name>)",
                sock_path
            )));
        }
    }

    if !config.allow_network {
        if options.isolation_mode == SandboxIsolationMode::StrictKernel {
            return Err(PlatformError::UnsupportedPlatform(
                "Windows Job Objects do not provide kernel-level network isolation; use WSL2 Linux namespaces or SandboxIsolationMode::BestEffort".into(),
            ));
        }
        tracing::warn!(
            "Windows Job Objects confine process hierarchy and resources; \
             network isolation relies on cooperative offline/proxy environment variables on Windows."
        );
    }

    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args);
    cmd.envs(config.envs.iter().map(|(k, v)| (k, v)));
    cmd.current_dir(&canonical_workspace);
    Ok(cmd)
}

pub fn run_sandboxed_with_options(
    config: &SandboxConfig,
    options: &SandboxPolicyOptions,
) -> Result<ExitStatus, PlatformError> {
    let job = WindowsJobGuard::create_kill_on_close()?;
    let mut cmd = build_command(config, options)?;
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    let mut child = cmd.spawn().map_err(|err| {
        PlatformError::WindowsError(format!("Failed to spawn Windows process: {}", err))
    })?;

    if let Err(err) = job.assign_raw_handle(child.as_raw_handle()) {
        let _ = child.kill();
        return Err(err);
    }

    let status = child.wait().map_err(|err| {
        PlatformError::WindowsError(format!("Failed to wait for Windows process: {}", err))
    })?;

    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_windows_strict_kernel_fails_closed_and_job_guard_allocates() {
        let dir = tempfile::tempdir().unwrap();
        let config = SandboxConfig::new(dir.path(), PathBuf::from("C:\\nix\\store"), "cmd.exe");

        let strict_opts = SandboxPolicyOptions {
            isolation_mode: SandboxIsolationMode::StrictKernel,
            proxy_addr: None,
            writable_paths: Vec::new(),
        };
        assert!(matches!(
            build_command(&config, &strict_opts),
            Err(PlatformError::UnsupportedPlatform(_))
        ));

        let best_effort_opts = SandboxPolicyOptions {
            isolation_mode: SandboxIsolationMode::BestEffort,
            proxy_addr: None,
            writable_paths: Vec::new(),
        };
        assert!(build_command(&config, &best_effort_opts).is_ok());

        let guard = WindowsJobGuard::create_kill_on_close().expect("create Win32 Job Object");
        drop(guard);
    }
}
