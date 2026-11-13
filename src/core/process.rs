use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::error::{AppError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessState {
    Running { pid: u32 },
    Exited { code: Option<i32> },
    Absent,
}

pub struct SpawnOptions {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub log_path: PathBuf,
}

struct ManagedProcess {
    child: Child,
    pid: u32,
    #[cfg(windows)]
    job: usize,
    started_at: String,
}

impl ManagedProcess {
    fn try_state(&mut self) -> ProcessState {
        match self.child.try_wait() {
            Ok(Some(status)) => ProcessState::Exited {
                code: status.code(),
            },
            Ok(None) => ProcessState::Running { pid: self.pid },
            Err(_) => ProcessState::Exited { code: None },
        }
    }

    fn force_kill(&self) {
        #[cfg(windows)]
        win_job::terminate(self.job as win_job::Handle);
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.pid as i32), libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
impl Drop for ManagedProcess {
    fn drop(&mut self) {
        // 关闭 JobObject 时 KILL_ON_JOB_CLOSE 生效，子进程与孙进程一锅端
        win_job::close(self.job as win_job::Handle);
    }
}

#[cfg(unix)]
impl Drop for ManagedProcess {
    fn drop(&mut self) {
        // 进程组兜底，避免 svcbox 退出后残留服务进程
        unsafe {
            libc::kill(-(self.pid as i32), libc::SIGKILL);
        }
    }
}

/// 进程组托管核心：Windows 用 JobObject，Linux 用进程组。
pub struct ProcessManager {
    inner: Mutex<HashMap<String, ManagedProcess>>,
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessManager {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, ManagedProcess>>> {
        self.inner
            .lock()
            .map_err(|_| AppError::other("进程表锁已损坏"))
    }

    pub fn spawn(&self, id: &str, opts: SpawnOptions) -> Result<u32> {
        {
            let mut map = self.lock()?;
            if let Some(p) = map.get_mut(id) {
                if matches!(p.try_state(), ProcessState::Running { .. }) {
                    return Err(AppError::conflict(format!("实例 {id} 的进程已在运行")));
                }
                map.remove(id);
            }
        }

        if let Some(parent) = opts.log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let log: File = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&opts.log_path)?;

        let mut cmd = Command::new(&opts.program);
        cmd.args(&opts.args)
            .current_dir(&opts.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));

        for (k, v) in &opts.env {
            cmd.env(k, v);
        }

        // 服务本身是控制台程序，从 GUI 启动时必须显式隐藏控制台窗口
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }

        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            cmd.process_group(0);
        }

        let mut child = cmd.spawn().map_err(|e| {
            AppError::other(format!(
                "无法启动 {}: {e}",
                opts.program
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| opts.program.display().to_string())
            ))
        })?;
        let pid = child.id();

        #[cfg(windows)]
        let job = {
            use std::os::windows::io::AsRawHandle;
            let job = win_job::create()?;
            if let Err(e) = win_job::assign(
                job as win_job::Handle,
                child.as_raw_handle() as win_job::Handle,
            ) {
                let _ = child.kill();
                win_job::close(job as win_job::Handle);
                return Err(e.into());
            }
            job
        };

        let managed = ManagedProcess {
            child,
            pid,
            #[cfg(windows)]
            job: job as usize,
            started_at: crate::utils::now_rfc3339(),
        };
        self.lock()?.insert(id.to_string(), managed);
        Ok(pid)
    }

    pub fn state(&self, id: &str) -> ProcessState {
        let Ok(mut map) = self.lock() else {
            return ProcessState::Absent;
        };
        let Some(proc) = map.get_mut(id) else {
            return ProcessState::Absent;
        };
        let state = proc.try_state();
        if matches!(state, ProcessState::Exited { .. }) {
            map.remove(id);
        }
        state
    }

    pub fn is_alive(&self, id: &str) -> bool {
        matches!(self.state(id), ProcessState::Running { .. })
    }

    pub fn pid(&self, id: &str) -> Option<u32> {
        match self.state(id) {
            ProcessState::Running { pid } => Some(pid),
            _ => None,
        }
    }

    pub fn started_at(&self, id: &str) -> Option<String> {
        let map = self.lock().ok()?;
        map.get(id).map(|p| p.started_at.clone())
    }

    pub fn force_kill(&self, id: &str) {
        if let Ok(map) = self.lock() {
            if let Some(p) = map.get(id) {
                p.force_kill();
            }
        }
    }

    pub fn remove(&self, id: &str) {
        if let Ok(mut map) = self.lock() {
            map.remove(id);
        }
    }

    /// 等待进程退出，返回是否在超时前退出。
    pub async fn wait_exit(&self, id: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match self.state(id) {
                ProcessState::Running { .. } => {}
                _ => return true,
            }
            if Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// 强制销毁整个进程组，并等待回收。
    pub async fn kill_tree(&self, id: &str) {
        self.force_kill(id);
        let _ = self.wait_exit(id, Duration::from_secs(3)).await;
        self.remove(id);
    }

    pub fn running_ids(&self) -> Vec<String> {
        self.lock()
            .map(|map| map.keys().cloned().collect())
            .unwrap_or_default()
    }
}

#[cfg(windows)]
mod win_job {
    use std::ffi::c_void;

    pub type Handle = *mut c_void;
    type Bool = i32;

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;

    #[repr(C)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    struct ExtendedLimitInformation {
        basic_limit_information: BasicLimitInformation,
        io_info: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, len: u32) -> Bool;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> Bool;
        fn TerminateJobObject(job: Handle, exit_code: u32) -> Bool;
        fn CloseHandle(handle: Handle) -> Bool;
    }

    pub fn create() -> std::io::Result<Handle> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mut info: ExtendedLimitInformation = std::mem::zeroed();
            info.basic_limit_information.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                job,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION_CLASS,
                &info as *const _ as *const c_void,
                std::mem::size_of::<ExtendedLimitInformation>() as u32,
            );
            if ok == 0 {
                let err = std::io::Error::last_os_error();
                CloseHandle(job);
                return Err(err);
            }
            Ok(job)
        }
    }

    pub fn assign(job: Handle, process: Handle) -> std::io::Result<()> {
        unsafe {
            if AssignProcessToJobObject(job, process) == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }
    }

    pub fn terminate(job: Handle) {
        unsafe {
            TerminateJobObject(job, 1);
        }
    }

    pub fn close(job: Handle) {
        unsafe {
            CloseHandle(job);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detects_process_exit() {
        let pm = ProcessManager::new();
        let log = std::env::temp_dir().join("svcbox-proc-test.log");
        #[cfg(windows)]
        let (program, args) = (
            PathBuf::from("cmd.exe"),
            vec!["/C".to_string(), "ping -n 2 127.0.0.1 > nul".to_string()],
        );
        #[cfg(unix)]
        let (program, args) = (
            PathBuf::from("sh"),
            vec!["-c".to_string(), "sleep 1".to_string()],
        );

        pm.spawn(
            "t",
            SpawnOptions {
                program,
                args,
                cwd: std::env::temp_dir(),
                env: vec![],
                log_path: log,
            },
        )
        .unwrap();
        assert!(pm.is_alive("t"));
        assert!(pm.wait_exit("t", Duration::from_secs(10)).await);
        assert_eq!(pm.state("t"), ProcessState::Absent);
    }
}
