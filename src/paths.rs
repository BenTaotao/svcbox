use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct Bootstrap {
    data_dir: Option<String>,
}

/// 运行数据目录。默认 `%APPDATA%\svcbox`，可用 --home / SVCBOX_HOME / 设置页覆盖。
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub home: PathBuf,
    pub db_file: PathBuf,
    pub versions: PathBuf,
    pub instances: PathBuf,
    pub cache: PathBuf,
    pub cache_packages: PathBuf,
    pub cache_acme: PathBuf,
    pub templates: PathBuf,
    pub logs: PathBuf,
    pub secret_key: PathBuf,
}

impl AppPaths {
    pub fn resolve(explicit_home: Option<PathBuf>) -> Result<Self> {
        let fallback = fallback_home();
        let configured = explicit_home
            .or_else(|| std::env::var_os("SVCBOX_HOME").map(PathBuf::from))
            .or_else(|| {
                read_bootstrap(&fallback)
                    .and_then(|b| b.data_dir)
                    .filter(|s| !s.trim().is_empty())
                    .map(PathBuf::from)
            });

        if let Some(home) = configured {
            return Self::from_home(home);
        }

        // 默认放在 D 盘，用户可在设置页/--home/环境变量中改成任意可写目录
        let preferred = default_home(&fallback);
        match Self::from_home(preferred.clone()) {
            Ok(paths) => Ok(paths),
            Err(e) if preferred != fallback => {
                tracing::warn!(
                    "默认数据目录 {} 不可用（{e}），回退到 {}",
                    preferred.display(),
                    fallback.display()
                );
                Self::from_home(fallback)
            }
            Err(e) => Err(e),
        }
    }

    fn from_home(home: PathBuf) -> Result<Self> {
        let paths = Self {
            db_file: home.join("svcbox.db"),
            versions: home.join("versions"),
            instances: home.join("instances"),
            cache: home.join("cache"),
            cache_packages: home.join("cache").join("packages"),
            cache_acme: home.join("cache").join("acme"),
            templates: home.join("templates"),
            logs: home.join("logs"),
            secret_key: home.join("secret.key"),
            home,
        };
        paths.ensure()?;
        Ok(paths)
    }

    pub fn ensure(&self) -> Result<()> {
        for dir in [
            &self.home,
            &self.versions,
            &self.instances,
            &self.cache,
            &self.cache_packages,
            &self.cache_acme,
            &self.templates,
            &self.logs,
        ] {
            fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    pub fn bootstrap_file() -> Option<PathBuf> {
        dirs::data_dir().map(|d| d.join("svcbox").join("bootstrap.json"))
    }

    /// 数据目录切换在下次启动生效，避免运行中数据库句柄失效。
    pub fn set_data_dir(new_home: Option<&Path>) -> Result<()> {
        let Some(file) = Self::bootstrap_file() else {
            return Err(crate::error::AppError::other("无法定位系统数据目录"));
        };
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }
        let bootstrap = Bootstrap {
            data_dir: new_home.map(|p| p.to_string_lossy().to_string()),
        };
        fs::write(file, serde_json::to_vec_pretty(&bootstrap)?)?;
        Ok(())
    }
}

fn fallback_home() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("svcbox")
}

/// 默认 D:\BC\svcbox；没有 D 盘或不可写时回退到用户数据目录。
fn default_home(fallback: &Path) -> PathBuf {
    if cfg!(windows) && Path::new("D:\\").is_dir() {
        return PathBuf::from(r"D:\BC\svcbox");
    }
    fallback.to_path_buf()
}

fn read_bootstrap(base: &Path) -> Option<Bootstrap> {
    let file = base.join("bootstrap.json");
    let data = fs::read(file).ok()?;
    serde_json::from_slice(&data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_home_prefers_d_drive_when_present() {
        let fallback = PathBuf::from(r"C:\fallback\svcbox");
        let home = default_home(&fallback);
        if cfg!(windows) && Path::new("D:\\").is_dir() {
            assert_eq!(home, PathBuf::from(r"D:\BC\svcbox"));
        } else {
            assert_eq!(home, fallback);
        }
    }
}
