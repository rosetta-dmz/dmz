use crate::platform::{PlatformError, SandboxConfig};
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

struct JobHandle(HANDLE);

impl Drop for JobHandle {
    fn drop(&mut self) {
        if self.0 != 0 {
            // `self.0` is a valid non-null Win32 Job Object handle owned exclusively by `JobHandle`.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    // Validate workspace path exists
    if !config.workspace_path.exists() {
        return Err(PlatformError::WindowsError(format!(
            "Workspace path does not exist: {:?}",
            config.workspace_path
        )));
    }

    let canonical_workspace =
        fs::canonicalize(&config.workspace_path).unwrap_or_else(|_| config.workspace_path.clone());

    // Validate Windows Named Pipe format if IPC socket path is provided
    if let Some(ref sock_path) = config.ipc_socket_path {
        let sock_str = sock_path.to_string_lossy();
        if !sock_str.starts_with(r"\\.\pipe\") && !sock_str.starts_with(r"\\?\pipe\") {
            return Err(PlatformError::WindowsError(format!(
                "Invalid Windows IPC socket path: {:?}. Must use Named Pipe format (e.g. \\\\.\\pipe\\dmz-<name>)",
                sock_path
            )));
        }
    }

    // LSP Invariant Warning: Win32 Job Objects restrict process lifecycles, not TCP/IP network sockets
    if !config.allow_network {
        tracing::warn!(
            "Windows Job Objects confine process hierarchy and resources; \
             network isolation relies on cooperative offline environment variables on Windows."
        );
    }

    // Create Win32 Job Object for process tree containment
    // Passing null security attributes and null name creates an unnamed Win32 Job Object.
    let job = unsafe {
        let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if h == 0 {
            return Err(PlatformError::WindowsError(format!(
                "Failed to create Win32 Job Object: {}",
                std::io::Error::last_os_error()
            )));
        }
        JobHandle(h)
    };

    // Configure Job Object to automatically terminate all child processes when the handle is closed
    // `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` is a plain-old-data C struct valid when zero-initialized,
    // and `job.0` is a verified non-null Job Object handle.
    unsafe {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let res = SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if res == 0 {
            return Err(PlatformError::WindowsError(format!(
                "Failed to set JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE on Job Object: {}",
                std::io::Error::last_os_error()
            )));
        }
    }

    let mut cmd = Command::new(&config.command);
    cmd.args(&config.args);
    cmd.envs(config.envs.iter().map(|(k, v)| (k, v)));
    cmd.current_dir(&canonical_workspace);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    cmd.stderr(Stdio::inherit());

    let mut child = cmd.spawn().map_err(|err| {
        PlatformError::WindowsError(format!("Failed to spawn Windows process: {}", err))
    })?;

    // Assign spawned process to Job Object with race defense against premature child exit
    // Both `job.0` and `child.as_raw_handle()` are valid open Win32 kernel handles.
    unsafe {
        let process_handle = child.as_raw_handle() as HANDLE;
        if AssignProcessToJobObject(job.0, process_handle) == 0 {
            let os_err = std::io::Error::last_os_error();

            // Check if failure occurred simply because the child process exited rapidly
            let mut exit_code: u32 = 0;
            let still_active =
                GetExitCodeProcess(process_handle, &mut exit_code) != 0 && exit_code == 259; // STILL_ACTIVE = 259

            if still_active {
                let _ = child.kill();
                return Err(PlatformError::WindowsError(format!(
                    "Failed to assign process to Win32 Job Object: {}",
                    os_err
                )));
            }
        }
    }

    let status = child.wait().map_err(|err| {
        PlatformError::WindowsError(format!("Failed to wait for Windows process: {}", err))
    })?;

    Ok(status)
}
