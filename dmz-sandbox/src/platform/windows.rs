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

struct JobHandle(HANDLE);

impl Drop for JobHandle {
    fn drop(&mut self) {
        if self.0 != 0 {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

pub fn run_sandboxed(config: &SandboxConfig) -> Result<ExitStatus, PlatformError> {
    let canonical_workspace =
        fs::canonicalize(&config.workspace_path).unwrap_or_else(|_| config.workspace_path.clone());

    // 1. Create a Win32 Job Object for process containment
    let job = unsafe {
        let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if h == 0 {
            return Err(PlatformError::WindowsError(
                "Failed to create Win32 Job Object".into(),
            ));
        }
        JobHandle(h)
    };

    // 2. Configure Job Object to automatically kill all child processes when the handle is closed
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
            return Err(PlatformError::WindowsError(
                "Failed to set JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE on Job Object".into(),
            ));
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

    // 3. Assign spawned process to Job Object
    unsafe {
        let process_handle = child.as_raw_handle() as HANDLE;
        if AssignProcessToJobObject(job.0, process_handle) == 0 {
            let _ = child.kill();
            return Err(PlatformError::WindowsError(
                "Failed to assign process to Win32 Job Object".into(),
            ));
        }
    }

    let status = child.wait().map_err(|err| {
        PlatformError::WindowsError(format!("Failed to wait for Windows process: {}", err))
    })?;

    Ok(status)
}
