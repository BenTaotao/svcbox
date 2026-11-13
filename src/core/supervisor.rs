use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::core::{instance, AppContext};
use crate::error::{AppError, Result};
use crate::models::{InstanceRecord, InstanceStatus};
use crate::utils;

const STATUS_FILE: &str = "supervisor.status";
const SUPERVISOR_PID_FILE: &str = "supervisor.pid";
const STOP_FILE: &str = "stop.request";
const SERVICE_PID_FILE: &str = "service.pid";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SupervisorStatus {
    pub state: String,
    pub message: String,
    pub pid: Option<u32>,
    pub updated_at: String,
}

fn run_dir(rec: &InstanceRecord) -> PathBuf {
    rec.dir.join("run")
}

pub fn status_file(rec: &InstanceRecord) -> PathBuf {
    run_dir(rec).join(STATUS_FILE)
}

pub fn supervisor_pid_file(rec: &InstanceRecord) -> PathBuf {
    run_dir(rec).join(SUPERVISOR_PID_FILE)
}

pub fn stop_file(rec: &InstanceRecord) -> PathBuf {
    run_dir(rec).join(STOP_FILE)
}

pub fn service_pid_file(rec: &InstanceRecord) -> PathBuf {
    run_dir(rec).join(SERVICE_PID_FILE)
}

pub fn write_status(rec: &InstanceRecord, state: &str, message: &str, pid: Option<u32>) {
    if fs::create_dir_all(run_dir(rec)).is_err() {
        return;
    }
    let status = SupervisorStatus {
        state: state.to_string(),
        message: message.to_string(),
        pid,
        updated_at: utils::now_rfc3339(),
    };
    if let Ok(data) = serde_json::to_vec_pretty(&status) {
        let _ = fs::write(status_file(rec), data);
    }
}

pub fn read_status(rec: &InstanceRecord) -> Option<SupervisorStatus> {
    let data = fs::read(status_file(rec)).ok()?;
    serde_json::from_slice(&data).ok()
}

pub fn supervisor_pid(rec: &InstanceRecord) -> Option<u32> {
    fs::read_to_string(supervisor_pid_file(rec))
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn supervisor_alive(rec: &InstanceRecord) -> bool {
    let Some(pid) = supervisor_pid(rec) else {
        return false;
    };
    if !utils::pid_alive(pid) {
        return false;
    }
    let Some(expected) = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
    else {
        return false;
    };
    utils::pid_image_name(pid)
        .map(|name| name.eq_ignore_ascii_case(&expected))
        .unwrap_or(false)
}

/// 守护进程异常退出会留下 pid 文件；PID 可能被系统复用，启动前先清理死 pid。
pub fn clear_stale_pid(rec: &InstanceRecord) {
    if let Some(pid) = supervisor_pid(rec) {
        if !utils::pid_alive(pid) {
            let _ = fs::remove_file(supervisor_pid_file(rec));
        }
    }
}

pub fn service_pid(rec: &InstanceRecord) -> Option<u32> {
    fs::read_to_string(service_pid_file(rec))
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub fn write_service_pid(rec: &InstanceRecord, pid: u32) -> Result<()> {
    fs::create_dir_all(run_dir(rec))?;
    fs::write(service_pid_file(rec), pid.to_string())?;
    Ok(())
}

pub fn request_stop(rec: &InstanceRecord) -> Result<()> {
    fs::create_dir_all(run_dir(rec))?;
    fs::write(stop_file(rec), b"stop")?;
    Ok(())
}

/// CLI 启动时清掉上一轮守护进程留下的运行文件。
pub fn clear_runtime_files(rec: &InstanceRecord) {
    for file in [
        status_file(rec),
        supervisor_pid_file(rec),
        stop_file(rec),
        service_pid_file(rec),
    ] {
        let _ = fs::remove_file(file);
    }
}

/// 以脱离终端的方式拉起守护进程（CLI start / MySQL 重置后恢复运行共用）。
pub fn spawn_detached(ctx: &AppContext, rec: &InstanceRecord) -> Result<()> {
    let exe = std::env::current_exe()?;
    let log_path = rec.dir.join("logs").join("supervisor.log");
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;

    let mut cmd = Command::new(exe);
    cmd.arg("__supervise")
        .arg(&rec.id)
        .arg("--home")
        .arg(&ctx.paths.home)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
        cmd.creation_flags(0x0000_0008 | 0x0000_0200);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    cmd.spawn()
        .map_err(|e| AppError::other(format!("无法启动后台守护进程: {e}")))?;
    Ok(())
}

pub async fn wait_ready(rec: &InstanceRecord, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = read_status(rec) {
            match status.state.as_str() {
                "running" => return Ok(()),
                "error" | "exited" => return Err(AppError::other(status.message)),
                "stopped" => return Err(AppError::other("后台守护进程已停止")),
                _ => {}
            }
        }
        if Instant::now() >= deadline {
            return Err(AppError::other(format!(
                "等待服务启动超时，请查看 {}",
                rec.dir.join("logs").join("supervisor.log").display()
            )));
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
}

pub async fn wait_stopped(rec: &InstanceRecord, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let sup_alive = supervisor_alive(rec);
        let port_open = utils::port_open_sync(rec.port, Duration::from_millis(250));
        if !sup_alive && !port_open {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

/// 常驻守护进程：持有 JobObject，CLI 退出后服务仍在；收到 stop.request 或服务退出时收尾。
pub async fn run(ctx: Arc<AppContext>, instance_id: &str) -> Result<()> {
    let rec = ctx.require_instance(instance_id)?;
    fs::create_dir_all(run_dir(&rec))?;
    let _ = fs::remove_file(stop_file(&rec));
    write_status(&rec, "starting", "正在启动服务", None);

    let view = match instance::start(&ctx, instance_id).await {
        Ok(view) => view,
        Err(e) => {
            write_status(&rec, "error", &e.to_string(), None);
            return Err(e);
        }
    };
    // 启动成功后再落 supervisor.pid，避免 instance::start 把自己当成外部托管进程
    fs::write(supervisor_pid_file(&rec), std::process::id().to_string())?;
    if let Some(pid) = view.pid {
        let _ = write_service_pid(&rec, pid);
    }
    write_status(&rec, "running", "运行中", view.pid);

    loop {
        if stop_file(&rec).exists() {
            let _ = fs::remove_file(stop_file(&rec));
            let _ = instance::stop(&ctx, instance_id).await;
            write_status(&rec, "stopped", "已停止", None);
            break;
        }
        if !ctx.processes.is_alive(instance_id) {
            let _ = ctx
                .db
                .update_instance_status(instance_id, InstanceStatus::Crashed);
            write_status(&rec, "exited", "服务进程已退出", None);
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    let _ = fs::remove_file(supervisor_pid_file(&rec));
    let _ = fs::remove_file(service_pid_file(&rec));
    Ok(())
}
