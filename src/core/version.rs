use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::core::download::{self, Downloader};
use crate::core::AppContext;
use crate::error::{AppError, Result};
use crate::models::{DownloadSource, ManifestEntry, ServiceKind, VersionInfo, VersionRecord};
use crate::utils;

const BUILTIN_MANIFEST: &str = include_str!("../../manifest.json");

pub fn current_platform() -> &'static str {
    if cfg!(windows) {
        "windows-x64"
    } else {
        "linux-x64"
    }
}

pub struct Manifest {
    entries: Vec<ManifestEntry>,
}

impl Manifest {
    pub fn builtin() -> Result<Self> {
        let entries: Vec<ManifestEntry> = serde_json::from_str(BUILTIN_MANIFEST)?;
        Self::validated(entries)
    }

    pub fn validated(entries: Vec<ManifestEntry>) -> Result<Self> {
        if entries.is_empty() {
            return Err(AppError::validation("版本清单为空"));
        }
        for entry in &entries {
            let ver = entry.version.trim();
            let version_safe = !ver.is_empty()
                && ver.len() <= 64
                && !ver.starts_with('.')
                && !ver.contains("..")
                && ver
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'));
            if !version_safe {
                return Err(AppError::validation(format!(
                    "清单版本号不合法（会作为目录名）: {}",
                    entry.version
                )));
            }
            if entry.platform.trim().is_empty() {
                return Err(AppError::validation("清单条目的 platform 不能为空"));
            }
            if !entry.download_url.starts_with("https://") {
                return Err(AppError::validation(format!(
                    "拒绝非 HTTPS 下载地址: {}",
                    entry.download_url
                )));
            }
            if !matches!(entry.archive_type.as_str(), "zip" | "tar.gz" | "tgz") {
                return Err(AppError::validation(format!(
                    "不支持的压缩格式: {}",
                    entry.archive_type
                )));
            }
            safe_relative(Path::new(&entry.bin_path))?;
            let sha = entry.sha256.trim();
            if !sha.is_empty() && (sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit())) {
                return Err(AppError::validation(format!(
                    "{} {} 的 sha256 格式不合法",
                    entry.kind, entry.version
                )));
            }
        }
        Ok(Self { entries })
    }

    /// 内置清单 + 远端缓存，同 key 远端优先。
    pub fn load(ctx: &AppContext) -> Result<Self> {
        let mut merged: HashMap<(String, String, String), ManifestEntry> = HashMap::new();
        for entry in Self::builtin()?.entries {
            merged.insert(
                (
                    entry.kind.as_str().to_string(),
                    entry.version.clone(),
                    entry.platform.clone(),
                ),
                entry,
            );
        }
        let remote_file = ctx.paths.cache.join("manifest.remote.json");
        if let Ok(raw) = fs::read_to_string(remote_file) {
            match serde_json::from_str::<Vec<ManifestEntry>>(&raw) {
                Ok(entries) => {
                    for entry in Self::validated(entries)?.entries {
                        merged.insert(
                            (
                                entry.kind.as_str().to_string(),
                                entry.version.clone(),
                                entry.platform.clone(),
                            ),
                            entry,
                        );
                    }
                }
                Err(e) => tracing::warn!("远端清单缓存解析失败，忽略: {e}"),
            }
        }
        let mut entries: Vec<ManifestEntry> = merged.into_values().collect();
        entries.sort_by(|a, b| {
            a.kind
                .as_str()
                .cmp(b.kind.as_str())
                .then_with(|| b.version.cmp(&a.version))
        });
        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[ManifestEntry] {
        &self.entries
    }

    pub fn find(&self, kind: ServiceKind, version: &str) -> Option<&ManifestEntry> {
        let platform = current_platform();
        self.entries
            .iter()
            .find(|e| e.kind == kind && e.version == version && e.platform == platform)
    }
}

pub async fn refresh_remote(ctx: &AppContext, url: &str) -> Result<usize> {
    if !url.starts_with("https://") {
        return Err(AppError::validation("远端清单地址必须使用 HTTPS"));
    }
    let client = reqwest::Client::builder()
        .user_agent(format!("svcbox/{}", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let resp = client.get(url).send().await?.error_for_status()?;
    let text = resp.text().await?;
    let entries: Vec<ManifestEntry> = serde_json::from_str(&text)
        .map_err(|e| AppError::validation(format!("远端清单 JSON 不合法: {e}")))?;
    let manifest = Manifest::validated(entries)?;
    let count = manifest.entries.len();
    let cache = ctx.paths.cache.join("manifest.remote.json");
    let tmp = ctx.paths.cache.join("manifest.remote.json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(&manifest.entries)?)?;
    fs::rename(tmp, cache)?;
    Ok(count)
}

/// 已安装版本的固定主程序路径，不依赖当前清单（用于旧版本/清单更新后的自检与清理）。
pub fn installed_binary(rec: &VersionRecord) -> PathBuf {
    let name = match rec.kind {
        ServiceKind::Nginx => {
            if cfg!(windows) {
                "nginx.exe"
            } else {
                "nginx"
            }
        }
        ServiceKind::Mysql => {
            if cfg!(windows) {
                "bin/mysqld.exe"
            } else {
                "bin/mysqld"
            }
        }
        ServiceKind::Redis => {
            if cfg!(windows) {
                "redis-server.exe"
            } else {
                "redis-server"
            }
        }
    };
    Path::new(&rec.install_path).join(name)
}

pub async fn list_available(ctx: &AppContext) -> Result<Vec<VersionInfo>> {
    let installed = ctx.db.list_versions()?;
    let manifest = Manifest::load(ctx)?;
    let platform = current_platform();
    let mut out = Vec::new();
    for entry in manifest.entries() {
        if entry.platform != platform {
            continue;
        }
        let record = installed
            .iter()
            .find(|v| v.kind == entry.kind && v.version == entry.version);
        let bin_ok = record
            .as_ref()
            .map(|r| Path::new(&r.install_path).join(&entry.bin_path).is_file())
            .unwrap_or(false);
        let installed_flag = record.is_some() && bin_ok;
        out.push(VersionInfo {
            kind: entry.kind,
            version: entry.version.clone(),
            platform: entry.platform.clone(),
            installed: installed_flag,
            installed_path: if installed_flag {
                record.map(|r| r.install_path.clone())
            } else {
                None
            },
            download_url: entry.download_url.clone(),
            mirror_url: entry.mirror_url.clone(),
            sha256: entry.sha256.clone(),
            bin_path: entry.bin_path.clone(),
            archive_type: entry.archive_type.clone(),
        });
    }
    // 清单更新后仍保留的旧版本也要出现在列表中，方便删除或继续使用
    for record in installed {
        if record.platform != platform {
            continue;
        }
        if out
            .iter()
            .any(|v| v.kind == record.kind && v.version == record.version)
        {
            continue;
        }
        let bin_path = match record.kind {
            ServiceKind::Nginx => "nginx.exe",
            ServiceKind::Mysql => "bin/mysqld.exe",
            ServiceKind::Redis => "redis-server.exe",
        };
        out.push(VersionInfo {
            kind: record.kind,
            version: record.version,
            platform: record.platform,
            installed: true,
            installed_path: Some(record.install_path),
            download_url: String::new(),
            mirror_url: None,
            sha256: record.sha256,
            bin_path: bin_path.to_string(),
            archive_type: "zip".to_string(),
        });
    }
    out.sort_by(|a, b| {
        a.kind
            .as_str()
            .cmp(b.kind.as_str())
            .then_with(|| b.version.cmp(&a.version))
    });
    Ok(out)
}

pub fn ensure_installed(ctx: &AppContext, kind: ServiceKind, version: &str) -> Result<PathBuf> {
    let record = ctx
        .db
        .get_version(kind, version)?
        .ok_or_else(|| AppError::not_found(format!("{kind} {version} 尚未安装")))?;
    let dir = PathBuf::from(&record.install_path);
    if !dir.is_dir() {
        return Err(AppError::not_found(format!(
            "{kind} {version} 目录不存在: {}",
            dir.display()
        )));
    }
    Ok(dir)
}

pub fn binary_path(ctx: &AppContext, kind: ServiceKind, version: &str) -> Result<PathBuf> {
    let dir = ensure_installed(ctx, kind, version)?;
    let manifest = Manifest::load(ctx)?;
    let entry = manifest
        .find(kind, version)
        .ok_or_else(|| AppError::not_found(format!("清单中没有 {kind} {version}")))?;
    let bin = dir.join(safe_relative(Path::new(&entry.bin_path))?);
    if !bin.is_file() {
        return Err(AppError::not_found(format!(
            "主程序不存在: {}",
            bin.display()
        )));
    }
    Ok(bin)
}

pub async fn install(ctx: &AppContext, kind: ServiceKind, version: &str) -> Result<VersionInfo> {
    let _guard = ctx.install_lock.lock().await;
    let manifest = Manifest::load(ctx)?;
    let entry = manifest
        .find(kind, version)
        .ok_or_else(|| AppError::not_found(format!("清单中没有 {kind} {version}")))?
        .clone();

    if entry.sha256.trim().is_empty() {
        return Err(AppError::validation(format!(
            "{kind} {version} 在清单中缺少 sha256，出于安全考虑拒绝安装。\
             请更新远端清单，或用 `version import` 手动导入并显式提供校验值"
        )));
    }

    if let Ok(dir) = ensure_installed(ctx, kind, version) {
        if dir
            .join(safe_relative(Path::new(&entry.bin_path))?)
            .is_file()
        {
            return version_info(ctx, &entry);
        }
    }

    let settings = ctx.db.load_settings()?;
    let url = pick_url(&entry, settings.download_source);
    // 主源失败时尝试备用源（远端清单可配 mirror_url；MySQL 官方 CDN 直链为主源）
    let alternate = match settings.download_source {
        DownloadSource::Official => entry
            .mirror_url
            .clone()
            .filter(|u| !u.trim().is_empty() && u != &url),
        DownloadSource::Mirror => Some(entry.download_url.clone()).filter(|u| u != &url),
    };
    let downloader = Downloader::new(ctx.paths.cache_packages.clone(), ctx.progress_callback())?;
    let mut archive_result = downloader
        .fetch(kind.as_str(), version, &url, Some(&entry.sha256))
        .await;
    if let Err(e) = &archive_result {
        if let Some(alt) = &alternate {
            tracing::warn!("主下载源失败（{e}），尝试备用源 {alt}");
            archive_result = downloader
                .fetch(kind.as_str(), version, alt, Some(&entry.sha256))
                .await;
        }
    }
    let archive = archive_result?;

    let outcome = finish_install(ctx, &entry, &archive, &url).await;
    let audit_result = outcome.as_ref().map(|_| ()).map_err(|e| e.to_string());
    utils::audit(
        &ctx.paths.home,
        "version.install",
        json!({"kind": kind.as_str(), "version": version, "url": url}),
        &audit_result,
    );
    outcome?;
    version_info(ctx, &entry)
}

/// 手动导入官方包：仍需 sha256 匹配（清单值或调用方显式提供）。
pub async fn import(
    ctx: &AppContext,
    kind: ServiceKind,
    version: &str,
    file: &Path,
    sha256: Option<String>,
) -> Result<VersionInfo> {
    let _guard = ctx.install_lock.lock().await;
    let manifest = Manifest::load(ctx)?;
    let mut entry = manifest
        .find(kind, version)
        .ok_or_else(|| AppError::not_found(format!("清单中没有 {kind} {version}")))?
        .clone();
    let expected = sha256
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| entry.sha256.clone());
    if expected.trim().is_empty() {
        return Err(AppError::validation(
            "手动导入必须提供 sha256（--sha256 参数或清单中已配置）",
        ));
    }
    let actual = download::sha256_file(file).await?;
    if !actual.eq_ignore_ascii_case(expected.trim()) {
        return Err(AppError::validation(format!(
            "sha256 不匹配：期望 {expected}，实际 {actual}"
        )));
    }
    entry.sha256 = actual;
    // 已安装且主程序存在时直接返回，避免重复导入时删除正在使用的版本目录
    let bin_rel = safe_relative(Path::new(&entry.bin_path))?;
    if let Ok(dir) = ensure_installed(ctx, kind, version) {
        if dir.join(&bin_rel).is_file() {
            return version_info(ctx, &entry);
        }
    }
    let archive = download::import_package(file, &ctx.paths.cache_packages)?;
    let result = finish_install(ctx, &entry, &archive, "manual-import").await;
    let audit_result = result.as_ref().map(|_| ()).map_err(|e| e.to_string());
    utils::audit(
        &ctx.paths.home,
        "version.import",
        json!({"kind": kind.as_str(), "version": version, "file": file.to_string_lossy()}),
        &audit_result,
    );
    result?;
    version_info(ctx, &entry)
}

async fn finish_install(
    ctx: &AppContext,
    entry: &ManifestEntry,
    archive: &Path,
    source_url: &str,
) -> Result<()> {
    let kind_dir = ctx.paths.versions.join(entry.kind.as_str());
    let final_dir = kind_dir.join(&entry.version);
    let staging = ctx.paths.versions.join(format!(
        ".install-{}-{}-{}",
        entry.kind.as_str(),
        entry.version,
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;

    let result = (|| -> Result<()> {
        download::extract_archive(archive, &entry.archive_type, &staging)?;
        let bin_rel = safe_relative(Path::new(&entry.bin_path))?;
        if !staging.join(&bin_rel).is_file() {
            return Err(AppError::validation(format!(
                "解压后未找到主程序 {}，压缩包结构可能已变化",
                bin_rel.display()
            )));
        }
        fs::create_dir_all(&kind_dir)?;
        // 先改名旧目录而非直接删除；新目录就位失败时再恢复，避免把正在使用的版本拆坏
        let backup = kind_dir.join(format!(
            ".old-{}-{}-{}",
            entry.version,
            std::process::id(),
            utils::now_stamp()
        ));
        let had_old = final_dir.exists();
        if had_old {
            fs::rename(&final_dir, &backup)?;
        }
        if let Err(e) = fs::rename(&staging, &final_dir) {
            if had_old {
                let _ = fs::rename(&backup, &final_dir);
            }
            return Err(e.into());
        }
        if had_old {
            let _ = fs::remove_dir_all(&backup);
        }
        ctx.db.upsert_version(&VersionRecord {
            kind: entry.kind,
            version: entry.version.clone(),
            platform: entry.platform.clone(),
            install_path: final_dir.to_string_lossy().to_string(),
            source_url: source_url.to_string(),
            sha256: entry.sha256.clone(),
            installed_at: utils::now_rfc3339(),
        })?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    if result.is_ok() {
        ctx.emit(
            "version-changed",
            json!({"kind": entry.kind.as_str(), "version": entry.version, "action": "installed"}),
        );
    }
    result
}

/// 清理“已安装但不在当前清单、且没有实例引用”的旧版本。
pub fn prune(ctx: &AppContext) -> Result<Vec<String>> {
    let manifest = Manifest::load(ctx)?;
    let instances = ctx.db.list_instances()?;
    let mut removed = Vec::new();
    for record in ctx.db.list_versions()? {
        let in_manifest = manifest
            .entries()
            .iter()
            .any(|e| e.kind == record.kind && e.version == record.version);
        if in_manifest {
            continue;
        }
        if instances
            .iter()
            .any(|i| i.kind == record.kind && i.version == record.version)
        {
            continue;
        }
        let dir = PathBuf::from(&record.install_path);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        ctx.db.remove_version(record.kind, &record.version)?;
        removed.push(format!("{} {}", record.kind, record.version));
    }
    if !removed.is_empty() {
        utils::audit(
            &ctx.paths.home,
            "version.prune",
            json!({"removed": removed}),
            &Ok(()),
        );
        ctx.emit("version-changed", json!({"action": "pruned"}));
    }
    Ok(removed)
}

pub fn remove(ctx: &AppContext, kind: ServiceKind, version: &str) -> Result<()> {
    let instances = ctx.db.list_instances()?;
    if instances
        .iter()
        .any(|i| i.kind == kind && i.version == version)
    {
        return Err(AppError::conflict(format!(
            "{kind} {version} 仍被实例引用，请先删除相关实例"
        )));
    }
    if let Some(record) = ctx.db.get_version(kind, version)? {
        let dir = PathBuf::from(&record.install_path);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        ctx.db.remove_version(kind, version)?;
        utils::audit(
            &ctx.paths.home,
            "version.remove",
            json!({"kind": kind.as_str(), "version": version}),
            &Ok(()),
        );
    }
    Ok(())
}

fn version_info(ctx: &AppContext, entry: &ManifestEntry) -> Result<VersionInfo> {
    let record = ctx.db.get_version(entry.kind, &entry.version)?;
    Ok(VersionInfo {
        kind: entry.kind,
        version: entry.version.clone(),
        platform: entry.platform.clone(),
        installed: record.is_some(),
        installed_path: record.map(|r| r.install_path),
        download_url: entry.download_url.clone(),
        mirror_url: entry.mirror_url.clone(),
        sha256: entry.sha256.clone(),
        bin_path: entry.bin_path.clone(),
        archive_type: entry.archive_type.clone(),
    })
}

fn pick_url(entry: &ManifestEntry, source: DownloadSource) -> String {
    match source {
        DownloadSource::Mirror => entry
            .mirror_url
            .clone()
            .filter(|u| !u.trim().is_empty())
            .unwrap_or_else(|| entry.download_url.clone()),
        DownloadSource::Official => entry.download_url.clone(),
    }
}

fn safe_relative(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Err(AppError::validation("bin_path 必须为相对路径"));
    }
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            _ => return Err(AppError::validation("bin_path 包含非法路径片段")),
        }
    }
    if out.as_os_str().is_empty() {
        return Err(AppError::validation("bin_path 不能为空"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_manifest_is_valid() {
        let m = Manifest::builtin().unwrap();
        assert!(m.entries().iter().any(|e| e.kind == ServiceKind::Nginx));
        assert!(m.entries().iter().any(|e| e.kind == ServiceKind::Mysql));
        assert!(m.entries().iter().any(|e| e.kind == ServiceKind::Redis));
    }

    #[test]
    fn rejects_path_escape_bin() {
        assert!(safe_relative(Path::new("../evil.exe")).is_err());
        assert!(safe_relative(Path::new("C:/evil.exe")).is_err());
        assert!(safe_relative(Path::new("bin/mysqld.exe")).is_ok());
    }

    #[test]
    fn rejects_unsafe_version_directory() {
        let entry = ManifestEntry {
            kind: ServiceKind::Nginx,
            version: "../../evil".into(),
            platform: "windows-x64".into(),
            download_url: "https://example.com/a.zip".into(),
            mirror_url: None,
            sha256: "".into(),
            archive_type: "zip".into(),
            bin_path: "x.exe".into(),
        };
        assert!(Manifest::validated(vec![entry]).is_err());
    }

    #[test]
    fn rejects_http_manifest_url() {
        let entry = ManifestEntry {
            kind: ServiceKind::Nginx,
            version: "1.0".into(),
            platform: "windows-x64".into(),
            download_url: "http://example.com/a.zip".into(),
            mirror_url: None,
            sha256: "".into(),
            archive_type: "zip".into(),
            bin_path: "x.exe".into(),
        };
        assert!(Manifest::validated(vec![entry]).is_err());
    }
}
