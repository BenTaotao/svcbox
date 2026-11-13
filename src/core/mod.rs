pub mod cert;
pub mod config;
pub mod db;
pub mod doctor;
pub mod download;
pub mod health;
pub mod instance;
pub mod process;
pub mod service;
pub mod supervisor;
pub mod version;

use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{AppError, Result};
use crate::models::{InstanceRecord, InstanceStatus, ServiceKind, Settings, SettingsResult};
use crate::paths::AppPaths;
use crate::secret::SecretCipher;
use crate::utils;

use self::db::Db;
use self::download::ProgressCallback;
use self::process::ProcessManager;

/// 内核事件出口：GUI 用 Tauri 事件实现，CLI 用空实现。
pub trait EventSink: Send + Sync {
    fn emit(&self, event: &str, payload: Value);
    fn notify(&self, title: &str, body: &str);
}

pub struct NoopSink;

impl EventSink for NoopSink {
    fn emit(&self, _event: &str, _payload: Value) {}
    fn notify(&self, _title: &str, _body: &str) {}
}

pub struct AppContext {
    pub paths: AppPaths,
    pub db: Db,
    pub cipher: SecretCipher,
    pub processes: ProcessManager,
    pub events: Arc<dyn EventSink>,
    pub install_lock: tokio::sync::Mutex<()>,
}

impl AppContext {
    pub fn new(paths: AppPaths, events: Arc<dyn EventSink>) -> Result<Arc<Self>> {
        let db = Db::open(&paths.db_file)?;
        let cipher = SecretCipher::load_or_create(&paths.secret_key)?;
        Ok(Arc::new(Self {
            paths,
            db,
            cipher,
            processes: ProcessManager::new(),
            events,
            install_lock: tokio::sync::Mutex::new(()),
        }))
    }

    pub fn emit(&self, event: &str, payload: Value) {
        self.events.emit(event, payload);
    }

    pub fn notify(&self, title: &str, body: &str) {
        self.events.notify(title, body);
    }

    pub fn settings(&self) -> Result<Settings> {
        self.db.load_settings()
    }

    /// 设置保存。数据目录、HTTP API 端口、日志级别的变更需要重启生效。
    pub fn update_settings(&self, mut settings: Settings) -> Result<SettingsResult> {
        let current = self.settings()?;
        settings = settings.normalized();
        if settings.http_api_port == 0 {
            return Err(AppError::validation("HTTP API 端口不合法"));
        }
        if let Some(url) = settings.manifest_url.as_mut() {
            if url.trim().is_empty() {
                settings.manifest_url = None;
            } else if !url.starts_with("https://") {
                return Err(AppError::validation("版本清单地址必须使用 HTTPS"));
            }
        }
        if settings
            .data_dir
            .as_deref()
            .map(|s| s.trim().is_empty())
            .unwrap_or(false)
        {
            settings.data_dir = None;
        }

        let mut restart_required = false;
        if let Some(dir) = &settings.data_dir {
            let path = PathBuf::from(dir);
            if !path.is_absolute() {
                return Err(AppError::validation("数据目录必须是绝对路径"));
            }
            std::fs::create_dir_all(&path)?;
            let probe = path.join(format!(".probe-{}", std::process::id()));
            std::fs::write(&probe, b"ok")?;
            std::fs::remove_file(&probe)?;
            if path != self.paths.home {
                AppPaths::set_data_dir(Some(&path))?;
                restart_required = true;
            }
        } else if current.data_dir.is_some() {
            AppPaths::set_data_dir(None)?;
            restart_required = true;
        }
        if settings.http_api_enabled != current.http_api_enabled
            || settings.http_api_port != current.http_api_port
        {
            restart_required = true;
        }
        if settings.log_level != current.log_level {
            restart_required = true;
        }
        self.db.save_settings(&settings)?;
        self.emit(
            "settings-changed",
            serde_json::to_value(&settings).unwrap_or(Value::Null),
        );
        Ok(SettingsResult {
            settings,
            restart_required,
        })
    }

    pub fn progress_callback(&self) -> ProgressCallback {
        let events = self.events.clone();
        Arc::new(move |progress| {
            if let Ok(value) = serde_json::to_value(progress) {
                events.emit("download-progress", value);
            }
        })
    }

    pub fn require_instance(&self, id: &str) -> Result<InstanceRecord> {
        self.db
            .get_instance(id)?
            .ok_or_else(|| AppError::not_found(format!("实例 {id} 不存在")))
    }

    /// GUI 启动时校正状态：CLI 守护进程管理的实例保持运行，其余残留标记为停止并清理 pid。
    pub fn reconcile(&self) -> Result<()> {
        for rec in self.db.list_instances()? {
            supervisor::clear_stale_pid(&rec);
            if rec.kind == ServiceKind::Mysql {
                // 兼容旧实例：移除会导致 127.0.0.1 连接被拒的 skip-name-resolve
                let _ = service::mysql::repair_legacy_host_resolution(&rec);
            }
            let externally_managed = instance::is_running_sync(&rec);
            if externally_managed {
                if rec.status != InstanceStatus::Running {
                    self.db
                        .update_instance_status(&rec.id, InstanceStatus::Running)?;
                }
                continue;
            }
            if rec.status != InstanceStatus::Stopped {
                self.db
                    .update_instance_status(&rec.id, InstanceStatus::Stopped)?;
            }
            let run_dir = rec.dir.join("run");
            if run_dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&run_dir) {
                    for entry in entries.flatten() {
                        if entry.path().is_file() {
                            let _ = std::fs::remove_file(entry.path());
                        }
                    }
                }
            }
        }
        utils::ensure_dir(&self.paths.logs)?;
        Ok(())
    }
}
