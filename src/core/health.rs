use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::core::process::{ProcessManager, ProcessState};
use crate::models::{HealthReport, HealthStatus, InstanceRecord, ServiceKind};
use crate::utils;

/// 只匹配各服务真正的错误级别标记，避免把 Redis 的正常启动警告（如 "OS error:"）误判成故障。
fn error_regex(kind: ServiceKind) -> &'static Regex {
    static NGINX: OnceLock<Regex> = OnceLock::new();
    static MYSQL: OnceLock<Regex> = OnceLock::new();
    static REDIS: OnceLock<Regex> = OnceLock::new();
    match kind {
        ServiceKind::Nginx => {
            NGINX.get_or_init(|| Regex::new(r"\[(error|emerg|crit|alert)\]").unwrap())
        }
        ServiceKind::Mysql => {
            MYSQL.get_or_init(|| Regex::new(r"\[(ERROR|FATAL)\]|\bERROR [0-9]+\b").unwrap())
        }
        ServiceKind::Redis => REDIS.get_or_init(|| {
            Regex::new(r"(?m)(^|\s)(#\s*)?(Fatal|FATAL|Error|ERROR)\b|#\s*(Failed|Can't)\b")
                .unwrap()
        }),
    }
}

pub fn log_path(instance: &InstanceRecord) -> PathBuf {
    match instance.kind {
        ServiceKind::Nginx => instance.dir.join("logs").join("error.log"),
        ServiceKind::Mysql => instance.dir.join("logs").join("mysql-error.log"),
        ServiceKind::Redis => instance.dir.join("logs").join("redis.log"),
    }
}

pub fn log_has_error(kind: ServiceKind, path: &Path) -> bool {
    let tail = utils::read_log_tail(path, 64 * 1024);
    if kind == ServiceKind::Nginx {
        // 静态文件 404（favicon.ico 等）会以 [error] 级别记录，但不代表服务故障
        return tail.lines().any(|line| {
            error_regex(kind).is_match(line)
                && !line.contains("failed (2: The system cannot find the file specified)")
        });
    }
    error_regex(kind).is_match(&tail)
}

/// 三重检测：进程组存活 + 端口连通 + 日志报错。
pub async fn check(
    instance: &InstanceRecord,
    pm: &ProcessManager,
    expect_running: bool,
) -> HealthReport {
    let state = pm.state(&instance.id);
    match state {
        ProcessState::Running { pid } => {
            let port_open = utils::tcp_open(instance.port).await;
            let log_has_error = log_has_error(instance.kind, &log_path(instance));
            let (status, detail) = if !port_open {
                (
                    HealthStatus::WarningPortNotListen,
                    format!("进程存活，但端口 {} 未监听", instance.port),
                )
            } else if log_has_error {
                (
                    HealthStatus::ErrorLogHasError,
                    "进程与端口正常，但日志中出现 ERROR".to_string(),
                )
            } else {
                (HealthStatus::Running, "运行正常".to_string())
            };
            HealthReport {
                status,
                pid: Some(pid),
                port_open,
                log_has_error,
                detail,
            }
        }
        ProcessState::Exited { code } => {
            if expect_running {
                return external_or_crashed(instance, code).await;
            }
            HealthReport {
                status: HealthStatus::Stopped,
                pid: None,
                port_open: false,
                log_has_error: log_has_error(instance.kind, &log_path(instance)),
                detail: code
                    .map(|c| format!("进程已退出，退出码 {c}"))
                    .unwrap_or_else(|| "进程已退出".to_string()),
            }
        }
        ProcessState::Absent => {
            if expect_running {
                return external_or_crashed(instance, None).await;
            }
            HealthReport {
                status: HealthStatus::Stopped,
                pid: None,
                port_open: false,
                log_has_error: false,
                detail: "未启动".to_string(),
            }
        }
    }
}

/// 进程不在当前进程表里时，区分“CLI 守护进程在托管”和真正崩溃。
async fn external_or_crashed(instance: &InstanceRecord, exit_code: Option<i32>) -> HealthReport {
    let port_open = utils::tcp_open(instance.port).await;
    let supervisor_alive = crate::core::supervisor::supervisor_alive(instance);
    let service_match = crate::core::instance::service_pid_matches(instance);
    if port_open && (supervisor_alive || service_match) {
        let pid =
            crate::core::supervisor::service_pid(instance).filter(|pid| utils::pid_alive(*pid));
        let log_err = log_has_error(instance.kind, &log_path(instance));
        return HealthReport {
            status: if log_err {
                HealthStatus::ErrorLogHasError
            } else {
                HealthStatus::Running
            },
            pid,
            port_open: true,
            log_has_error: log_err,
            detail: if log_err {
                "端口可连通，但日志中出现 ERROR".to_string()
            } else {
                "端口可连通（由外部守护进程托管）".to_string()
            },
        };
    }
    if supervisor_alive {
        return HealthReport {
            status: HealthStatus::WarningPortNotListen,
            pid: None,
            port_open: false,
            log_has_error: log_has_error(instance.kind, &log_path(instance)),
            detail: "守护进程存在，但端口尚未监听".to_string(),
        };
    }
    HealthReport {
        status: HealthStatus::Crashed,
        pid: None,
        port_open,
        log_has_error: log_has_error(instance.kind, &log_path(instance)),
        detail: if port_open {
            "服务进程不在，端口可能被其他程序占用".to_string()
        } else {
            exit_code
                .map(|c| format!("进程已退出，退出码 {c}"))
                .unwrap_or_else(|| "进程不存在，可能已崩溃".to_string())
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn detects_error_keywords() {
        let path = std::env::temp_dir().join(format!("svcbox-log-{}", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "2026/01/01 [notice] start").unwrap();
        writeln!(f, "2026/01/01 [error] bind() failed").unwrap();
        f.flush().unwrap();
        assert!(log_has_error(ServiceKind::Nginx, &path));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn redis_startup_warnings_are_not_errors() {
        let path = std::env::temp_dir().join(format!("svcbox-redis-log-{}", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(
            f,
            "390:M 13 Nov 2026 16:34:23.229 # You requested maxclients of 10000"
        )
        .unwrap();
        writeln!(
            f,
            "# Server can't set maximum open files because of OS error: Operation not permitted"
        )
        .unwrap();
        writeln!(f, "# WARNING: Redis does not require authentication").unwrap();
        writeln!(f, "* Ready to accept connections tcp").unwrap();
        f.flush().unwrap();
        assert!(!log_has_error(ServiceKind::Redis, &path));
        let _ = std::fs::remove_file(path);
    }
}
