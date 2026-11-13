use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use crate::core::{config, instance, AppContext};
use crate::error::{AppError, Result};
use crate::models::{InstanceRecord, RedisConfigInput, RedisConfigView, ServiceKind};
use crate::utils;

fn render_config(ctx: &AppContext, rec: &InstanceRecord) -> Result<String> {
    if rec.kind != ServiceKind::Redis {
        return Err(AppError::validation("该实例不是 Redis"));
    }
    let cfg = &rec.config.redis;
    let password = match &cfg.requirepass_enc {
        Some(enc) => Some(ctx.cipher.decrypt(enc)?),
        None => None,
    };
    let tmpl = config::template_text(&ctx.paths, "redis.conf.hbs", config::REDIS_CONF_TMPL);
    let data = json!({
        "bind": cfg.bind,
        "port": cfg.port,
        "pid_file": utils::forward(&rec.dir.join("run").join("redis.pid")),
        "log_file": utils::forward(&rec.dir.join("logs").join("redis.log")),
        "data_dir": utils::forward(&rec.dir.join("data")),
        "appendonly": cfg.appendonly,
        "has_password": password.is_some(),
        "requirepass": password.unwrap_or_default(),
    });
    let rendered = config::render(&tmpl, &data)?;
    config::validate_redis_config(&rendered)?;
    Ok(rendered)
}

pub fn write_config_file(ctx: &AppContext, rec: &InstanceRecord) -> Result<()> {
    let rendered = render_config(ctx, rec)?;
    fs::create_dir_all(rec.dir.join("conf").join("backup"))?;
    fs::create_dir_all(rec.dir.join("data"))?;
    fs::create_dir_all(rec.dir.join("logs"))?;
    fs::create_dir_all(rec.dir.join("run"))?;
    config::write_validated(
        &instance::main_config_path(rec),
        &rendered,
        &rec.dir.join("conf").join("backup"),
        |p| {
            let text = fs::read_to_string(p)?;
            config::validate_redis_config(&text)
        },
    )
}

pub fn config_view(ctx: &AppContext, instance_id: &str) -> Result<RedisConfigView> {
    let rec = ctx.require_instance(instance_id)?;
    if rec.kind != ServiceKind::Redis {
        return Err(AppError::validation("该实例不是 Redis"));
    }
    Ok(RedisConfigView {
        bind: rec.config.redis.bind.clone(),
        port: rec.port,
        has_password: rec.config.redis.requirepass_enc.is_some(),
        appendonly: rec.config.redis.appendonly,
        requires_restart: false,
    })
}

pub async fn update_config(
    ctx: &AppContext,
    instance_id: &str,
    input: RedisConfigInput,
) -> Result<RedisConfigView> {
    let rec = ctx.require_instance(instance_id)?;
    if rec.kind != ServiceKind::Redis {
        return Err(AppError::validation("该实例不是 Redis"));
    }
    utils::validate_bind(&input.bind)?;
    utils::validate_port(input.port)?;
    if let Some(pwd) = &input.requirepass {
        if !pwd.is_empty() {
            utils::validate_redis_password(pwd)?;
        }
    }
    // 先用旧配置判断运行状态：改端口后新端口当然还没监听
    let running = instance::is_running(ctx, &rec);
    if input.port != rec.port {
        utils::ensure_port_available(input.port)?;
    }

    let password_enc = match &input.requirepass {
        None => rec.config.redis.requirepass_enc.clone(),
        Some(p) if p.is_empty() => None,
        Some(p) => Some(ctx.cipher.encrypt(p)?),
    };

    let mut updated = rec.clone();
    updated.port = input.port;
    updated.config.redis.bind = input.bind;
    updated.config.redis.port = input.port;
    updated.config.redis.requirepass_enc = password_enc;
    updated.config.redis.appendonly = input.appendonly;
    updated.updated_at = utils::now_rfc3339();

    write_config_file(ctx, &updated)?;
    ctx.db.update_instance(&updated)?;
    utils::audit(
        &updated.dir,
        "redis.config.update",
        json!({
            "id": instance_id,
            "bind": updated.config.redis.bind,
            "port": updated.port,
            "appendonly": updated.config.redis.appendonly,
        }),
        &Ok(()),
    );
    ctx.emit(
        "instance-changed",
        json!({"id": instance_id, "action": "config"}),
    );
    if running {
        ctx.notify(
            &format!("{} 配置已保存", updated.name),
            "Redis 的 bind/port/密码/AOF 变更需要重启实例后生效",
        );
    }
    Ok(RedisConfigView {
        bind: updated.config.redis.bind,
        port: updated.port,
        has_password: updated.config.redis.requirepass_enc.is_some(),
        appendonly: updated.config.redis.appendonly,
        requires_restart: running,
    })
}

pub fn requirepass(ctx: &AppContext, rec: &InstanceRecord) -> Result<Option<String>> {
    match &rec.config.redis.requirepass_enc {
        Some(enc) => Ok(Some(ctx.cipher.decrypt(enc)?)),
        None => Ok(None),
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ActiveRedis {
    port: u16,
    password_enc: Option<String>,
}

fn active_file(rec: &InstanceRecord) -> PathBuf {
    rec.dir.join("run").join("redis-active.json")
}

/// 记录进程实际启动时使用的端口/密码。运行中改配置后，stop 仍能连上旧进程。
pub fn write_active(rec: &InstanceRecord) {
    if rec.kind != ServiceKind::Redis {
        return;
    }
    let active = ActiveRedis {
        port: rec.port,
        password_enc: rec.config.redis.requirepass_enc.clone(),
    };
    if let Ok(data) = serde_json::to_vec(&active) {
        let _ = fs::write(active_file(rec), data);
    }
}

pub fn clear_active(rec: &InstanceRecord) {
    let _ = fs::remove_file(active_file(rec));
}

fn active_credentials(ctx: &AppContext, rec: &InstanceRecord) -> (u16, Option<String>) {
    if let Ok(data) = fs::read(active_file(rec)) {
        if let Ok(active) = serde_json::from_slice::<ActiveRedis>(&data) {
            let password = active
                .password_enc
                .as_ref()
                .and_then(|enc| ctx.cipher.decrypt(enc).ok());
            return (active.port, password);
        }
    }
    let password = rec
        .config
        .redis
        .requirepass_enc
        .as_ref()
        .and_then(|enc| ctx.cipher.decrypt(enc).ok());
    (rec.port, password)
}

/// 直接走 RESP 发 AUTH + SHUTDOWN，避免 redis-cli 把密码放进 argv，也避免版本差异。
pub async fn shutdown(ctx: &AppContext, rec: &InstanceRecord) -> Result<()> {
    let (port, password) = active_credentials(ctx, rec);
    tokio::task::spawn_blocking(move || shutdown_blocking(port, password.as_deref()))
        .await
        .map_err(|e| AppError::other(format!("Redis 停机任务失败: {e}")))?
}

fn shutdown_blocking(port: u16, password: Option<&str>) -> Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(2))?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    stream.set_write_timeout(Some(Duration::from_secs(3)))?;

    if let Some(pwd) = password {
        write_resp(&mut stream, &["AUTH", pwd])?;
        let reply = read_line(&mut stream)?;
        if reply.starts_with('-') {
            return Err(AppError::other(format!("Redis 认证失败: {}", reply.trim())));
        }
    }
    write_resp(&mut stream, &["SHUTDOWN"])?;
    // SHUTDOWN 成功时服务端通常直接关闭连接；读到 -ERR 才算失败
    if let Ok(reply) = read_line(&mut stream) {
        if reply.starts_with('-') {
            return Err(AppError::other(format!(
                "Redis SHUTDOWN 失败: {}",
                reply.trim()
            )));
        }
    }
    Ok(())
}

fn write_resp(stream: &mut TcpStream, parts: &[&str]) -> Result<()> {
    let mut buf = format!("*{}\r\n", parts.len());
    for part in parts {
        buf.push_str(&format!("${}\r\n{}\r\n", part.as_bytes().len(), part));
    }
    stream.write_all(buf.as_bytes())?;
    stream.flush()?;
    Ok(())
}

fn read_line(stream: &mut TcpStream) -> Result<String> {
    let mut out = Vec::new();
    let mut byte = [0u8; 1];
    while out.len() < 4096 {
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {
                if byte[0] == b'\n' {
                    break;
                }
                if byte[0] != b'\r' {
                    out.push(byte[0]);
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(String::from_utf8_lossy(&out).to_string())
}
