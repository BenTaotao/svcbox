use serde_json::json;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::core::instance;
use crate::core::AppContext;
use crate::error::{AppError, Result};
use crate::models::{CertificateInfo, CertificateRecord, InstanceRecord, ServiceKind};
use crate::utils;

const ACME_PINNED_URL: &str =
    "https://raw.githubusercontent.com/acmesh-official/acme.sh/3.1.1/acme.sh";
const ACME_FALLBACK_URL: &str =
    "https://raw.githubusercontent.com/acmesh-official/acme.sh/master/acme.sh";

pub fn list(ctx: &AppContext) -> Result<Vec<CertificateInfo>> {
    let instances = ctx.db.list_instances()?;
    let now = chrono::Utc::now();
    let mut out = Vec::new();
    for cert in ctx.db.list_certificates()? {
        let name = instances
            .iter()
            .find(|i| i.id == cert.instance_id)
            .map(|i| i.name.clone())
            .unwrap_or_else(|| "已删除".to_string());
        out.push(to_info(&cert, &name, now));
    }
    Ok(out)
}

fn to_info(
    cert: &CertificateRecord,
    instance_name: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> CertificateInfo {
    let expires = chrono::DateTime::parse_from_rfc3339(&cert.expires_at)
        .map(|d| d.with_timezone(&chrono::Utc))
        .unwrap_or(now);
    CertificateInfo {
        domain: cert.domain.clone(),
        instance_id: cert.instance_id.clone(),
        instance_name: instance_name.to_string(),
        cert_path: cert.cert_path.clone(),
        key_path: cert.key_path.clone(),
        issued_at: cert.issued_at.clone(),
        expires_at: cert.expires_at.clone(),
        days_left: (expires - now).num_days(),
    }
}

fn cert_expiry_timestamp(path: &Path) -> Result<i64> {
    let pem_bytes = fs::read(path)?;
    let (_, pem) = x509_parser::pem::parse_x509_pem(&pem_bytes)
        .map_err(|e| AppError::other(format!("证书解析失败: {e}")))?;
    let cert = pem
        .parse_x509()
        .map_err(|e| AppError::other(format!("证书解析失败: {e}")))?;
    Ok(cert.validity().not_after.timestamp())
}

async fn ensure_acme_script(ctx: &AppContext) -> Result<PathBuf> {
    let dir = &ctx.paths.cache_acme;
    fs::create_dir_all(dir)?;
    let script = dir.join("acme.sh");
    if script.is_file() {
        return Ok(script);
    }

    let client = reqwest::Client::builder()
        .user_agent(format!("svcbox/{}", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(60))
        .build()?;

    let mut last_error = None;
    for url in [ACME_PINNED_URL, ACME_FALLBACK_URL] {
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => {
                let bytes = resp.bytes().await?;
                if bytes.len() < 20_000 {
                    last_error = Some(AppError::other("acme.sh 下载内容异常（文件过小）"));
                    continue;
                }
                let actual = hex::encode(Sha256::digest(&bytes));
                if let Ok(expected) = std::env::var("SVCBOX_ACME_SHA256") {
                    let expected = expected.trim().to_ascii_lowercase();
                    if !expected.is_empty() && expected != actual {
                        return Err(AppError::validation(format!(
                            "acme.sh sha256 校验失败：期望 {expected}，实际 {actual}"
                        )));
                    }
                }
                let tmp = dir.join("acme.sh.tmp");
                fs::write(&tmp, &bytes)?;
                fs::rename(&tmp, &script)?;
                fs::write(dir.join("acme.sh.sha256"), &actual)?;
                tracing::info!("已下载 acme.sh，sha256={actual}");
                return Ok(script);
            }
            Ok(resp) => {
                last_error = Some(AppError::other(format!(
                    "下载 acme.sh 失败，HTTP {}",
                    resp.status().as_u16()
                )));
            }
            Err(e) => last_error = Some(e.into()),
        }
    }
    Err(last_error.unwrap_or_else(|| AppError::other("下载 acme.sh 失败")))
}

async fn run_acme(bash: &Path, script: &Path, home: &Path, args: &[String]) -> Result<String> {
    fs::create_dir_all(home)?;
    let mut cmd = tokio::process::Command::new(bash);
    // Git Bash 对反斜杠路径处理不一致，统一传正斜杠
    cmd.arg(utils::forward(script))
        .arg("--home")
        .arg(utils::forward(home))
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = tokio::time::timeout(Duration::from_secs(240), cmd.output())
        .await
        .map_err(|_| AppError::other("acme.sh 执行超时（240 秒）"))??;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if !output.status.success() {
        return Err(AppError::External {
            tool: "acme.sh".to_string(),
            code: output.status.code().unwrap_or(-1),
            stderr: utils::tail_lines(&format!("{stdout}\n{stderr}"), 30),
        });
    }
    Ok(format!("{stdout}\n{stderr}"))
}

fn require_issue_prereq(rec: &InstanceRecord) -> Result<()> {
    if rec.kind != ServiceKind::Nginx {
        return Err(AppError::validation("证书只能绑定到 Nginx 实例"));
    }
    if rec.port != 80 {
        return Err(AppError::validation(format!(
            "HTTP-01 验证要求实例监听 80 端口，当前为 {}",
            rec.port
        )));
    }
    Ok(())
}

pub async fn issue(
    ctx: &AppContext,
    rec: &InstanceRecord,
    domain: &str,
) -> Result<CertificateInfo> {
    utils::validate_domain(domain)?;
    require_issue_prereq(rec)?;
    if !instance::is_running(ctx, rec) {
        return Err(AppError::conflict("申请证书前请先启动 Nginx 实例"));
    }

    let existing = ctx.db.get_certificate(&rec.id, domain)?;
    if let Some(cert) = &existing {
        let expires = chrono::DateTime::parse_from_rfc3339(&cert.expires_at)
            .map(|d| d.with_timezone(&chrono::Utc))
            .unwrap_or_else(|_| chrono::Utc::now());
        if expires > chrono::Utc::now() + chrono::Duration::days(30) {
            return Ok(to_info(cert, &rec.name, chrono::Utc::now()));
        }
    }

    let bash = utils::find_bash().ok_or_else(|| {
        AppError::other("未找到 bash。acme.sh 需要 Git for Windows（含 bash/curl/openssl）")
    })?;
    let script = ensure_acme_script(ctx).await?;
    let acme_home = ctx.paths.cache_acme.join("home");
    let webroot = rec.dir.clone();
    fs::create_dir_all(webroot.join(".well-known").join("acme-challenge"))?;

    let mut args = vec![
        "--issue".to_string(),
        "--server".to_string(),
        "letsencrypt".to_string(),
        "-d".to_string(),
        domain.to_string(),
        "-w".to_string(),
        utils::forward(&webroot),
        "--keylength".to_string(),
        "ec-256".to_string(),
    ];
    // 已存在的证书（包括过期）需要 --force 才会重新签发
    if existing.is_some() {
        args.push("--force".to_string());
    }
    let log = run_acme(&bash, &script, &acme_home, &args).await;
    let audit = log.as_ref().map(|_| ()).map_err(|e| e.to_string());
    utils::audit(&rec.dir, "cert.issue", json!({"domain": domain}), &audit);
    if let Err(e) = log {
        ctx.notify(&format!("证书申请失败: {domain}"), &e.to_string());
        return Err(e);
    }

    install_cert(ctx, rec, domain, &bash, &script, &acme_home).await?;
    let info = cert_info_from_db(ctx, rec, domain)?;
    ctx.notify(
        &format!("证书申请成功: {domain}"),
        &format!("到期时间 {}", info.expires_at),
    );
    Ok(info)
}

async fn install_cert(
    ctx: &AppContext,
    rec: &InstanceRecord,
    domain: &str,
    bash: &Path,
    script: &Path,
    acme_home: &Path,
) -> Result<()> {
    let dir = instance::cert_dir(rec, domain);
    fs::create_dir_all(&dir)?;
    let key = dir.join("key.pem");
    let fullchain = dir.join("fullchain.pem");
    let args = vec![
        "--install-cert".to_string(),
        "-d".to_string(),
        domain.to_string(),
        "--ecc".to_string(),
        "--key-file".to_string(),
        utils::forward(&key),
        "--fullchain-file".to_string(),
        utils::forward(&fullchain),
    ];
    run_acme(bash, script, acme_home, &args).await?;
    if !key.is_file() || !fullchain.is_file() {
        return Err(AppError::other("acme.sh 未生成预期的证书文件"));
    }
    let expiry_ts = cert_expiry_timestamp(&fullchain)?;
    let expires_at = chrono::DateTime::<chrono::Utc>::from_timestamp(expiry_ts, 0)
        .ok_or_else(|| AppError::other("证书到期时间不合法"))?
        .to_rfc3339();
    ctx.db.upsert_certificate(&CertificateRecord {
        id: 0,
        instance_id: rec.id.clone(),
        domain: domain.to_string(),
        cert_path: utils::forward(&fullchain),
        key_path: utils::forward(&key),
        issued_at: utils::now_rfc3339(),
        expires_at,
    })?;
    Ok(())
}

fn cert_info_from_db(
    ctx: &AppContext,
    rec: &InstanceRecord,
    domain: &str,
) -> Result<CertificateInfo> {
    let cert = ctx
        .db
        .get_certificate(&rec.id, domain)?
        .ok_or_else(|| AppError::not_found(format!("证书 {domain} 不存在")))?;
    Ok(to_info(&cert, &rec.name, chrono::Utc::now()))
}

pub async fn renew(
    ctx: &AppContext,
    rec: &InstanceRecord,
    domain: &str,
) -> Result<CertificateInfo> {
    utils::validate_domain(domain)?;
    require_issue_prereq(rec)?;
    if ctx.db.get_certificate(&rec.id, domain)?.is_none() {
        return Err(AppError::not_found(format!("证书 {domain} 不存在")));
    }
    if !instance::is_running(ctx, rec) {
        return Err(AppError::conflict("续期前请先启动 Nginx 实例"));
    }
    let bash =
        utils::find_bash().ok_or_else(|| AppError::other("未找到 bash，无法运行 acme.sh"))?;
    let script = ensure_acme_script(ctx).await?;
    let acme_home = ctx.paths.cache_acme.join("home");
    let args = vec![
        "--renew".to_string(),
        "-d".to_string(),
        domain.to_string(),
        "--ecc".to_string(),
        "--force".to_string(),
    ];
    let log = run_acme(&bash, &script, &acme_home, &args).await;
    let audit = log.as_ref().map(|_| ()).map_err(|e| e.to_string());
    utils::audit(&rec.dir, "cert.renew", json!({"domain": domain}), &audit);
    log?;
    install_cert(ctx, rec, domain, &bash, &script, &acme_home).await?;
    // 证书文件已替换，运行中的 Nginx 必须 reload 才会加载新证书
    if rec.kind == ServiceKind::Nginx && instance::is_running(ctx, rec) {
        if let Err(e) = crate::core::service::nginx::reload(ctx, &rec.id).await {
            tracing::warn!("证书续期后 Nginx reload 失败: {e}");
        }
    }
    let info = cert_info_from_db(ctx, rec, domain)?;
    ctx.notify(
        &format!("证书续期成功: {domain}"),
        &format!("新到期时间 {}", info.expires_at),
    );
    Ok(info)
}

/// 每日检查一次，剩余不足 30 天的证书自动续期。
pub async fn renew_loop(ctx: Arc<AppContext>) {
    tokio::time::sleep(Duration::from_secs(300)).await;
    loop {
        if let Err(e) = renew_due(&ctx).await {
            tracing::warn!("证书到期检查失败: {e}");
        }
        tokio::time::sleep(Duration::from_secs(24 * 3600)).await;
    }
}

pub fn spawn_renew_task(ctx: Arc<AppContext>) {
    if tokio::runtime::Handle::try_current().is_err() {
        tracing::warn!("没有可用的 tokio runtime，跳过证书自动续期任务");
        return;
    }
    tokio::spawn(renew_loop(ctx));
}

pub async fn renew_due(ctx: &AppContext) -> Result<Vec<CertificateInfo>> {
    let mut renewed = Vec::new();
    for cert in list(ctx)? {
        if cert.days_left >= 30 {
            continue;
        }
        let Ok(rec) = ctx.require_instance(&cert.instance_id) else {
            continue;
        };
        if !instance::is_running(ctx, &rec) {
            tracing::warn!("实例 {} 未运行，跳过证书 {} 自动续期", rec.id, cert.domain);
            continue;
        }
        match renew(ctx, &rec, &cert.domain).await {
            Ok(info) => renewed.push(info),
            Err(e) => {
                tracing::warn!("证书 {} 自动续期失败: {e}", cert.domain);
                ctx.notify(&format!("证书续期失败: {}", cert.domain), &e.to_string());
            }
        }
    }
    Ok(renewed)
}
