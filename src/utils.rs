use crate::error::{AppError, Result};
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339()
}

pub fn now_stamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

/// 实例 ID 会直接作为目录名，必须是安全的 ASCII 片段。
pub fn validate_instance_id(id: &str) -> Result<()> {
    let ok = !id.is_empty()
        && id.len() <= 40
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(AppError::validation(
            "实例名只能包含小写字母、数字、-、_，长度 1-40（会作为目录名）",
        ))
    }
}

pub fn validate_domain(domain: &str) -> Result<()> {
    let d = domain.trim().trim_end_matches('.');
    if d.is_empty() || d.len() > 253 || !d.contains('.') {
        return Err(AppError::validation("域名格式不合法"));
    }
    for label in d.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(AppError::validation("域名标签长度不合法"));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(AppError::validation("域名标签不能以 - 开头或结尾"));
        }
        if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(AppError::validation("域名只能包含字母、数字、-、."));
        }
    }
    Ok(())
}

/// 同时用作 nginx server_name 与站点文件名，支持 `_` 兜底与常规主机名。
pub fn validate_server_name(name: &str) -> Result<()> {
    if name == "_" {
        return Ok(());
    }
    validate_domain(name)
}

pub fn validate_db_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$');
    if ok {
        Ok(())
    } else {
        Err(AppError::validation(
            "库名只能包含字母、数字、_、$，长度 1-64",
        ))
    }
}

pub fn validate_db_user(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 32
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.');
    if ok {
        Ok(())
    } else {
        Err(AppError::validation(
            "账号只能包含字母、数字、_、-、.，长度 1-32",
        ))
    }
}

pub fn validate_db_host(host: &str) -> Result<()> {
    match host {
        "localhost" | "%" | "127.0.0.1" => Ok(()),
        _ => Err(AppError::validation(
            "账号 host 仅支持 localhost、127.0.0.1 或 %",
        )),
    }
}

pub fn validate_password(pwd: &str) -> Result<()> {
    if pwd.is_empty() {
        return Err(AppError::validation("密码不能为空"));
    }
    if pwd.len() > 128 {
        return Err(AppError::validation("密码长度不能超过 128"));
    }
    if pwd.chars().any(|c| c.is_control()) {
        return Err(AppError::validation("密码不能包含控制字符"));
    }
    Ok(())
}

/// Redis 配置文件使用双引号包裹密码，这里拒绝会破坏配置的字符。
pub fn validate_redis_password(pwd: &str) -> Result<()> {
    if pwd.is_empty() {
        return Ok(());
    }
    validate_password(pwd)?;
    if pwd.contains('"') || pwd.contains('\\') || pwd.contains('\n') {
        return Err(AppError::validation(
            "Redis 密码不能包含双引号、反斜杠或换行",
        ));
    }
    Ok(())
}

pub fn validate_bind(bind: &str) -> Result<()> {
    bind.parse::<IpAddr>()
        .map(|_| ())
        .map_err(|_| AppError::validation("bind 必须是合法的 IP 地址"))
}

pub fn validate_port(port: u16) -> Result<()> {
    if port == 0 {
        return Err(AppError::validation("端口必须在 1-65535 之间"));
    }
    Ok(())
}

/// 前置端口占用检测：能 bind 成功即认为可用。
pub fn port_available(port: u16) -> Result<bool> {
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => Ok(false),
        Err(e) => Err(e.into()),
    }
}

pub fn ensure_port_available(port: u16) -> Result<()> {
    validate_port(port)?;
    if !port_available(port)? {
        return Err(AppError::conflict(format!(
            "端口 {port} 已被占用，请更换端口"
        )));
    }
    Ok(())
}

pub async fn tcp_open(port: u16) -> bool {
    let addr = format!("127.0.0.1:{port}");
    matches!(
        tokio::time::timeout(
            std::time::Duration::from_millis(1200),
            tokio::net::TcpStream::connect(&addr)
        )
        .await,
        Ok(Ok(_))
    )
}

pub fn read_log_tail(path: &Path, max_bytes: u64) -> String {
    let Ok(mut f) = fs::File::open(path) else {
        return String::new();
    };
    let Ok(len) = f.metadata().map(|m| m.len()) else {
        return String::new();
    };
    let start = len.saturating_sub(max_bytes);
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::with_capacity((len - start) as usize);
    let _ = f.read_to_end(&mut buf);
    String::from_utf8_lossy(&buf).to_string()
}

pub fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Nginx / MySQL 配置文件统一使用正斜杠路径。
pub fn forward(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

pub fn run_tool(
    tool: &Path,
    args: &[String],
    cwd: Option<&Path>,
    envs: &[(&str, String)],
) -> Result<Output> {
    let mut cmd = Command::new(tool);
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    for (k, v) in envs {
        cmd.env(k, v);
    }
    // GUI 进程下调用控制台程序（nginx -t / mysql / mysqladmin / bash 等）时不要弹黑框
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output().map_err(|e| {
        AppError::other(format!(
            "无法启动 {}: {e}",
            tool.file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| tool.display().to_string())
        ))
    })
}

pub fn output_ok(tool: &Path, out: Output) -> Result<String> {
    if out.status.success() {
        return Ok(String::from_utf8_lossy(&out.stdout).to_string());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Err(AppError::External {
        tool: tool
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| tool.display().to_string()),
        code: out.status.code().unwrap_or(-1),
        stderr: if stderr.is_empty() { stdout } else { stderr },
    })
}

pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".to_string())
            .split(';')
            .map(|s| s.to_ascii_lowercase())
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        let direct = dir.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        if cfg!(windows) && !name.contains('.') {
            for ext in &exts {
                let candidate = dir.join(format!("{name}{ext}"));
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// Windows 上 acme.sh 依赖 bash（Git for Windows 自带）。
pub fn find_bash() -> Option<PathBuf> {
    find_in_path("bash").or_else(|| {
        if cfg!(windows) {
            for base in [
                r"C:\Program Files\Git\bin\bash.exe",
                r"C:\Program Files (x86)\Git\bin\bash.exe",
            ] {
                let p = PathBuf::from(base);
                if p.is_file() {
                    return Some(p);
                }
            }
        }
        None
    })
}

pub fn random_password(len: usize) -> String {
    const CHARSET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz23456789!@#%^*-_+";
    let mut buf = vec![0u8; len];
    getrandom::fill(&mut buf).expect("系统随机数不可用");
    buf.into_iter()
        .map(|b| CHARSET[b as usize % CHARSET.len()] as char)
        .collect()
}

pub fn port_open_sync(port: u16, timeout: std::time::Duration) -> bool {
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    std::net::TcpStream::connect_timeout(&addr, timeout).is_ok()
}

#[cfg(windows)]
mod win_pid {
    use std::ffi::c_void;
    pub type Handle = *mut c_void;
    pub const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    #[link(name = "kernel32")]
    extern "system" {
        pub fn OpenProcess(access: u32, inherit: i32, pid: u32) -> Handle;
        pub fn GetExitCodeProcess(process: Handle, code: *mut u32) -> i32;
        pub fn QueryFullProcessImageNameW(
            process: Handle,
            flags: u32,
            name: *mut u16,
            size: *mut u32,
        ) -> i32;
        pub fn CloseHandle(handle: Handle) -> i32;
    }
}

#[cfg(windows)]
pub fn pid_alive(pid: u32) -> bool {
    use win_pid::*;
    const STILL_ACTIVE: u32 = 259;
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return false;
        }
        let mut code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        ok != 0 && code == STILL_ACTIVE
    }
}

/// 进程映像文件名（不含路径），用于避免 PID 复用导致的误判。
#[cfg(windows)]
pub fn pid_image_name(pid: u32) -> Option<String> {
    use std::path::Path;
    use win_pid::*;
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(handle);
        if ok == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..size as usize]);
        Path::new(&path)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
    }
}

#[cfg(unix)]
pub fn pid_alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(unix)]
pub fn pid_image_name(pid: u32) -> Option<String> {
    let path = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    path.file_name().map(|s| s.to_string_lossy().to_string())
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    Ok(())
}

pub fn remove_dir_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path)?;
    }
    Ok(())
}

/// 审计日志：所有配置、账号、证书变更都落盘，密码类字段脱敏。
pub fn audit(
    dir: &Path,
    action: &str,
    mut params: Value,
    outcome: &std::result::Result<(), String>,
) {
    redact(&mut params);
    let entry = json!({
        "ts": now_rfc3339(),
        "action": action,
        "params": params,
        "ok": outcome.is_ok(),
        "error": outcome.as_ref().err().cloned(),
    });
    let log_dir = dir.join("logs");
    if fs::create_dir_all(&log_dir).is_err() {
        return;
    }
    let line = format!("{entry}\n");
    if let Ok(mut f) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_dir.join("audit.log"))
    {
        let _ = f.write_all(line.as_bytes());
    }
}

fn redact(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (k, v) in map.iter_mut() {
                let lower = k.to_ascii_lowercase();
                if lower.contains("pass") || lower.contains("pwd") || lower.contains("secret") {
                    *v = Value::String("***".to_string());
                } else {
                    redact(v);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(redact),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_id_rejects_path_traversal() {
        assert!(validate_instance_id("../etc").is_err());
        assert!(validate_instance_id("dev_1").is_ok());
        assert!(validate_instance_id("").is_err());
        assert!(validate_instance_id("Dev").is_err());
        assert!(validate_instance_id("dev").is_ok());
    }

    #[test]
    fn domain_validation() {
        assert!(validate_domain("example.com").is_ok());
        assert!(validate_domain("sub.example.com").is_ok());
        assert!(validate_domain("-bad.com").is_err());
        assert!(validate_domain("localhost").is_err());
        assert!(validate_domain("a b.com").is_err());
    }

    #[test]
    fn db_identifier_validation() {
        assert!(validate_db_name("test_db").is_ok());
        assert!(validate_db_name("bad;drop").is_err());
        assert!(validate_db_user("app.user").is_ok());
        assert!(validate_db_host("localhost").is_ok());
        assert!(validate_db_host("0.0.0.0").is_err());
    }

    #[test]
    fn redacts_passwords_in_audit() {
        let mut v = json!({"password": "123", "nested": {"root_pwd": "x"}, "name": "ok"});
        redact(&mut v);
        assert_eq!(v["password"], "***");
        assert_eq!(v["nested"]["root_pwd"], "***");
        assert_eq!(v["name"], "ok");
    }
}
