use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::core::health;
use crate::core::process::SpawnOptions;
use crate::core::service::{mysql, mysql_account, nginx, redis};
use crate::core::{version, AppContext};
use crate::error::{AppError, Result};
use crate::models::{
    CreateInstanceOptions, CreateInstanceResult, HealthReport, HealthStatus, InstanceConfig,
    InstanceRecord, InstanceStatus, InstanceView, LogChunk, ServiceKind,
};
use crate::utils;

pub fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn version_bin(ctx: &AppContext, rec: &InstanceRecord, name: &str) -> Result<PathBuf> {
    let dir = version::ensure_installed(ctx, rec.kind, &rec.version)?;
    let path = dir.join(name);
    if !path.is_file() {
        return Err(AppError::not_found(format!(
            "{} 缺少可执行文件 {}",
            rec.kind,
            path.display()
        )));
    }
    Ok(path)
}

pub fn nginx_exe(ctx: &AppContext, rec: &InstanceRecord) -> Result<PathBuf> {
    version_bin(ctx, rec, &exe_name("nginx"))
}

pub fn redis_server_exe(ctx: &AppContext, rec: &InstanceRecord) -> Result<PathBuf> {
    version_bin(ctx, rec, &exe_name("redis-server"))
}

pub fn mysqld_exe(ctx: &AppContext, rec: &InstanceRecord) -> Result<PathBuf> {
    let dir = version::ensure_installed(ctx, rec.kind, &rec.version)?;
    let path = dir.join("bin").join(exe_name("mysqld"));
    if !path.is_file() {
        return Err(AppError::not_found(format!(
            "MySQL 缺少 mysqld: {}",
            path.display()
        )));
    }
    Ok(path)
}

pub fn mysql_client_exe(ctx: &AppContext, rec: &InstanceRecord) -> Result<PathBuf> {
    let dir = version::ensure_installed(ctx, rec.kind, &rec.version)?;
    let path = dir.join("bin").join(exe_name("mysql"));
    if !path.is_file() {
        return Err(AppError::not_found(format!(
            "MySQL 缺少 mysql 客户端: {}",
            path.display()
        )));
    }
    Ok(path)
}

pub fn mysqladmin_exe(ctx: &AppContext, rec: &InstanceRecord) -> Result<PathBuf> {
    let dir = version::ensure_installed(ctx, rec.kind, &rec.version)?;
    let path = dir.join("bin").join(exe_name("mysqladmin"));
    if !path.is_file() {
        return Err(AppError::not_found(format!(
            "MySQL 缺少 mysqladmin: {}",
            path.display()
        )));
    }
    Ok(path)
}

pub fn main_config_path(rec: &InstanceRecord) -> PathBuf {
    match rec.kind {
        ServiceKind::Nginx => rec.dir.join("conf").join("nginx.conf"),
        ServiceKind::Mysql => rec.dir.join("conf").join("my.ini"),
        ServiceKind::Redis => rec.dir.join("conf").join("redis.conf"),
    }
}

pub fn console_log_path(rec: &InstanceRecord) -> PathBuf {
    rec.dir.join("logs").join("console.log")
}

pub fn cert_dir(rec: &InstanceRecord, domain: &str) -> PathBuf {
    rec.dir.join("certs").join(domain)
}

pub fn shared_memory_name(id: &str) -> String {
    let sanitized: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("svcbox_{}", sanitized).chars().take(48).collect()
}

fn make_dirs(kind: ServiceKind, dir: &Path) -> Result<()> {
    let dirs: Vec<PathBuf> = match kind {
        ServiceKind::Nginx => vec![
            dir.join("conf").join("sites"),
            dir.join("conf").join("sites_meta"),
            dir.join("conf").join("backup"),
            dir.join("logs"),
            dir.join("run"),
            dir.join("certs"),
            dir.join("www"),
            dir.join(".well-known").join("acme-challenge"),
            dir.join("temp").join("client_body_temp"),
            dir.join("temp").join("proxy_temp"),
            dir.join("temp").join("fastcgi_temp"),
            dir.join("temp").join("uwsgi_temp"),
            dir.join("temp").join("scgi_temp"),
        ],
        ServiceKind::Mysql => vec![
            dir.join("conf").join("backup"),
            dir.join("data"),
            dir.join("logs"),
            dir.join("run"),
            dir.join("tmp"),
        ],
        ServiceKind::Redis => vec![
            dir.join("conf").join("backup"),
            dir.join("data"),
            dir.join("logs"),
            dir.join("run"),
        ],
    };
    for d in dirs {
        fs::create_dir_all(d)?;
    }
    Ok(())
}

pub fn view(rec: &InstanceRecord, report: &HealthReport) -> InstanceView {
    let urls = match rec.kind {
        ServiceKind::Nginx => vec![format!("http://localhost:{}", rec.port)],
        ServiceKind::Mysql => vec![format!("mysql://127.0.0.1:{}", rec.port)],
        ServiceKind::Redis => vec![format!("redis://127.0.0.1:{}", rec.port)],
    };
    InstanceView {
        id: rec.id.clone(),
        kind: rec.kind,
        name: rec.name.clone(),
        version: rec.version.clone(),
        port: rec.port,
        https_port: if rec.kind == ServiceKind::Nginx {
            rec.config.nginx.https_port
        } else {
            0
        },
        status: rec.status,
        health: report.status,
        pid: report.pid,
        dir: rec.dir.to_string_lossy().to_string(),
        urls,
        has_root_password: rec.config.root_password_enc.is_some(),
    }
}

pub async fn create(ctx: &AppContext, opts: CreateInstanceOptions) -> Result<CreateInstanceResult> {
    utils::validate_instance_id(&opts.name)?;
    if ctx.db.get_instance(&opts.name)?.is_some() {
        return Err(AppError::conflict(format!("实例 {} 已存在", opts.name)));
    }
    let dir = ctx.paths.instances.join(&opts.name);
    if dir.exists() {
        return Err(AppError::conflict(format!(
            "实例目录已存在（可能是残留数据）: {}",
            dir.display()
        )));
    }
    version::ensure_installed(ctx, opts.kind, &opts.version)?;
    let port = opts.port.unwrap_or_else(|| opts.kind.default_port());
    utils::ensure_port_available(port)?;

    let mut config = InstanceConfig::for_kind(opts.kind, port);
    let initial_password = if opts.kind == ServiceKind::Mysql {
        match opts.root_password {
            Some(p) => {
                utils::validate_password(&p)?;
                Some(p)
            }
            None => Some(utils::random_password(16)),
        }
    } else {
        None
    };

    make_dirs(opts.kind, &dir)?;
    let now = utils::now_rfc3339();
    let rec = InstanceRecord {
        id: opts.name.clone(),
        kind: opts.kind,
        name: opts.name.clone(),
        version: opts.version.clone(),
        port,
        dir,
        status: InstanceStatus::Stopped,
        config: config.clone(),
        created_at: now.clone(),
        updated_at: now,
    };
    ctx.db.insert_instance(&rec)?;

    let init_result: Result<()> = async {
        match rec.kind {
            ServiceKind::Nginx => {
                nginx::write_main_config(ctx, &rec)?;
                let www = rec.dir.join("www");
                fs::create_dir_all(&www)?;
                let index = www.join("index.html");
                if !index.exists() {
                    fs::write(
                        &index,
                        "<!doctype html><meta charset=\"utf-8\"><title>svcbox</title>\
                         <h1>svcbox is running</h1>",
                    )?;
                }
                let site = crate::models::SiteConfig {
                    server_name: "_".to_string(),
                    root: utils::forward(&www),
                    https: false,
                    cert_domain: None,
                    ..Default::default()
                };
                nginx::create_site(ctx, &rec.id, site).await?;
                Ok(())
            }
            ServiceKind::Redis => {
                redis::write_config_file(ctx, &rec)?;
                Ok(())
            }
            ServiceKind::Mysql => {
                let pwd = initial_password
                    .clone()
                    .ok_or_else(|| AppError::other("缺少初始 root 密码"))?;
                mysql::initialize_instance(ctx, &rec, &pwd).await?;
                config.root_password_enc = Some(ctx.cipher.encrypt(&pwd)?);
                config.mysql_initialized = true;
                let mut updated = rec.clone();
                updated.config = config;
                updated.updated_at = utils::now_rfc3339();
                ctx.db.update_instance(&updated)?;
                Ok(())
            }
        }
    }
    .await;

    if let Err(e) = init_result {
        let _ = ctx.processes.kill_tree(&rec.id).await;
        let _ = ctx.db.delete_instance(&rec.id);
        let _ = fs::remove_dir_all(&rec.dir);
        return Err(e);
    }

    let mut created = ctx.require_instance(&rec.id)?;
    created.status = InstanceStatus::Stopped;
    let report = health::check(&created, &ctx.processes, false).await;
    utils::audit(
        &created.dir,
        "instance.create",
        json!({"kind": created.kind.as_str(), "version": created.version, "port": created.port}),
        &Ok(()),
    );
    ctx.emit(
        "instance-changed",
        json!({"id": created.id, "action": "created"}),
    );
    Ok(CreateInstanceResult {
        instance: view(&created, &report),
        initial_root_password: initial_password,
    })
}

fn spawn_options(ctx: &AppContext, rec: &InstanceRecord) -> Result<SpawnOptions> {
    let (program, args) = match rec.kind {
        ServiceKind::Nginx => {
            let exe = nginx_exe(ctx, rec)?;
            let conf = main_config_path(rec);
            (
                exe,
                vec![
                    "-p".to_string(),
                    format!("{}/", utils::forward(&rec.dir)),
                    "-c".to_string(),
                    utils::forward(&conf),
                ],
            )
        }
        ServiceKind::Mysql => {
            let exe = mysqld_exe(ctx, rec)?;
            (
                exe,
                vec![format!(
                    "--defaults-file={}",
                    utils::forward(&main_config_path(rec))
                )],
            )
        }
        ServiceKind::Redis => {
            let exe = redis_server_exe(ctx, rec)?;
            // Redis 8.x 的 Windows 构建基于 MSYS2，绝对盘符参数会被当成相对路径；
            // 用实例目录下的相对配置路径（cwd == rec.dir）兼容新旧构建。
            let conf = main_config_path(rec);
            let rel = conf.strip_prefix(&rec.dir).unwrap_or(&conf);
            (exe, vec![utils::forward(rel)])
        }
    };
    Ok(SpawnOptions {
        program,
        args,
        cwd: rec.dir.clone(),
        env: vec![],
        log_path: console_log_path(rec),
    })
}

fn clear_run_dir(rec: &InstanceRecord) {
    let run = rec.dir.join("run");
    let keep = [
        "supervisor.pid",
        "supervisor.status",
        "stop.request",
        "service.pid",
    ];
    if let Ok(entries) = fs::read_dir(run) {
        for entry in entries.flatten() {
            if !entry.path().is_file() {
                continue;
            }
            if keep.contains(&entry.file_name().to_string_lossy().as_ref()) {
                continue;
            }
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn log_excerpt(rec: &InstanceRecord) -> String {
    let mut text = utils::read_log_tail(&health::log_path(rec), 8 * 1024);
    if text.trim().is_empty() {
        text = utils::read_log_tail(&console_log_path(rec), 8 * 1024);
    }
    utils::tail_lines(&text, 20)
}

pub async fn start(ctx: &AppContext, id: &str) -> Result<InstanceView> {
    let mut rec = ctx.require_instance(id)?;
    if ctx.processes.is_alive(id) {
        return Err(AppError::conflict(format!("实例 {id} 已在运行")));
    }
    crate::core::supervisor::clear_stale_pid(&rec);
    if crate::core::supervisor::supervisor_alive(&rec) {
        return Err(AppError::conflict(format!(
            "实例 {id} 由 CLI 守护进程运行，请在 CLI 中操作"
        )));
    }
    utils::ensure_port_available(rec.port)?;
    if rec.kind == ServiceKind::Mysql {
        if let Err(e) = mysql::repair_legacy_host_resolution(&rec) {
            tracing::warn!("修复 MySQL 本地连接配置失败: {e}");
        }
    }

    let opts = spawn_options(ctx, &rec)?;
    clear_run_dir(&rec);
    let pid = ctx.processes.spawn(id, opts)?;
    let _ = crate::core::supervisor::write_service_pid(&rec, pid);
    if rec.kind == ServiceKind::Redis {
        redis::write_active(&rec);
    }
    ctx.db.update_instance_status(id, InstanceStatus::Running)?;
    rec.status = InstanceStatus::Running;
    rec.updated_at = utils::now_rfc3339();

    let start_result: Result<HealthReport> = async {
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            if let crate::core::process::ProcessState::Exited { code } = ctx.processes.state(id) {
                return Err(AppError::other(format!(
                    "{} 启动后立即退出（exit={}）。日志:\n{}",
                    rec.kind,
                    code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                    log_excerpt(&rec)
                )));
            }
            if utils::tcp_open(rec.port).await {
                break;
            }
            if Instant::now() >= deadline {
                tracing::warn!("实例 {id} 端口 {} 在超时内未监听", rec.port);
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        Ok(health::check(&rec, &ctx.processes, true).await)
    }
    .await;

    match start_result {
        Ok(report) => {
            if report.status == HealthStatus::Crashed {
                ctx.db.update_instance_status(id, InstanceStatus::Crashed)?;
                return Err(AppError::other(format!(
                    "{} 启动失败。日志:\n{}",
                    rec.kind,
                    log_excerpt(&rec)
                )));
            }
            utils::audit(&rec.dir, "instance.start", json!({"id": id}), &Ok(()));
            ctx.notify(
                &format!("{} 已启动", rec.name),
                &format!("{} {} (端口 {})", rec.kind, rec.version, rec.port),
            );
            ctx.emit("instance-changed", json!({"id": id, "status": "running"}));
            Ok(view(&rec, &report))
        }
        Err(e) => {
            ctx.processes.kill_tree(id).await;
            ctx.db.update_instance_status(id, InstanceStatus::Crashed)?;
            ctx.notify(&format!("{} 启动失败", rec.name), &e.to_string());
            utils::audit(
                &rec.dir,
                "instance.start",
                json!({"id": id}),
                &Err(e.to_string()),
            );
            ctx.emit("instance-changed", json!({"id": id, "status": "crashed"}));
            Err(e)
        }
    }
}

async fn stop_inner(ctx: &AppContext, rec: &InstanceRecord) -> Result<()> {
    if !ctx.processes.is_alive(&rec.id) {
        if crate::core::supervisor::supervisor_alive(rec) {
            crate::core::supervisor::request_stop(rec)?;
            if !crate::core::supervisor::wait_stopped(rec, Duration::from_secs(25)).await {
                return Err(AppError::conflict("等待 CLI 守护进程停止超时，请稍后重试"));
            }
            ctx.db
                .update_instance_status(&rec.id, InstanceStatus::Stopped)?;
            return Ok(());
        }
        if rec.status == InstanceStatus::Running && service_pid_matches(rec) {
            return Err(AppError::conflict(format!(
                "实例 {} 由其他 svcbox 进程托管，请在该进程或 HTTP API 中停止",
                rec.id
            )));
        }
        ctx.db
            .update_instance_status(&rec.id, InstanceStatus::Stopped)?;
        let _ = fs::remove_file(crate::core::supervisor::service_pid_file(rec));
        return Ok(());
    }
    match rec.kind {
        ServiceKind::Nginx => {
            if let Ok(exe) = nginx_exe(ctx, rec) {
                let args = vec![
                    "-p".to_string(),
                    format!("{}/", utils::forward(&rec.dir)),
                    "-c".to_string(),
                    utils::forward(&main_config_path(rec)),
                    "-s".to_string(),
                    "quit".to_string(),
                ];
                if let Err(e) = utils::run_tool(&exe, &args, None, &[]) {
                    tracing::warn!("nginx 优雅停机命令失败: {e}");
                }
            }
            if !ctx
                .processes
                .wait_exit(&rec.id, Duration::from_secs(3))
                .await
            {
                ctx.processes.force_kill(&rec.id);
            }
        }
        ServiceKind::Mysql => {
            if let Ok(session) = mysql_account::MySqlSession::from_instance(ctx, rec) {
                if let Err(e) = session.shutdown() {
                    tracing::warn!("mysqladmin shutdown 失败: {e}");
                }
            }
            if !ctx
                .processes
                .wait_exit(&rec.id, Duration::from_secs(8))
                .await
            {
                ctx.processes.force_kill(&rec.id);
            }
        }
        ServiceKind::Redis => {
            // 使用进程启动时记录的实际端口/密码，运行中改配置也能正确停机
            if let Err(e) = redis::shutdown(ctx, rec).await {
                tracing::warn!("Redis SHUTDOWN 失败，将等待超时后强制结束: {e}");
            }
            // SAVE 可能较慢，给足时间，避免把 dump.rdb 打断
            if !ctx
                .processes
                .wait_exit(&rec.id, Duration::from_secs(15))
                .await
            {
                ctx.processes.force_kill(&rec.id);
            }
        }
    }
    ctx.processes.remove(&rec.id);
    let _ = fs::remove_file(crate::core::supervisor::service_pid_file(rec));
    if rec.kind == ServiceKind::Redis {
        redis::clear_active(rec);
    }
    ctx.db
        .update_instance_status(&rec.id, InstanceStatus::Stopped)?;
    Ok(())
}

pub async fn stop(ctx: &AppContext, id: &str) -> Result<InstanceView> {
    let rec = ctx.require_instance(id)?;
    stop_inner(ctx, &rec).await?;
    utils::audit(&rec.dir, "instance.stop", json!({"id": id}), &Ok(()));
    ctx.emit("instance-changed", json!({"id": id, "status": "stopped"}));
    let mut updated = ctx.require_instance(id)?;
    updated.updated_at = utils::now_rfc3339();
    let report = health::check(&updated, &ctx.processes, false).await;
    Ok(view(&updated, &report))
}

pub async fn restart(ctx: &AppContext, id: &str) -> Result<InstanceView> {
    stop(ctx, id).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    start(ctx, id).await
}

pub async fn remove(ctx: &AppContext, id: &str, delete_data: bool) -> Result<()> {
    let rec = ctx.require_instance(id)?;
    // 必须先确认已停止；由其他进程托管且无法停止时不能删库删目录
    stop_inner(ctx, &rec).await?;
    if delete_data && rec.dir.exists() {
        fs::remove_dir_all(&rec.dir)?;
    }
    ctx.db.delete_instance(id)?;
    utils::audit(
        &ctx.paths.home,
        "instance.remove",
        json!({"id": id, "deleteData": delete_data}),
        &Ok(()),
    );
    ctx.emit("instance-changed", json!({"id": id, "action": "removed"}));
    Ok(())
}

pub fn service_pid_matches(rec: &InstanceRecord) -> bool {
    let Some(pid) = crate::core::supervisor::service_pid(rec) else {
        return false;
    };
    if !utils::pid_alive(pid) {
        return false;
    }
    let expected = match rec.kind {
        ServiceKind::Nginx => exe_name("nginx"),
        ServiceKind::Mysql => exe_name("mysqld"),
        ServiceKind::Redis => exe_name("redis-server"),
    };
    utils::pid_image_name(pid)
        .map(|name| name.eq_ignore_ascii_case(&expected))
        .unwrap_or(false)
}

/// 跨进程判断：当前进程表、CLI 守护进程、或“服务进程 pid 与映像名匹配且端口可连”。
pub fn is_running_sync(rec: &InstanceRecord) -> bool {
    crate::core::supervisor::supervisor_alive(rec)
        || (service_pid_matches(rec) && utils::port_open_sync(rec.port, Duration::from_millis(300)))
}

pub fn is_running(ctx: &AppContext, rec: &InstanceRecord) -> bool {
    ctx.processes.is_alive(&rec.id) || is_running_sync(rec)
}

/// 全部启动 / 全部停止用于 Dashboard 和托盘，返回失败的实例与原因。
pub async fn start_all(ctx: &AppContext) -> Vec<(String, String)> {
    let mut errors = Vec::new();
    let records = ctx.db.list_instances().unwrap_or_default();
    for rec in records {
        if ctx.processes.is_alive(&rec.id)
            || !utils::port_available(rec.port).unwrap_or(true)
            || crate::core::supervisor::supervisor_alive(&rec)
        {
            continue;
        }
        if let Err(e) = start(ctx, &rec.id).await {
            errors.push((rec.id, e.to_string()));
        }
    }
    errors
}

pub async fn stop_all(ctx: &AppContext) -> Vec<(String, String)> {
    let mut errors = Vec::new();
    let records = ctx.db.list_instances().unwrap_or_default();
    for rec in records {
        if let Err(e) = stop(ctx, &rec.id).await {
            errors.push((rec.id, e.to_string()));
        }
    }
    errors
}

/// CLI 只读列表：不写回状态，避免与正在运行的 GUI 进程互相覆盖。
pub async fn list_readonly(ctx: &AppContext) -> Result<Vec<InstanceView>> {
    let records = ctx.db.list_instances()?;
    let mut out = Vec::with_capacity(records.len());
    for rec in records {
        let expect_running = matches!(
            rec.status,
            InstanceStatus::Running | InstanceStatus::Crashed
        );
        let report = health::check(&rec, &ctx.processes, expect_running).await;
        out.push(view(&rec, &report));
    }
    Ok(out)
}

pub async fn list(ctx: &AppContext) -> Result<Vec<InstanceView>> {
    let records = ctx.db.list_instances()?;
    let mut out = Vec::with_capacity(records.len());
    for mut rec in records {
        let expect_running = matches!(
            rec.status,
            InstanceStatus::Running | InstanceStatus::Crashed
        );
        let report = health::check(&rec, &ctx.processes, expect_running).await;
        match report.status {
            HealthStatus::Crashed if rec.status != InstanceStatus::Crashed => {
                ctx.db
                    .update_instance_status(&rec.id, InstanceStatus::Crashed)?;
                rec.status = InstanceStatus::Crashed;
            }
            HealthStatus::Running
            | HealthStatus::WarningPortNotListen
            | HealthStatus::ErrorLogHasError
                if rec.status != InstanceStatus::Running =>
            {
                ctx.db
                    .update_instance_status(&rec.id, InstanceStatus::Running)?;
                rec.status = InstanceStatus::Running;
            }
            _ => {}
        }
        out.push(view(&rec, &report));
    }
    Ok(out)
}

pub async fn get(ctx: &AppContext, id: &str) -> Result<InstanceView> {
    let rec = ctx.require_instance(id)?;
    let expect_running = matches!(
        rec.status,
        InstanceStatus::Running | InstanceStatus::Crashed
    );
    let report = health::check(&rec, &ctx.processes, expect_running).await;
    Ok(view(&rec, &report))
}

pub fn log_file_path(ctx: &AppContext, id: &str) -> Result<PathBuf> {
    let rec = ctx.require_instance(id)?;
    let primary = health::log_path(&rec);
    Ok(if primary.exists() {
        primary
    } else {
        console_log_path(&rec)
    })
}

pub fn read_logs(ctx: &AppContext, id: &str, lines: usize) -> Result<LogChunk> {
    ctx.require_instance(id)?;
    let path = log_file_path(ctx, id)?;
    let text = utils::read_log_tail(&path, 256 * 1024);
    Ok(LogChunk {
        path: path.to_string_lossy().to_string(),
        content: utils::tail_lines(&text, lines.max(1)),
    })
}

pub fn read_main_config(ctx: &AppContext, id: &str) -> Result<String> {
    let rec = ctx.require_instance(id)?;
    Ok(fs::read_to_string(main_config_path(&rec))?)
}

pub async fn save_raw_config(ctx: &AppContext, id: &str, content: &str) -> Result<()> {
    let rec = ctx.require_instance(id)?;
    if content.trim().is_empty() {
        return Err(AppError::validation("配置内容不能为空"));
    }
    let path = main_config_path(&rec);
    let backup_dir = path.parent().unwrap_or(&rec.dir).join("backup");
    match rec.kind {
        ServiceKind::Nginx => {
            let exe = nginx_exe(ctx, &rec)?;
            let prefix = rec.dir.clone();
            crate::core::config::write_validated(&path, content, &backup_dir, |p| {
                crate::core::config::nginx_test(&exe, &prefix, p)
            })?;
            if is_running(ctx, &rec) {
                nginx::reload(ctx, id).await?;
            }
        }
        ServiceKind::Redis => {
            crate::core::config::validate_redis_config(content)?;
            crate::core::config::write_validated(&path, content, &backup_dir, |_| Ok(()))?;
        }
        ServiceKind::Mysql => {
            crate::core::config::write_validated(&path, content, &backup_dir, |_| Ok(()))?;
        }
    }
    utils::audit(
        &rec.dir,
        "config.save_raw",
        json!({"id": id, "path": path.to_string_lossy()}),
        &Ok(()),
    );
    Ok(())
}
