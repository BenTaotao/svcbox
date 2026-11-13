use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use crate::error::{AppError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceKind {
    Nginx,
    Mysql,
    Redis,
}

impl ServiceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ServiceKind::Nginx => "nginx",
            ServiceKind::Mysql => "mysql",
            ServiceKind::Redis => "redis",
        }
    }

    pub fn default_port(self) -> u16 {
        match self {
            ServiceKind::Nginx => 8080,
            ServiceKind::Mysql => 3306,
            ServiceKind::Redis => 6379,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ServiceKind::Nginx => "Nginx",
            ServiceKind::Mysql => "MySQL",
            ServiceKind::Redis => "Redis",
        }
    }
}

impl fmt::Display for ServiceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ServiceKind {
    type Err = AppError;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "nginx" => Ok(Self::Nginx),
            "mysql" => Ok(Self::Mysql),
            "redis" => Ok(Self::Redis),
            _ => Err(AppError::validation(format!(
                "未知服务类型 {s}，仅支持 nginx/mysql/redis"
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstanceStatus {
    Stopped,
    Running,
    Crashed,
}

impl InstanceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            InstanceStatus::Stopped => "stopped",
            InstanceStatus::Running => "running",
            InstanceStatus::Crashed => "crashed",
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "crashed" => Self::Crashed,
            _ => Self::Stopped,
        }
    }
}

/// 三重健康检测结果，前端只消费这一个枚举。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HealthStatus {
    Stopped,
    Running,
    WarningPortNotListen,
    ErrorLogHasError,
    Crashed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    pub status: HealthStatus,
    pub pid: Option<u32>,
    pub port_open: bool,
    pub log_has_error: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NginxGlobalConfig {
    pub worker_processes: u32,
    pub worker_connections: u32,
    pub keepalive_timeout: u32,
    pub gzip: bool,
    pub client_max_body_size: String,
    pub https_port: u16,
}

impl Default for NginxGlobalConfig {
    fn default() -> Self {
        Self {
            worker_processes: 1,
            worker_connections: 1024,
            keepalive_timeout: 65,
            gzip: true,
            client_max_body_size: "64m".to_string(),
            https_port: 8443,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedisConfig {
    pub bind: String,
    pub port: u16,
    /// AES-GCM 密文，落库不存明文
    #[serde(default)]
    pub requirepass_enc: Option<String>,
    pub appendonly: bool,
}

impl Default for RedisConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".to_string(),
            port: 6379,
            requirepass_enc: None,
            appendonly: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct InstanceConfig {
    pub nginx: NginxGlobalConfig,
    pub redis: RedisConfig,
    /// MySQL root@localhost 密码密文
    pub root_password_enc: Option<String>,
    pub mysql_initialized: bool,
}

impl InstanceConfig {
    pub fn for_kind(kind: ServiceKind, port: u16) -> Self {
        let mut cfg = Self::default();
        match kind {
            ServiceKind::Nginx => {}
            ServiceKind::Mysql => {}
            ServiceKind::Redis => {
                cfg.redis.port = port;
            }
        }
        cfg
    }
}

#[derive(Debug, Clone)]
pub struct InstanceRecord {
    pub id: String,
    pub kind: ServiceKind,
    pub name: String,
    pub version: String,
    pub port: u16,
    pub dir: PathBuf,
    pub status: InstanceStatus,
    pub config: InstanceConfig,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstanceView {
    pub id: String,
    pub kind: ServiceKind,
    pub name: String,
    pub version: String,
    pub port: u16,
    pub https_port: u16,
    pub status: InstanceStatus,
    pub health: HealthStatus,
    pub pid: Option<u32>,
    pub dir: String,
    pub urls: Vec<String>,
    pub has_root_password: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInstanceResult {
    pub instance: InstanceView,
    /// 仅 MySQL 创建时返回一次，之后无法再取回明文
    pub initial_root_password: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateInstanceOptions {
    pub kind: ServiceKind,
    pub name: String,
    pub version: String,
    pub port: Option<u16>,
    pub root_password: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SiteConfig {
    pub server_name: String,
    pub root: String,
    #[serde(default = "default_index")]
    pub index: String,
    #[serde(default)]
    pub custom_locations: String,
    #[serde(default)]
    pub https: bool,
    #[serde(default)]
    pub cert_domain: Option<String>,
}

fn default_index() -> String {
    "index.html index.htm".to_string()
}

impl Default for SiteConfig {
    fn default() -> Self {
        Self {
            server_name: String::new(),
            root: String::new(),
            index: default_index(),
            custom_locations: String::new(),
            https: false,
            cert_domain: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    pub kind: ServiceKind,
    pub version: String,
    pub platform: String,
    pub download_url: String,
    #[serde(default)]
    pub mirror_url: Option<String>,
    #[serde(default)]
    pub sha256: String,
    pub archive_type: String,
    pub bin_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub kind: ServiceKind,
    pub version: String,
    pub platform: String,
    pub installed: bool,
    pub installed_path: Option<String>,
    pub download_url: String,
    pub mirror_url: Option<String>,
    pub sha256: String,
    pub bin_path: String,
    pub archive_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionRecord {
    pub kind: ServiceKind,
    pub version: String,
    pub platform: String,
    pub install_path: String,
    pub source_url: String,
    pub sha256: String,
    pub installed_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub data_dir: Option<String>,
    pub manifest_url: Option<String>,
    pub download_source: DownloadSource,
    pub stop_instances_on_close: bool,
    pub autostart: bool,
    pub http_api_enabled: bool,
    pub http_api_port: u16,
    pub log_level: String,
}

impl Settings {
    pub fn normalized(mut self) -> Self {
        if self.http_api_port == 0 {
            self.http_api_port = 9090;
        }
        if self.log_level.is_empty() {
            self.log_level = "info".to_string();
        }
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadSource {
    #[default]
    Official,
    Mirror,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub kind: String,
    pub version: String,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub percent: f64,
    pub stage: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateInfo {
    pub domain: String,
    pub instance_id: String,
    pub instance_name: String,
    pub cert_path: String,
    pub key_path: String,
    pub issued_at: String,
    pub expires_at: String,
    pub days_left: i64,
}

#[derive(Debug, Clone)]
pub struct CertificateRecord {
    pub id: i64,
    pub instance_id: String,
    pub domain: String,
    pub cert_path: String,
    pub key_path: String,
    pub issued_at: String,
    pub expires_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsResult {
    pub settings: Settings,
    pub restart_required: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorCheck {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DoctorReport {
    pub ok: bool,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogChunk {
    pub path: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedisConfigInput {
    pub bind: String,
    pub port: u16,
    /// None=保持原密码，Some("")=清除密码
    pub requirepass: Option<String>,
    pub appendonly: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MysqlUser {
    pub user: String,
    pub host: String,
    pub dbs: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedisConfigView {
    pub bind: String,
    pub port: u16,
    pub has_password: bool,
    pub appendonly: bool,
    #[serde(default)]
    pub requires_restart: bool,
}
