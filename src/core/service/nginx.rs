use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};

use crate::core::{config, instance, version, AppContext};
use crate::error::{AppError, Result};
use crate::models::{CertificateInfo, InstanceRecord, NginxGlobalConfig, ServiceKind, SiteConfig};
use crate::utils;

fn sites_conf_dir(rec: &InstanceRecord) -> PathBuf {
    rec.dir.join("conf").join("sites")
}

fn sites_meta_dir(rec: &InstanceRecord) -> PathBuf {
    rec.dir.join("conf").join("sites_meta")
}

fn backup_dir(rec: &InstanceRecord) -> PathBuf {
    rec.dir.join("conf").join("backup")
}

fn site_key(server_name: &str) -> Result<String> {
    utils::validate_server_name(server_name)?;
    if server_name == "_" {
        return Ok("default".to_string());
    }
    Ok(server_name.to_ascii_lowercase())
}

fn conf_path(rec: &InstanceRecord, key: &str) -> PathBuf {
    sites_conf_dir(rec).join(format!("{key}.conf"))
}

fn meta_path(rec: &InstanceRecord, key: &str) -> PathBuf {
    sites_meta_dir(rec).join(format!("{key}.json"))
}

fn save_meta(rec: &InstanceRecord, key: &str, site: &SiteConfig) -> Result<()> {
    fs::create_dir_all(sites_meta_dir(rec))?;
    let path = meta_path(rec, key);
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(site)?)?;
    fs::rename(tmp, path)?;
    Ok(())
}

pub fn list_sites(ctx: &AppContext, instance_id: &str) -> Result<Vec<SiteConfig>> {
    let rec = ctx.require_instance(instance_id)?;
    if rec.kind != ServiceKind::Nginx {
        return Err(AppError::validation("该实例不是 Nginx"));
    }
    let dir = sites_meta_dir(&rec);
    let mut out = Vec::new();
    if dir.is_dir() {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            if entry
                .path()
                .extension()
                .map(|e| e != "json")
                .unwrap_or(true)
            {
                continue;
            }
            let raw = fs::read_to_string(entry.path())?;
            match serde_json::from_str::<SiteConfig>(&raw) {
                Ok(site) => out.push(site),
                Err(e) => tracing::warn!("站点元数据 {} 解析失败: {e}", entry.path().display()),
            }
        }
    }
    out.sort_by(|a, b| a.server_name.cmp(&b.server_name));
    Ok(out)
}

fn find_site(rec: &InstanceRecord, server_name: &str) -> Result<SiteConfig> {
    let key = site_key(server_name)?;
    let path = meta_path(rec, &key);
    if !path.is_file() {
        return Err(AppError::not_found(format!("站点 {server_name} 不存在")));
    }
    let raw = fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

fn validate_site(site: &SiteConfig) -> Result<()> {
    utils::validate_server_name(&site.server_name)?;
    if site.root.trim().is_empty() {
        return Err(AppError::validation("站点 root 目录不能为空"));
    }
    if site.root.chars().any(|c| c.is_control() || c == '"') {
        return Err(AppError::validation("站点 root 不能包含引号或控制字符"));
    }
    if site.index.trim().is_empty() {
        return Err(AppError::validation("index 不能为空"));
    }
    if site
        .index
        .chars()
        .any(|c| c.is_control() || matches!(c, ';' | '{' | '}'))
    {
        return Err(AppError::validation("index 不能包含 ; { } 或控制字符"));
    }
    if site.https {
        let domain = site
            .cert_domain
            .as_deref()
            .ok_or_else(|| AppError::validation("开启 HTTPS 必须指定证书域名"))?;
        utils::validate_domain(domain)?;
    }
    Ok(())
}

fn render_site(ctx: &AppContext, rec: &InstanceRecord, site: &SiteConfig) -> Result<String> {
    let access_log = utils::forward(&rec.dir.join("logs").join("access.log"));
    let error_log = utils::forward(&rec.dir.join("logs").join("error.log"));
    let acme_root = utils::forward(&rec.dir);
    if site.https {
        let domain = site
            .cert_domain
            .as_deref()
            .ok_or_else(|| AppError::validation("缺少证书域名"))?;
        let cert_path = instance::cert_dir(rec, domain).join("fullchain.pem");
        let key_path = instance::cert_dir(rec, domain).join("key.pem");
        if !cert_path.is_file() || !key_path.is_file() {
            return Err(AppError::not_found(format!(
                "证书 {domain} 尚未申请或文件缺失，请先执行 HTTPS 申请"
            )));
        }
        let tmpl = config::template_text(
            &ctx.paths,
            "nginx_site_https.hbs",
            config::NGINX_SITE_HTTPS_TMPL,
        );
        let data = json!({
            "http_port": rec.port,
            "https_port": rec.config.nginx.https_port,
            "server_name": site.server_name,
            "root": utils::forward(Path::new(&site.root)),
            "index": site.index,
            "access_log": access_log,
            "error_log": error_log,
            "acme_root": acme_root,
            "cert_file": utils::forward(&cert_path),
            "key_file": utils::forward(&key_path),
            "custom_locations": site.custom_locations,
        });
        return config::render(&tmpl, &data);
    }

    let tmpl = config::template_text(&ctx.paths, "nginx_site.hbs", config::NGINX_SITE_TMPL);
    let data = json!({
        "port": rec.port,
        "server_name": site.server_name,
        "root": utils::forward(Path::new(&site.root)),
        "index": site.index,
        "access_log": access_log,
        "error_log": error_log,
        "acme_root": acme_root,
        "custom_locations": site.custom_locations,
    });
    config::render(&tmpl, &data)
}

pub fn write_main_config(ctx: &AppContext, rec: &InstanceRecord) -> Result<()> {
    if rec.kind != ServiceKind::Nginx {
        return Err(AppError::validation("该实例不是 Nginx"));
    }
    let version_dir = version::ensure_installed(ctx, rec.kind, &rec.version)?;
    let mime = version_dir.join("conf").join("mime.types");
    if !mime.is_file() {
        return Err(AppError::not_found(format!(
            "Nginx 缺少 mime.types: {}",
            mime.display()
        )));
    }
    let exe = instance::nginx_exe(ctx, rec)?;
    let cfg = &rec.config.nginx;
    let tmpl = config::template_text(&ctx.paths, "nginx_main.hbs", config::NGINX_MAIN_TMPL);
    let data = json!({
        "worker_processes": cfg.worker_processes,
        "worker_connections": cfg.worker_connections,
        "keepalive_timeout": cfg.keepalive_timeout,
        "gzip": cfg.gzip,
        "client_max_body_size": cfg.client_max_body_size,
        "error_log": utils::forward(&rec.dir.join("logs").join("error.log")),
        "pid_file": utils::forward(&rec.dir.join("run").join("nginx.pid")),
        "mime_types": utils::forward(&mime),
        "access_log": utils::forward(&rec.dir.join("logs").join("access.log")),
        "sites_glob": utils::forward(&rec.dir.join("conf").join("sites").join("*.conf")),
    });
    let rendered = config::render(&tmpl, &data)?;
    let path = instance::main_config_path(rec);
    let prefix = rec.dir.clone();
    config::write_validated(&path, &rendered, &backup_dir(rec), |p| {
        config::nginx_test(&exe, &prefix, p)
    })
}

async fn reload_if_running(ctx: &AppContext, rec: &InstanceRecord) -> Result<()> {
    if instance::is_running(ctx, rec) {
        reload(ctx, &rec.id).await?;
    }
    Ok(())
}

pub async fn create_site(
    ctx: &AppContext,
    instance_id: &str,
    mut site: SiteConfig,
) -> Result<SiteConfig> {
    let rec = ctx.require_instance(instance_id)?;
    if rec.kind != ServiceKind::Nginx {
        return Err(AppError::validation("该实例不是 Nginx"));
    }
    if site.index.trim().is_empty() {
        site.index = "index.html index.htm".to_string();
    }
    validate_site(&site)?;
    let key = site_key(&site.server_name)?;
    if meta_path(&rec, &key).exists() {
        return Err(AppError::conflict(format!(
            "站点 {} 已存在",
            site.server_name
        )));
    }

    let rendered = render_site(ctx, &rec, &site)?;
    let exe = instance::nginx_exe(ctx, &rec)?;
    let prefix = rec.dir.clone();
    let main_conf = instance::main_config_path(&rec);
    let path = conf_path(&rec, &key);
    let result = (|| -> Result<()> {
        // 站点文件本身不能作为 nginx 主配置校验，必须校验整份 nginx.conf
        config::write_validated(&path, &rendered, &backup_dir(&rec), |_| {
            config::nginx_test(&exe, &prefix, &main_conf)
        })?;
        save_meta(&rec, &key, &site)?;
        Ok(())
    })();

    let audit = result.as_ref().map(|_| ()).map_err(|e| e.to_string());
    utils::audit(
        &rec.dir,
        "nginx.site.create",
        json!({"serverName": site.server_name, "root": site.root}),
        &audit,
    );
    result?;

    reload_if_running(ctx, &rec).await?;
    ctx.emit(
        "nginx-sites-changed",
        json!({"instanceId": instance_id, "action": "created"}),
    );
    Ok(site)
}

pub async fn update_site(
    ctx: &AppContext,
    instance_id: &str,
    mut site: SiteConfig,
) -> Result<SiteConfig> {
    let rec = ctx.require_instance(instance_id)?;
    if rec.kind != ServiceKind::Nginx {
        return Err(AppError::validation("该实例不是 Nginx"));
    }
    if site.index.trim().is_empty() {
        site.index = "index.html index.htm".to_string();
    }
    validate_site(&site)?;
    let key = site_key(&site.server_name)?;
    if !meta_path(&rec, &key).exists() {
        return Err(AppError::not_found(format!(
            "站点 {} 不存在",
            site.server_name
        )));
    }

    let rendered = render_site(ctx, &rec, &site)?;
    let exe = instance::nginx_exe(ctx, &rec)?;
    let prefix = rec.dir.clone();
    let main_conf = instance::main_config_path(&rec);
    let path = conf_path(&rec, &key);
    let result = (|| -> Result<()> {
        // 站点文件本身不能作为 nginx 主配置校验，必须校验整份 nginx.conf
        config::write_validated(&path, &rendered, &backup_dir(&rec), |_| {
            config::nginx_test(&exe, &prefix, &main_conf)
        })?;
        save_meta(&rec, &key, &site)?;
        Ok(())
    })();
    let audit = result.as_ref().map(|_| ()).map_err(|e| e.to_string());
    utils::audit(
        &rec.dir,
        "nginx.site.update",
        json!({"serverName": site.server_name, "root": site.root, "https": site.https}),
        &audit,
    );
    result?;

    reload_if_running(ctx, &rec).await?;
    ctx.emit(
        "nginx-sites-changed",
        json!({"instanceId": instance_id, "action": "updated"}),
    );
    Ok(site)
}

/// 删除站点只删配置文件，绝不删除站点 root 目录。
pub async fn delete_site(ctx: &AppContext, instance_id: &str, server_name: &str) -> Result<()> {
    let rec = ctx.require_instance(instance_id)?;
    if rec.kind != ServiceKind::Nginx {
        return Err(AppError::validation("该实例不是 Nginx"));
    }
    let site = find_site(&rec, server_name)?;
    let key = site_key(server_name)?;
    let conf = conf_path(&rec, &key);
    let meta = meta_path(&rec, &key);
    if conf.exists() {
        fs::remove_file(&conf)?;
    }
    if meta.exists() {
        fs::remove_file(&meta)?;
    }
    utils::audit(
        &rec.dir,
        "nginx.site.delete",
        json!({"serverName": server_name, "rootKept": site.root}),
        &Ok(()),
    );
    reload_if_running(ctx, &rec).await?;
    ctx.emit(
        "nginx-sites-changed",
        json!({"instanceId": instance_id, "action": "deleted"}),
    );
    Ok(())
}

pub async fn reload(ctx: &AppContext, instance_id: &str) -> Result<()> {
    let rec = ctx.require_instance(instance_id)?;
    if rec.kind != ServiceKind::Nginx {
        return Err(AppError::validation("该实例不是 Nginx"));
    }
    if !instance::is_running(ctx, &rec) {
        return Err(AppError::conflict("Nginx 未运行，无法 reload"));
    }
    let exe = instance::nginx_exe(ctx, &rec)?;
    let args = vec![
        "-p".to_string(),
        format!("{}/", utils::forward(&rec.dir)),
        "-c".to_string(),
        utils::forward(&instance::main_config_path(&rec)),
        "-s".to_string(),
        "reload".to_string(),
    ];
    let out = utils::run_tool(&exe, &args, None, &[])?;
    utils::output_ok(&exe, out)?;
    // nginx -s reload 只是发信号，等 master 完成 reconfig 再返回，避免脚本立刻请求拿到旧配置
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    utils::audit(
        &rec.dir,
        "nginx.reload",
        json!({"id": instance_id}),
        &Ok(()),
    );
    Ok(())
}

pub fn global_config(ctx: &AppContext, instance_id: &str) -> Result<NginxGlobalConfig> {
    let rec = ctx.require_instance(instance_id)?;
    if rec.kind != ServiceKind::Nginx {
        return Err(AppError::validation("该实例不是 Nginx"));
    }
    Ok(rec.config.nginx.clone())
}

pub async fn update_global(
    ctx: &AppContext,
    instance_id: &str,
    cfg: NginxGlobalConfig,
) -> Result<()> {
    let rec = ctx.require_instance(instance_id)?;
    if rec.kind != ServiceKind::Nginx {
        return Err(AppError::validation("该实例不是 Nginx"));
    }
    if cfg.worker_processes == 0 || cfg.worker_processes > 64 {
        return Err(AppError::validation("worker_processes 必须在 1-64 之间"));
    }
    if cfg.worker_connections == 0 || cfg.worker_connections > 65535 {
        return Err(AppError::validation(
            "worker_connections 必须在 1-65535 之间",
        ));
    }
    if cfg.keepalive_timeout > 3600 {
        return Err(AppError::validation("keepalive_timeout 不能超过 3600"));
    }
    let size = cfg.client_max_body_size.trim();
    let mut chars = size.chars();
    let first_ok = chars.next().map(|c| c.is_ascii_digit()).unwrap_or(false);
    let rest_ok =
        chars.all(|c| c.is_ascii_digit() || matches!(c, 'k' | 'K' | 'm' | 'M' | 'g' | 'G'));
    if !first_ok || !rest_ok || size.is_empty() {
        return Err(AppError::validation(
            "client_max_body_size 格式应为 64m / 10M / 1024k",
        ));
    }
    if cfg.https_port == 0 {
        return Err(AppError::validation("HTTPS 端口不能为 0"));
    }
    if cfg.https_port == rec.port {
        return Err(AppError::validation("HTTPS 端口不能与 HTTP 端口相同"));
    }
    if cfg.https_port != rec.config.nginx.https_port {
        utils::ensure_port_available(cfg.https_port)?;
    }

    let mut updated = rec.clone();
    updated.config.nginx = cfg;
    updated.updated_at = utils::now_rfc3339();
    write_main_config(ctx, &updated)?;
    ctx.db.update_instance(&updated)?;
    reload_if_running(ctx, &updated).await?;
    utils::audit(
        &updated.dir,
        "nginx.global.update",
        json!({"id": instance_id}),
        &Ok(()),
    );
    ctx.emit(
        "instance-changed",
        json!({"id": instance_id, "action": "config"}),
    );
    Ok(())
}

pub async fn apply_https(
    ctx: &AppContext,
    instance_id: &str,
    server_name: &str,
    domain: &str,
) -> Result<CertificateInfo> {
    let rec = ctx.require_instance(instance_id)?;
    if rec.kind != ServiceKind::Nginx {
        return Err(AppError::validation("该实例不是 Nginx"));
    }
    utils::validate_domain(domain)?;
    if !instance::is_running(ctx, &rec) {
        return Err(AppError::conflict("申请证书前请先启动 Nginx 实例"));
    }
    if rec.port != 80 {
        return Err(AppError::validation(format!(
            "HTTP-01 验证要求实例监听 80 端口，当前为 {}，请调整端口后重试",
            rec.port
        )));
    }
    let mut site = find_site(&rec, server_name)?;
    // 首个 HTTPS 站点需要确保 https_port 没被其他程序占用；后续站点复用本实例已监听的端口
    let has_https = list_sites(ctx, instance_id)?.iter().any(|s| s.https);
    if !has_https {
        utils::ensure_port_available(rec.config.nginx.https_port)?;
    }
    let cert = crate::core::cert::issue(ctx, &rec, domain).await?;
    site.https = true;
    site.cert_domain = Some(domain.to_string());
    // 直接写文件，避免 update_site 再次校验实例运行状态
    let rendered = render_site(ctx, &rec, &site)?;
    let exe = instance::nginx_exe(ctx, &rec)?;
    let prefix = rec.dir.clone();
    let main_conf = instance::main_config_path(&rec);
    let key = site_key(&site.server_name)?;
    config::write_validated(&conf_path(&rec, &key), &rendered, &backup_dir(&rec), |_| {
        config::nginx_test(&exe, &prefix, &main_conf)
    })?;
    save_meta(&rec, &key, &site)?;
    reload(ctx, instance_id).await?;
    if !utils::tcp_open(rec.config.nginx.https_port).await {
        return Err(AppError::other(format!(
            "HTTPS 端口 {} 未监听，可能是端口被占用或 Nginx reload 失败，请查看实例日志",
            rec.config.nginx.https_port
        )));
    }
    utils::audit(
        &rec.dir,
        "nginx.site.https",
        json!({"serverName": server_name, "domain": domain}),
        &Ok(()),
    );
    ctx.emit(
        "nginx-sites-changed",
        json!({"instanceId": instance_id, "action": "https"}),
    );
    Ok(cert)
}

pub async fn renew_cert(
    ctx: &AppContext,
    instance_id: &str,
    domain: &str,
) -> Result<CertificateInfo> {
    let rec = ctx.require_instance(instance_id)?;
    crate::core::cert::renew(ctx, &rec, domain).await
}
