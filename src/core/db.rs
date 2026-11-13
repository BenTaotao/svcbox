use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use crate::error::{AppError, Result};
use crate::models::{
    CertificateRecord, InstanceConfig, InstanceRecord, InstanceStatus, ServiceKind, Settings,
    VersionRecord,
};

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.migrate()?;
        Ok(db)
    }

    fn conn(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| AppError::other("数据库连接锁已损坏"))
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn()?;
        conn.execute_batch(
            r#"
CREATE TABLE IF NOT EXISTS versions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    version TEXT NOT NULL,
    platform TEXT NOT NULL,
    install_path TEXT NOT NULL,
    source_url TEXT NOT NULL,
    sha256 TEXT NOT NULL,
    installed_at TEXT NOT NULL,
    UNIQUE(kind, version, platform)
);

CREATE TABLE IF NOT EXISTS instances (
    id TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    port INTEGER NOT NULL,
    dir TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'stopped',
    config_json TEXT NOT NULL DEFAULT '{}',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS certificates (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    instance_id TEXT NOT NULL,
    domain TEXT NOT NULL,
    cert_path TEXT NOT NULL,
    key_path TEXT NOT NULL,
    issued_at TEXT NOT NULL,
    expires_at TEXT NOT NULL,
    UNIQUE(instance_id, domain)
);

CREATE INDEX IF NOT EXISTS idx_instances_kind ON instances(kind);
CREATE INDEX IF NOT EXISTS idx_certificates_instance ON certificates(instance_id);
"#,
        )?;
        Ok(())
    }

    pub fn load_settings(&self) -> Result<Settings> {
        let conn = self.conn()?;
        let raw: Option<String> = conn
            .query_row(
                "SELECT value FROM settings WHERE key = 'settings'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        Ok(raw
            .and_then(|s| serde_json::from_str::<Settings>(&s).ok())
            .unwrap_or_default()
            .normalized())
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<()> {
        let value = serde_json::to_string(settings)?;
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO settings(key, value) VALUES('settings', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![value],
        )?;
        Ok(())
    }

    pub fn upsert_version(&self, record: &VersionRecord) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO versions(kind, version, platform, install_path, source_url, sha256, installed_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(kind, version, platform) DO UPDATE SET
                install_path = excluded.install_path,
                source_url = excluded.source_url,
                sha256 = excluded.sha256,
                installed_at = excluded.installed_at",
            params![
                record.kind.as_str(),
                record.version,
                record.platform,
                record.install_path,
                record.source_url,
                record.sha256,
                record.installed_at
            ],
        )?;
        Ok(())
    }

    pub fn list_versions(&self) -> Result<Vec<VersionRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT kind, version, platform, install_path, source_url, sha256, installed_at
             FROM versions ORDER BY kind, version DESC",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (kind, version, platform, install_path, source_url, sha256, installed_at) = row?;
            out.push(VersionRecord {
                kind: kind.parse()?,
                version,
                platform,
                install_path,
                source_url,
                sha256,
                installed_at,
            });
        }
        Ok(out)
    }

    pub fn get_version(&self, kind: ServiceKind, version: &str) -> Result<Option<VersionRecord>> {
        let versions = self.list_versions()?;
        Ok(versions
            .into_iter()
            .find(|v| v.kind == kind && v.version == version))
    }

    pub fn remove_version(&self, kind: ServiceKind, version: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM versions WHERE kind = ?1 AND version = ?2",
            params![kind.as_str(), version],
        )?;
        Ok(())
    }

    pub fn insert_instance(&self, rec: &InstanceRecord) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO instances(id, kind, name, version, port, dir, status, config_json, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                rec.id,
                rec.kind.as_str(),
                rec.name,
                rec.version,
                rec.port,
                rec.dir.to_string_lossy(),
                rec.status.as_str(),
                serde_json::to_string(&rec.config)?,
                rec.created_at,
                rec.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn update_instance(&self, rec: &InstanceRecord) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE instances SET port = ?2, status = ?3, config_json = ?4, updated_at = ?5 WHERE id = ?1",
            params![
                rec.id,
                rec.port,
                rec.status.as_str(),
                serde_json::to_string(&rec.config)?,
                rec.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn update_instance_status(&self, id: &str, status: InstanceStatus) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "UPDATE instances SET status = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, status.as_str(), crate::utils::now_rfc3339()],
        )?;
        Ok(())
    }

    pub fn list_instances(&self) -> Result<Vec<InstanceRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, kind, name, version, port, dir, status, config_json, created_at, updated_at
             FROM instances ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, String>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, String>(8)?,
                r.get::<_, String>(9)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, kind, name, version, port, dir, status, config_json, created_at, updated_at) =
                row?;
            let config: InstanceConfig = serde_json::from_str(&config_json)
                .map_err(|e| AppError::other(format!("实例 {id} 配置损坏: {e}")))?;
            out.push(InstanceRecord {
                id,
                kind: kind.parse()?,
                name,
                version,
                port: port as u16,
                dir: dir.into(),
                status: InstanceStatus::parse(&status),
                config,
                created_at,
                updated_at,
            });
        }
        Ok(out)
    }

    pub fn get_instance(&self, id: &str) -> Result<Option<InstanceRecord>> {
        Ok(self.list_instances()?.into_iter().find(|i| i.id == id))
    }

    pub fn delete_instance(&self, id: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM certificates WHERE instance_id = ?1",
            params![id],
        )?;
        conn.execute("DELETE FROM instances WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn upsert_certificate(&self, rec: &CertificateRecord) -> Result<i64> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT INTO certificates(instance_id, domain, cert_path, key_path, issued_at, expires_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(instance_id, domain) DO UPDATE SET
                cert_path = excluded.cert_path,
                key_path = excluded.key_path,
                issued_at = excluded.issued_at,
                expires_at = excluded.expires_at",
            params![
                rec.instance_id,
                rec.domain,
                rec.cert_path,
                rec.key_path,
                rec.issued_at,
                rec.expires_at
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    pub fn list_certificates(&self) -> Result<Vec<CertificateRecord>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, instance_id, domain, cert_path, key_path, issued_at, expires_at
             FROM certificates ORDER BY expires_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(CertificateRecord {
                id: r.get(0)?,
                instance_id: r.get(1)?,
                domain: r.get(2)?,
                cert_path: r.get(3)?,
                key_path: r.get(4)?,
                issued_at: r.get(5)?,
                expires_at: r.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_certificate(
        &self,
        instance_id: &str,
        domain: &str,
    ) -> Result<Option<CertificateRecord>> {
        Ok(self
            .list_certificates()?
            .into_iter()
            .find(|c| c.instance_id == instance_id && c.domain == domain))
    }

    pub fn delete_certificate(&self, instance_id: &str, domain: &str) -> Result<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM certificates WHERE instance_id = ?1 AND domain = ?2",
            params![instance_id, domain],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::InstanceConfig;

    #[test]
    fn settings_and_instance_roundtrip() {
        let path = std::env::temp_dir().join(format!("svcbox-db-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let db = Db::open(&path).unwrap();

        let mut settings = db.load_settings().unwrap();
        settings.http_api_port = 9191;
        db.save_settings(&settings).unwrap();
        assert_eq!(db.load_settings().unwrap().http_api_port, 9191);

        let rec = InstanceRecord {
            id: "dev".into(),
            kind: ServiceKind::Nginx,
            name: "dev".into(),
            version: "1.26.2".into(),
            port: 8080,
            dir: std::env::temp_dir().join("svcbox-dev"),
            status: InstanceStatus::Stopped,
            config: InstanceConfig::for_kind(ServiceKind::Nginx, 8080),
            created_at: crate::utils::now_rfc3339(),
            updated_at: crate::utils::now_rfc3339(),
        };
        db.insert_instance(&rec).unwrap();
        assert_eq!(db.list_instances().unwrap().len(), 1);
        db.delete_instance("dev").unwrap();
        assert!(db.list_instances().unwrap().is_empty());

        drop(db);
        let _ = std::fs::remove_file(path);
    }
}
