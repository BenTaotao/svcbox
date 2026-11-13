use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

use crate::error::{AppError, Result};
use crate::models::DownloadProgress;

pub type ProgressCallback = Arc<dyn Fn(DownloadProgress) + Send + Sync>;

pub struct Downloader {
    client: reqwest::Client,
    cache_dir: PathBuf,
    progress: ProgressCallback,
}

impl Downloader {
    pub fn new(cache_dir: PathBuf, progress: ProgressCallback) -> Result<Self> {
        fs::create_dir_all(&cache_dir)?;
        let client = reqwest::Client::builder()
            .user_agent(format!("svcbox/{}", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?;
        Ok(Self {
            client,
            cache_dir,
            progress,
        })
    }

    fn emit(&self, kind: &str, version: &str, downloaded: u64, total: Option<u64>, stage: &str) {
        let percent = match total {
            Some(t) if t > 0 => (downloaded as f64 / t as f64 * 100.0).min(100.0),
            _ => 0.0,
        };
        (self.progress)(DownloadProgress {
            kind: kind.to_string(),
            version: version.to_string(),
            downloaded,
            total,
            percent,
            stage: stage.to_string(),
        });
    }

    /// 下载并做 sha256 校验。缓存命中且校验通过时直接复用。
    pub async fn fetch(
        &self,
        kind: &str,
        version: &str,
        url: &str,
        expected_sha: Option<&str>,
    ) -> Result<PathBuf> {
        let raw_name = url
            .split('?')
            .next()
            .and_then(|u| u.rsplit('/').next())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::validation(format!("无法从 URL 解析文件名: {url}")))?;
        let file_name = safe_file_name(raw_name)
            .ok_or_else(|| AppError::validation(format!("下载文件名不合法: {raw_name}")))?
            .to_string();
        let final_path = self.cache_dir.join(&file_name);
        let part_path = self.cache_dir.join(format!("{file_name}.part"));
        let expected = expected_sha.map(|s| s.trim().to_ascii_lowercase());

        if final_path.exists() {
            self.emit(kind, version, 0, None, "校验缓存");
            let actual = sha256_file(&final_path).await?;
            match &expected {
                Some(want) if !want.is_empty() && &actual != want => {
                    tracing::warn!("缓存 {file_name} 校验失败，删除后重新下载");
                    let _ = fs::remove_file(&final_path);
                }
                _ => {
                    let size = fs::metadata(&final_path)?.len();
                    self.emit(kind, version, size, Some(size), "缓存命中");
                    return Ok(final_path);
                }
            }
        }

        let mut last_error = None;
        for attempt in 1..=4 {
            match self
                .download_attempt(kind, version, url, &part_path, &final_path)
                .await
            {
                Ok(()) => {
                    self.emit(
                        kind,
                        version,
                        fs::metadata(&final_path)?.len(),
                        fs::metadata(&final_path).ok().map(|m| m.len()),
                        "校验 sha256",
                    );
                    let actual = sha256_file(&final_path).await?;
                    if let Some(want) = &expected {
                        if !want.is_empty() && &actual != want {
                            let _ = fs::remove_file(&final_path);
                            let _ = fs::remove_file(&part_path);
                            return Err(AppError::validation(format!(
                                "sha256 校验失败：期望 {want}，实际 {actual}。已删除缓存文件"
                            )));
                        }
                        if want.is_empty() {
                            tracing::warn!("{kind} {version} 未配置 sha256，已跳过强校验");
                        }
                    } else {
                        tracing::warn!("{kind} {version} 未配置 sha256，已跳过强校验");
                    }
                    return Ok(final_path);
                }
                Err(e) => {
                    tracing::warn!("下载 {url} 第 {attempt} 次失败: {e}");
                    last_error = Some(e);
                    tokio::time::sleep(Duration::from_secs(attempt as u64)).await;
                }
            }
        }
        Err(last_error.unwrap_or_else(|| AppError::other("下载失败")))
    }

    async fn download_attempt(
        &self,
        kind: &str,
        version: &str,
        url: &str,
        part_path: &Path,
        final_path: &Path,
    ) -> Result<()> {
        let mut offset = fs::metadata(part_path).map(|m| m.len()).unwrap_or(0);
        let mut hasher = Sha256::new();
        if offset > 0 {
            let mut existing = tokio::fs::File::open(part_path).await?;
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = tokio::io::AsyncReadExt::read(&mut existing, &mut buf).await?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
        }

        let mut req = self.client.get(url);
        if offset > 0 {
            req = req.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let response = req.send().await?;
        let status = response.status();
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            let _ = fs::remove_file(part_path);
            return Err(AppError::other("断点续传区间无效，已重置"));
        }
        if !status.is_success() {
            return Err(AppError::other(format!(
                "下载失败，HTTP {}: {url}",
                status.as_u16()
            )));
        }

        let append = status == reqwest::StatusCode::PARTIAL_CONTENT && offset > 0;
        if !append {
            offset = 0;
            hasher = Sha256::new();
        }
        let total = response.content_length().map(|len| len + offset);
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(part_path)
            .await?;

        let mut downloaded = offset;
        let mut last_emit = Instant::now();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            hasher.update(&chunk);
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            if last_emit.elapsed() >= Duration::from_millis(120) {
                self.emit(kind, version, downloaded, total, "下载中");
                last_emit = Instant::now();
            }
        }
        file.flush().await?;
        drop(file);

        let actual = hex::encode(hasher.finalize());
        let expected = fs::metadata(part_path).is_ok();
        if !expected {
            return Err(AppError::other("下载文件为空"));
        }
        tracing::debug!("下载完成 {url}, sha256={actual}");
        if final_path.exists() {
            fs::remove_file(final_path)?;
        }
        fs::rename(part_path, final_path)?;
        self.emit(kind, version, downloaded, Some(downloaded), "下载完成");
        Ok(())
    }
}

fn safe_file_name(name: &str) -> Option<&str> {
    let ok = !name.is_empty()
        && name.len() <= 200
        && name != "."
        && name != ".."
        && !name.contains("..")
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'));
    if ok {
        Some(name)
    } else {
        None
    }
}

pub async fn sha256_file(path: &Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = tokio::io::AsyncReadExt::read(&mut file, &mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// 解压 zip / tar.gz，zip 自动剥掉唯一的顶层目录（官方包常见结构）。
pub fn extract_archive(archive: &Path, archive_type: &str, dest: &Path) -> Result<()> {
    fs::create_dir_all(dest)?;
    match archive_type {
        "zip" => extract_zip(archive, dest),
        "tar.gz" | "tgz" => extract_tar_gz(archive, dest),
        other => Err(AppError::validation(format!(
            "不支持的压缩格式 {other}，仅支持 zip / tar.gz"
        ))),
    }
}

fn extract_zip(archive: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file)?;

    let mut names = Vec::new();
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
        let Some(path) = entry.enclosed_name() else {
            return Err(AppError::validation(format!(
                "压缩包包含非法路径: {}",
                entry.name()
            )));
        };
        names.push(path);
    }
    let strip_root = common_root(&names);

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        let Some(raw_path) = entry.enclosed_name() else {
            continue;
        };
        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                return Err(AppError::validation("压缩包包含符号链接，已拒绝解压"));
            }
        }
        let relative = match &strip_root {
            Some(root) => match raw_path.strip_prefix(root) {
                Ok(p) => p.to_path_buf(),
                Err(_) => continue,
            },
            None => raw_path,
        };
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = dest.join(&relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut out = File::create(&target)?;
        std::io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

fn common_root(paths: &[PathBuf]) -> Option<PathBuf> {
    let mut roots = BTreeSet::new();
    for p in paths {
        let first = p.components().next()?;
        match first {
            Component::Normal(s) => {
                roots.insert(s.to_string_lossy().to_string());
            }
            _ => return None,
        }
    }
    if roots.len() != 1 {
        return None;
    }
    let root = PathBuf::from(roots.into_iter().next()?);
    // 目录项本身等于 root，文件都在 root 之下时才剥离
    let all_inside = paths.iter().all(|p| p == &root || p.starts_with(&root));
    let has_child = paths.iter().any(|p| p != &root);
    if all_inside && has_child {
        Some(root)
    } else {
        None
    }
}

fn extract_tar_gz(archive: &Path, dest: &Path) -> Result<()> {
    let file = File::open(archive)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut tar = tar::Archive::new(decoder);
    for entry in tar.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let relative = sanitize_relative(&path)?;
        let target = dest.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(&target)?;
    }
    Ok(())
}

fn sanitize_relative(path: &Path) -> Result<PathBuf> {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            _ => return Err(AppError::validation("压缩包包含非法路径")),
        }
    }
    Ok(out)
}

/// 手动导入：把用户下载的官方包复制进缓存目录。
pub fn import_package(source: &Path, cache_dir: &Path) -> Result<PathBuf> {
    if !source.is_file() {
        return Err(AppError::validation(format!(
            "文件不存在: {}",
            source.display()
        )));
    }
    fs::create_dir_all(cache_dir)?;
    let name = source
        .file_name()
        .ok_or_else(|| AppError::validation("文件名不合法"))?;
    let target = cache_dir.join(name);
    if source != target {
        fs::copy(source, &target)?;
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_single_top_level_dir() {
        let paths = vec![
            PathBuf::from("mysql-8.0.40-winx64/bin/mysqld.exe"),
            PathBuf::from("mysql-8.0.40-winx64/share/english/errmsg.sys"),
        ];
        assert_eq!(
            common_root(&paths),
            Some(PathBuf::from("mysql-8.0.40-winx64"))
        );
    }

    #[test]
    fn keeps_flat_archive() {
        let paths = vec![
            PathBuf::from("redis-server.exe"),
            PathBuf::from("redis-cli.exe"),
        ];
        assert_eq!(common_root(&paths), None);
    }

    #[test]
    fn download_names_cannot_escape_cache() {
        assert!(safe_file_name("nginx-1.26.2.zip").is_some());
        assert!(safe_file_name("..").is_none());
        assert!(safe_file_name("../evil.zip").is_none());
        assert!(safe_file_name("a/b.zip").is_none());
        assert!(safe_file_name("a%2fb.zip").is_none());
    }

    #[test]
    fn rejects_absolute_tar_paths() {
        assert!(sanitize_relative(Path::new("C:/windows/system32/x")).is_err());
        assert!(sanitize_relative(Path::new("../x")).is_err());
        assert!(sanitize_relative(Path::new("a/b")).is_ok());
    }

    #[test]
    fn zip_roundtrip_blocks_traversal() {
        use std::io::Write;
        let dir = std::env::temp_dir().join(format!("svcbox-zip-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let zip_path = dir.join("bad.zip");
        {
            let file = File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            writer.start_file("../evil.txt", opts).unwrap();
            writer.write_all(b"x").unwrap();
            writer.finish().unwrap();
        }
        let out = extract_zip(&zip_path, &dir.join("out"));
        assert!(out.is_err());
        let _ = fs::remove_dir_all(dir);
    }
}
