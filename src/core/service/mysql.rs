use serde_json::json;
use std::fs;
use std::time::{Duration, Instant};

use crate::core::health;
use crate::core::process::SpawnOptions;
use crate::core::service::mysql_account::{sql_string, MySqlSession};
use crate::core::{config, instance, version, AppContext};
use crate::error::{AppError, Result};
use crate::models::{InstanceRecord, MysqlUser, ServiceKind};
use crate::utils;

fn render_my_ini(ctx: &AppContext, rec: &InstanceRecord) -> Result<String> {
    if rec.kind != ServiceKind::Mysql {
        return Err(AppError::validation("该实例不是 MySQL"));
    }
    let basedir = version::ensure_installed(ctx, rec.kind, &rec.version)?;
    let socket_line = if cfg!(windows) {
        String::new()
    } else {
        format!(
            "socket={}",
            utils::forward(&rec.dir.join("run").join("mysql.sock"))
        )
    };
    let tmpl = config::template_text(&ctx.paths, "my.ini.hbs", config::MY_INI_TMPL);
    let data = json!({
        "basedir": utils::forward(&basedir),
        "datadir": utils::forward(&rec.dir.join("data")),
        "port": rec.port,
        "shared_memory_name": instance::shared_memory_name(&rec.id),
        "log_error": utils::forward(&rec.dir.join("logs").join("mysql-error.log")),
        "pid_file": utils::forward(&rec.dir.join("run").join("mysql.pid")),
        "tmp_dir": utils::forward(&rec.dir.join("tmp")),
        "socket_line": socket_line,
    });
    config::render(&tmpl, &data)
}

pub fn write_my_ini(ctx: &AppContext, rec: &InstanceRecord) -> Result<()> {
    let rendered = render_my_ini(ctx, rec)?;
    let path = instance::main_config_path(rec);
    config::write_validated(
        &path,
        &rendered,
        &rec.dir.join("conf").join("backup"),
        |_| Ok(()),
    )
}

/// 旧模板写入了 `skip-name-resolve`，导致经 TCP 连接 127.0.0.1 时无法匹配 localhost 账号（MySQL 1130）。
/// 这里只移除这一行，保留用户的其他修改；下次启动实例时生效。
pub fn repair_legacy_host_resolution(rec: &InstanceRecord) -> Result<bool> {
    if rec.kind != ServiceKind::Mysql {
        return Ok(false);
    }
    let path = instance::main_config_path(rec);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(false);
    };
    if !text.lines().any(|line| line.trim() == "skip-name-resolve") {
        return Ok(false);
    }
    let cleaned = text
        .lines()
        .filter(|line| line.trim() != "skip-name-resolve")
        .map(|line| format!("{line}\n"))
        .collect::<String>();
    config::backup(&path, &rec.dir.join("conf").join("backup"))?;
    fs::write(&path, cleaned)?;
    tracing::info!("已移除旧 my.ini 中的 skip-name-resolve（允许 127.0.0.1 本地 TCP 连接）");
    utils::audit(
        &rec.dir,
        "mysql.config.repair_host_resolution",
        json!({"id": rec.id}),
        &Ok(()),
    );
    Ok(true)
}

/// 初始化数据目录 → 临时启动 → 设置 root 密码 → 停止。
pub async fn initialize_instance(
    ctx: &AppContext,
    rec: &InstanceRecord,
    password: &str,
) -> Result<()> {
    utils::validate_password(password)?;
    write_my_ini(ctx, rec)?;
    let basedir = version::ensure_installed(ctx, rec.kind, &rec.version)?;
    let mysqld = instance::mysqld_exe(ctx, rec)?;
    let my_ini = utils::forward(&instance::main_config_path(rec));

    let args = vec![
        format!("--defaults-file={my_ini}"),
        "--initialize-insecure".to_string(),
    ];
    let out = utils::run_tool(&mysqld, &args, Some(&basedir), &[])?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(AppError::other(format!(
            "MySQL 数据目录初始化失败: {stderr}\n{}",
            utils::tail_lines(&utils::read_log_tail(&health::log_path(rec), 16 * 1024), 30)
        )));
    }

    let spawn = SpawnOptions {
        program: mysqld.clone(),
        args: vec![format!("--defaults-file={my_ini}")],
        cwd: basedir,
        env: vec![],
        log_path: instance::console_log_path(rec),
    };
    ctx.processes.spawn(&rec.id, spawn)?;

    let session = MySqlSession::without_password(ctx, rec)?;
    if let Err(e) = wait_ready(ctx, rec, &session, Duration::from_secs(90)).await {
        ctx.processes.kill_tree(&rec.id).await;
        return Err(e);
    }

    let set_pwd = format!(
        "ALTER USER 'root'@'localhost' IDENTIFIED BY '{}';",
        sql_string(password)
    );
    if let Err(e) = session.execute(&set_pwd) {
        ctx.processes.kill_tree(&rec.id).await;
        return Err(AppError::other(format!("设置 root 密码失败: {e}")));
    }

    if let Ok(with_pwd) = MySqlSession::with_password(ctx, rec, password) {
        let _ = with_pwd.shutdown();
    }
    if !ctx
        .processes
        .wait_exit(&rec.id, Duration::from_secs(15))
        .await
    {
        ctx.processes.force_kill(&rec.id);
        let _ = ctx
            .processes
            .wait_exit(&rec.id, Duration::from_secs(5))
            .await;
    }
    ctx.processes.remove(&rec.id);
    Ok(())
}

async fn wait_ready(
    ctx: &AppContext,
    rec: &InstanceRecord,
    session: &MySqlSession,
    timeout: Duration,
) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if let crate::core::process::ProcessState::Exited { code } = ctx.processes.state(&rec.id) {
            return Err(AppError::other(format!(
                "mysqld 启动后退出（exit={}）。日志:\n{}",
                code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                utils::tail_lines(&utils::read_log_tail(&health::log_path(rec), 16 * 1024), 30)
            )));
        }
        if session.try_ping() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(AppError::other(format!(
                "等待 MySQL 就绪超时。日志:\n{}",
                utils::tail_lines(&utils::read_log_tail(&health::log_path(rec), 16 * 1024), 30)
            )));
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn restore_running(ctx: &AppContext, rec: &InstanceRecord, was_in_process: bool) {
    if was_in_process {
        if let Err(e) = crate::core::instance::start(ctx, &rec.id).await {
            tracing::warn!("重置失败后恢复 MySQL 运行失败: {e}");
        }
    } else {
        crate::core::supervisor::clear_runtime_files(rec);
        if crate::core::supervisor::spawn_detached(ctx, rec).is_ok() {
            if let Err(e) = crate::core::supervisor::wait_ready(rec, Duration::from_secs(90)).await
            {
                tracing::warn!("重置失败后恢复 MySQL 守护进程失败: {e}");
            }
        }
    }
}

fn running_session<'a>(
    ctx: &'a AppContext,
    instance_id: &str,
) -> Result<(InstanceRecord, MySqlSession)> {
    let rec = ctx.require_instance(instance_id)?;
    if rec.kind != ServiceKind::Mysql {
        return Err(AppError::validation("该实例不是 MySQL"));
    }
    if !instance::is_running(ctx, &rec) {
        return Err(AppError::conflict("MySQL 实例未运行，请先启动"));
    }
    let session = MySqlSession::from_instance(ctx, &rec)?;
    Ok((rec, session))
}

pub fn list_databases(ctx: &AppContext, instance_id: &str) -> Result<Vec<String>> {
    let (_, session) = running_session(ctx, instance_id)?;
    session.databases()
}

pub fn create_database(
    ctx: &AppContext,
    instance_id: &str,
    name: &str,
    charset: Option<String>,
    collate: Option<String>,
) -> Result<()> {
    let (rec, session) = running_session(ctx, instance_id)?;
    let charset = charset.unwrap_or_else(|| "utf8mb4".to_string());
    let collate = collate.unwrap_or_else(|| "utf8mb4_unicode_ci".to_string());
    let result = session.create_database(name, &charset, &collate);
    utils::audit(
        &rec.dir,
        "mysql.db.create",
        json!({"db": name, "charset": charset, "collate": collate}),
        &result.as_ref().map(|_| ()).map_err(|e| e.to_string()),
    );
    result?;
    ctx.emit(
        "mysql-changed",
        json!({"instanceId": instance_id, "action": "db-created", "db": name}),
    );
    Ok(())
}

pub fn drop_database(ctx: &AppContext, instance_id: &str, name: &str) -> Result<()> {
    let (rec, session) = running_session(ctx, instance_id)?;
    let result = session.drop_database(name);
    utils::audit(
        &rec.dir,
        "mysql.db.drop",
        json!({"db": name}),
        &result.as_ref().map(|_| ()).map_err(|e| e.to_string()),
    );
    result?;
    ctx.emit(
        "mysql-changed",
        json!({"instanceId": instance_id, "action": "db-dropped", "db": name}),
    );
    Ok(())
}

pub fn list_users(ctx: &AppContext, instance_id: &str) -> Result<Vec<MysqlUser>> {
    let (_, session) = running_session(ctx, instance_id)?;
    session.users()
}

pub fn create_user(
    ctx: &AppContext,
    instance_id: &str,
    user: &str,
    host: &str,
    password: &str,
    dbs: Vec<String>,
) -> Result<()> {
    let (rec, session) = running_session(ctx, instance_id)?;
    let result = session.create_user(user, host, password, &dbs);
    utils::audit(
        &rec.dir,
        "mysql.user.create",
        json!({"user": user, "host": host, "password": password, "dbs": dbs}),
        &result.as_ref().map(|_| ()).map_err(|e| e.to_string()),
    );
    result?;
    ctx.emit(
        "mysql-changed",
        json!({"instanceId": instance_id, "action": "user-created", "user": user}),
    );
    Ok(())
}

pub fn alter_user_password(
    ctx: &AppContext,
    instance_id: &str,
    user: &str,
    host: &str,
    password: &str,
) -> Result<()> {
    let (rec, session) = running_session(ctx, instance_id)?;
    let result = session.alter_user_password(user, host, password);
    utils::audit(
        &rec.dir,
        "mysql.user.password",
        json!({"user": user, "host": host, "password": password}),
        &result.as_ref().map(|_| ()).map_err(|e| e.to_string()),
    );
    result?;
    ctx.emit(
        "mysql-changed",
        json!({"instanceId": instance_id, "action": "user-password", "user": user}),
    );
    Ok(())
}

pub fn drop_user(ctx: &AppContext, instance_id: &str, user: &str, host: &str) -> Result<()> {
    let (rec, session) = running_session(ctx, instance_id)?;
    let result = session.drop_user(user, host);
    utils::audit(
        &rec.dir,
        "mysql.user.drop",
        json!({"user": user, "host": host}),
        &result.as_ref().map(|_| ()).map_err(|e| e.to_string()),
    );
    result?;
    ctx.emit(
        "mysql-changed",
        json!({"instanceId": instance_id, "action": "user-dropped", "user": user}),
    );
    Ok(())
}

/// 应急重置 root 密码：--skip-grant-tables 起服，同一会话内 FLUSH + ALTER。
pub async fn reset_root_password(
    ctx: &AppContext,
    instance_id: &str,
    new_password: &str,
) -> Result<()> {
    utils::validate_password(new_password)?;
    let rec = ctx.require_instance(instance_id)?;
    if rec.kind != ServiceKind::Mysql {
        return Err(AppError::validation("该实例不是 MySQL"));
    }
    if rec.config.root_password_enc.is_none() {
        return Err(AppError::conflict("实例尚未初始化 root 密码"));
    }
    let was_in_process = ctx.processes.is_alive(instance_id);
    let was_external = !was_in_process && instance::is_running(ctx, &rec);
    let basedir = version::ensure_installed(ctx, rec.kind, &rec.version)?;
    let mysqld = instance::mysqld_exe(ctx, &rec)?;
    let my_ini = utils::forward(&instance::main_config_path(&rec));

    let was_running = was_in_process || was_external;
    if was_running {
        crate::core::instance::stop(ctx, instance_id).await?;
    }

    let spawn = SpawnOptions {
        program: mysqld,
        args: vec![
            format!("--defaults-file={my_ini}"),
            "--skip-grant-tables".to_string(),
            "--skip-networking".to_string(),
        ],
        cwd: basedir,
        env: vec![],
        log_path: instance::console_log_path(&rec),
    };
    if let Err(e) = ctx.processes.spawn(instance_id, spawn) {
        if was_running {
            restore_running(ctx, &rec, was_in_process).await;
        }
        return Err(e);
    }

    let session = match MySqlSession::without_password(ctx, &rec) {
        Ok(session) => session,
        Err(e) => {
            ctx.processes.kill_tree(instance_id).await;
            if was_running {
                restore_running(ctx, &rec, was_in_process).await;
            }
            return Err(e);
        }
    };
    let reset_result: Result<()> = async {
        wait_ready(ctx, &rec, &session, Duration::from_secs(60)).await?;
        let sql = format!(
            "FLUSH PRIVILEGES; ALTER USER 'root'@'localhost' IDENTIFIED BY '{}';",
            sql_string(new_password)
        );
        session.execute(&sql)?;
        Ok(())
    }
    .await;

    if let Err(e) = reset_result {
        ctx.processes.kill_tree(instance_id).await;
        if was_running {
            restore_running(ctx, &rec, was_in_process).await;
        }
        return Err(e);
    }

    if let Ok(with_pwd) = MySqlSession::with_password(ctx, &rec, new_password) {
        let _ = with_pwd.shutdown();
    }
    if !ctx
        .processes
        .wait_exit(instance_id, Duration::from_secs(15))
        .await
    {
        ctx.processes.force_kill(instance_id);
        let _ = ctx
            .processes
            .wait_exit(instance_id, Duration::from_secs(5))
            .await;
    }
    ctx.processes.remove(instance_id);

    let mut updated = rec.clone();
    updated.config.root_password_enc = Some(ctx.cipher.encrypt(new_password)?);
    updated.updated_at = utils::now_rfc3339();
    ctx.db.update_instance(&updated)?;

    if was_running {
        if was_in_process {
            let _ = crate::core::instance::start(ctx, instance_id).await;
        } else {
            // 原本由 CLI 守护进程托管，重置后交还给守护进程
            crate::core::supervisor::clear_runtime_files(&updated);
            let _ = crate::core::supervisor::spawn_detached(ctx, &updated);
            let _ = crate::core::supervisor::wait_ready(&updated, Duration::from_secs(90)).await;
        }
    }
    utils::audit(
        &rec.dir,
        "mysql.root.reset",
        json!({"id": instance_id, "password": new_password}),
        &Ok(()),
    );
    ctx.notify(
        &format!("{} root 密码已重置", rec.name),
        "请使用新密码重新连接 MySQL",
    );
    Ok(())
}
