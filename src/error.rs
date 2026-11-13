use serde::ser::{Serialize, Serializer};

/// 内核统一错误。所有对外命令都返回它，GUI/CLI/HTTP 只需展示 message。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("I/O 错误: {0}")]
    Io(#[from] std::io::Error),

    #[error("数据库错误: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("网络请求失败: {0}")]
    Http(#[from] reqwest::Error),

    #[error("模板渲染失败: {0}")]
    Template(#[from] handlebars::RenderError),

    #[error("JSON 解析失败: {0}")]
    Json(#[from] serde_json::Error),

    #[error("压缩包处理失败: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("参数不合法: {0}")]
    Validation(String),

    #[error("对象不存在: {0}")]
    NotFound(String),

    #[error("当前状态不允许该操作: {0}")]
    Conflict(String),

    #[error("[{tool}] 执行失败 (exit={code}): {stderr}")]
    External {
        tool: String,
        code: i32,
        stderr: String,
    },

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, AppError>;

impl AppError {
    pub fn validation(msg: impl Into<String>) -> Self {
        Self::Validation(msg.into())
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    pub fn conflict(msg: impl Into<String>) -> Self {
        Self::Conflict(msg.into())
    }

    pub fn other(msg: impl Into<String>) -> Self {
        Self::Other(msg.into())
    }
}

impl Serialize for AppError {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}
