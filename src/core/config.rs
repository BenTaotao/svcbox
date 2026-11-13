use handlebars::Handlebars;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};
use crate::paths::AppPaths;
use crate::utils;

pub const NGINX_MAIN_TMPL: &str = include_str!("../templates/nginx_main.hbs");
pub const NGINX_SITE_TMPL: &str = include_str!("../templates/nginx_site.hbs");
pub const NGINX_SITE_HTTPS_TMPL: &str = include_str!("../templates/nginx_site_https.hbs");
pub const MY_INI_TMPL: &str = include_str!("../templates/my.ini.hbs");
pub const REDIS_CONF_TMPL: &str = include_str!("../templates/redis.conf.hbs");

/// 数据目录 templates/ 下的同名文件优先于内置模板。
pub fn template_text(paths: &AppPaths, name: &str, builtin: &str) -> String {
    fs::read_to_string(paths.templates.join(name)).unwrap_or_else(|_| builtin.to_string())
}

pub fn render(text: &str, data: &impl serde::Serialize) -> Result<String> {
    let mut hb = Handlebars::new();
    hb.register_escape_fn(handlebars::no_escape);
    Ok(hb.render_template(text, data)?)
}

pub fn read_to_string(path: &Path) -> Result<String> {
    Ok(fs::read_to_string(path)?)
}

/// 备份到 backup_dir，返回备份文件路径；原文件不存在时返回 None。
pub fn backup(path: &Path, backup_dir: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    fs::create_dir_all(backup_dir)?;
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .ok_or_else(|| AppError::other("配置文件路径不合法"))?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S-%3f");
    let target = backup_dir.join(format!("{name}.bak.{stamp}"));
    fs::copy(path, &target)?;
    cleanup_backups(backup_dir, &name, 30);
    Ok(Some(target))
}

fn cleanup_backups(dir: &Path, file_name: &str, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let prefix = format!("{file_name}.bak.");
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(&prefix))
        .map(|e| e.path())
        .collect();
    files.sort();
    if files.len() > keep {
        for old in &files[..files.len() - keep] {
            let _ = fs::remove_file(old);
        }
    }
}

/// 配置安全闭环：备份 → 写目标 → 校验 → 失败自动回滚。
pub fn write_validated(
    path: &Path,
    content: &str,
    backup_dir: &Path,
    validate: impl Fn(&Path) -> Result<()>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let backup_file = backup(path, backup_dir)?;
    fs::write(path, content)?;
    match validate(path) {
        Ok(()) => Ok(()),
        Err(e) => {
            match &backup_file {
                Some(b) => {
                    let _ = fs::copy(b, path);
                }
                None => {
                    let _ = fs::remove_file(path);
                }
            }
            Err(e)
        }
    }
}

pub fn nginx_test(nginx: &Path, prefix: &Path, conf: &Path) -> Result<()> {
    let args = vec![
        "-t".to_string(),
        "-p".to_string(),
        format!("{}/", utils::forward(prefix)),
        "-c".to_string(),
        utils::forward(conf),
    ];
    let out = utils::run_tool(nginx, &args, None, &[])?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Err(AppError::External {
        tool: "nginx -t".to_string(),
        code: out.status.code().unwrap_or(-1),
        stderr: if stderr.is_empty() { stdout } else { stderr },
    })
}

/// Redis 没有 `--test-config`，这里做结构化校验，启动时再由健康检测兜底。
pub fn validate_redis_config(text: &str) -> Result<()> {
    if text.contains('\0') {
        return Err(AppError::validation("Redis 配置包含 NUL 字符"));
    }
    for (idx, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, char::is_whitespace);
        let key = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("").trim();
        match key {
            "port" => {
                let port: u16 = value
                    .parse()
                    .map_err(|_| AppError::validation(format!("第 {} 行 port 不合法", idx + 1)))?;
                if port == 0 {
                    return Err(AppError::validation("Redis 端口不能为 0"));
                }
            }
            "bind" => {
                value.parse::<std::net::IpAddr>().map_err(|_| {
                    AppError::validation(format!("第 {} 行 bind 不是合法 IP", idx + 1))
                })?;
            }
            "appendonly" => {
                if value != "yes" && value != "no" {
                    return Err(AppError::validation("appendonly 只能是 yes/no"));
                }
            }
            "requirepass" => {
                if value.trim_matches('"').is_empty() {
                    return Err(AppError::validation("requirepass 不能为空"));
                }
            }
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn site_template_renders_without_leftovers() {
        let out = render(
            NGINX_SITE_TMPL,
            &serde_json::json!({
                "port": 8080,
                "server_name": "test.com",
                "root": "D:/www/test",
                "index": "index.html",
                "access_log": "D:/logs/access.log",
                "error_log": "D:/logs/error.log",
                "acme_root": "D:/instance",
                "custom_locations": ""
            }),
        )
        .unwrap();
        assert!(out.contains("server_name  test.com;"));
        assert!(out.contains("listen       8080;"));
        assert!(!out.contains("{{"));
    }

    #[test]
    fn redis_config_validation() {
        assert!(validate_redis_config("port 6379\nbind 127.0.0.1\nappendonly no").is_ok());
        assert!(validate_redis_config("port abc").is_err());
        assert!(validate_redis_config("bind not-an-ip").is_err());
        assert!(validate_redis_config("appendonly maybe").is_err());
    }

    #[test]
    fn failed_validation_restores_backup() {
        let dir = std::env::temp_dir().join(format!("svcbox-cfg-{}", std::process::id()));
        let backup_dir = dir.join("backup");
        let file = dir.join("test.conf");
        fs::create_dir_all(&dir).unwrap();
        fs::write(&file, "old").unwrap();
        let err = write_validated(&file, "new", &backup_dir, |_| {
            Err(AppError::validation("bad"))
        });
        assert!(err.is_err());
        assert_eq!(fs::read_to_string(&file).unwrap(), "old");
        let _ = fs::remove_dir_all(dir);
    }
}
