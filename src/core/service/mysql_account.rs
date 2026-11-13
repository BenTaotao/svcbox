use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::{instance, AppContext};
use crate::error::{AppError, Result};
use crate::models::{InstanceRecord, MysqlUser};
use crate::utils;

pub const SAFE_PRIVILEGES: &str = "SELECT,INSERT,UPDATE,DELETE,CREATE,DROP,ALTER,INDEX";

const SYSTEM_DBS: &[&str] = &["mysql", "sys", "information_schema", "performance_schema"];
const SYSTEM_USERS: &[&str] = &["root", "mysql.sys", "mysql.session", "mysql.infoschema"];

pub struct MySqlSession {
    mysql: PathBuf,
    mysqladmin: PathBuf,
    conn_args: Vec<String>,
    password: Option<String>,
    work_dir: PathBuf,
}

impl MySqlSession {
    pub fn from_instance(ctx: &AppContext, rec: &InstanceRecord) -> Result<Self> {
        let enc = rec
            .config
            .root_password_enc
            .as_ref()
            .ok_or_else(|| AppError::conflict("实例尚未设置 root 密码，请先完成初始化"))?;
        let password = ctx.cipher.decrypt(enc)?;
        Self::build(ctx, rec, Some(password))
    }

    pub fn without_password(ctx: &AppContext, rec: &InstanceRecord) -> Result<Self> {
        Self::build(ctx, rec, None)
    }

    pub fn with_password(ctx: &AppContext, rec: &InstanceRecord, password: &str) -> Result<Self> {
        Self::build(ctx, rec, Some(password.to_string()))
    }

    fn build(ctx: &AppContext, rec: &InstanceRecord, password: Option<String>) -> Result<Self> {
        Ok(Self {
            mysql: instance::mysql_client_exe(ctx, rec)?,
            mysqladmin: instance::mysqladmin_exe(ctx, rec)?,
            conn_args: connection_args(rec),
            password,
            work_dir: rec.dir.join("run"),
        })
    }

    fn option_file(&self) -> Result<PathBuf> {
        fs::create_dir_all(&self.work_dir)?;
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = self
            .work_dir
            .join(format!(".client-{}-{nanos}.cnf", std::process::id()));
        let mut content = String::from("[client]\nuser=root\n");
        if let Some(pwd) = &self.password {
            content.push_str(&format!("password=\"{}\"\n", escape_option_value(pwd)));
        }
        fs::write(&path, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
        }
        Ok(path)
    }

    fn base_args(&self) -> Result<Vec<String>> {
        let opt = self.option_file()?;
        let mut args = vec![format!("--defaults-extra-file={}", utils::forward(&opt))];
        args.extend(self.conn_args.iter().cloned());
        Ok(args)
    }

    fn cleanup_option(&self, args: &[String]) {
        if let Some(first) = args.first() {
            if let Some(path) = first.strip_prefix("--defaults-extra-file=") {
                let _ = fs::remove_file(path);
            }
        }
    }

    /// 执行 SQL，返回制表符分隔的原始输出。
    pub fn execute(&self, sql: &str) -> Result<String> {
        let mut args = self.base_args()?;
        args.extend([
            "--batch".to_string(),
            "--skip-column-names".to_string(),
            "--default-character-set=utf8mb4".to_string(),
            "--connect-timeout=10".to_string(),
            "-e".to_string(),
            sql.to_string(),
        ]);
        let out = utils::run_tool(&self.mysql, &args, Some(&self.work_dir), &[]);
        self.cleanup_option(&args);
        utils::output_ok(&self.mysql, out?)
    }

    pub fn ping(&self) -> Result<()> {
        let mut args = self.base_args()?;
        args.extend(["--connect-timeout=3".to_string(), "ping".to_string()]);
        let out = utils::run_tool(&self.mysqladmin, &args, Some(&self.work_dir), &[]);
        self.cleanup_option(&args);
        utils::output_ok(&self.mysqladmin, out?).map(|_| ())
    }

    pub fn try_ping(&self) -> bool {
        self.ping().is_ok()
    }

    pub fn shutdown(&self) -> Result<()> {
        let mut args = self.base_args()?;
        args.push("shutdown".to_string());
        let out = utils::run_tool(&self.mysqladmin, &args, Some(&self.work_dir), &[]);
        self.cleanup_option(&args);
        utils::output_ok(&self.mysqladmin, out?).map(|_| ())
    }

    pub fn databases(&self) -> Result<Vec<String>> {
        let out = self.execute("SHOW DATABASES")?;
        let mut dbs: Vec<String> = out
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !SYSTEM_DBS.contains(&l.as_str()))
            .collect();
        dbs.sort();
        Ok(dbs)
    }

    pub fn users(&self) -> Result<Vec<MysqlUser>> {
        let users_raw = self.execute(
            "SELECT user, host FROM mysql.user \
             WHERE user NOT IN ('root','mysql.sys','mysql.session','mysql.infoschema') \
             ORDER BY user, host",
        )?;
        let grants_raw =
            self.execute("SELECT user, host, db FROM mysql.db ORDER BY user, host, db")?;
        let mut grants: std::collections::HashMap<(String, String), Vec<String>> =
            std::collections::HashMap::new();
        for line in grants_raw.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 3 {
                continue;
            }
            grants
                .entry((cols[0].to_string(), cols[1].to_string()))
                .or_default()
                .push(cols[2].to_string());
        }
        let mut out = Vec::new();
        for line in users_raw.lines() {
            let cols: Vec<&str> = line.split('\t').collect();
            if cols.len() < 2 || SYSTEM_USERS.contains(&cols[0]) {
                continue;
            }
            let key = (cols[0].to_string(), cols[1].to_string());
            let dbs = grants.remove(&key).unwrap_or_default();
            out.push(MysqlUser {
                user: key.0,
                host: key.1,
                dbs,
            });
        }
        Ok(out)
    }

    pub fn create_database(&self, name: &str, charset: &str, collate: &str) -> Result<()> {
        utils::validate_db_name(name)?;
        validate_word(charset, "字符集")?;
        validate_word(collate, "排序规则")?;
        self.execute(&format!(
            "CREATE DATABASE `{}` CHARACTER SET {} COLLATE {};",
            name, charset, collate
        ))
        .map(|_| ())
    }

    pub fn drop_database(&self, name: &str) -> Result<()> {
        utils::validate_db_name(name)?;
        if SYSTEM_DBS.contains(&name) {
            return Err(AppError::validation("不能删除系统库"));
        }
        self.execute(&format!("DROP DATABASE IF EXISTS `{}`;", name))
            .map(|_| ())
    }

    pub fn create_user(
        &self,
        user: &str,
        host: &str,
        password: &str,
        dbs: &[String],
    ) -> Result<()> {
        utils::validate_db_user(user)?;
        utils::validate_db_host(host)?;
        utils::validate_password(password)?;
        if user == "root" {
            return Err(AppError::validation(
                "不允许通过该接口创建 root 账号，请使用 root 密码管理功能",
            ));
        }
        for db in dbs {
            utils::validate_db_name(db)?;
        }
        let account = format!("'{}'@'{}'", sql_string(user), sql_string(host));
        self.execute(&format!(
            "CREATE USER {account} IDENTIFIED BY '{}';",
            sql_string(password)
        ))?;
        for db in dbs {
            let grant = format!("GRANT {SAFE_PRIVILEGES} ON `{}`.* TO {account};", db);
            if let Err(e) = self.execute(&grant) {
                let _ = self.execute(&format!("DROP USER IF EXISTS {account};"));
                return Err(e);
            }
        }
        self.execute("FLUSH PRIVILEGES;").map(|_| ())
    }

    pub fn alter_user_password(&self, user: &str, host: &str, password: &str) -> Result<()> {
        utils::validate_db_user(user)?;
        utils::validate_db_host(host)?;
        utils::validate_password(password)?;
        if user == "root" {
            return Err(AppError::validation(
                "root 密码请使用「重置 root 密码」功能修改",
            ));
        }
        self.execute(&format!(
            "ALTER USER '{}'@'{}' IDENTIFIED BY '{}';",
            sql_string(user),
            sql_string(host),
            sql_string(password)
        ))
        .map(|_| ())
    }

    pub fn drop_user(&self, user: &str, host: &str) -> Result<()> {
        utils::validate_db_user(user)?;
        utils::validate_db_host(host)?;
        if user == "root" {
            return Err(AppError::validation("不允许通过该接口删除 root 账号"));
        }
        self.execute(&format!(
            "DROP USER IF EXISTS '{}'@'{}';",
            sql_string(user),
            sql_string(host)
        ))
        .map(|_| ())
    }
}

/// MySQL 客户端与 root@localhost 的连接方式：Windows 共享内存、Unix socket。
/// 避免依赖主机名解析，也避免 127.0.0.1 与 localhost 的授权差异。
pub fn connection_args(rec: &InstanceRecord) -> Vec<String> {
    if cfg!(windows) {
        vec![
            "--protocol=MEMORY".to_string(),
            format!(
                "--shared-memory-base-name={}",
                instance::shared_memory_name(&rec.id)
            ),
        ]
    } else {
        vec![
            "--protocol=SOCKET".to_string(),
            format!(
                "--socket={}",
                utils::forward(&rec.dir.join("run").join("mysql.sock"))
            ),
        ]
    }
}

pub fn escape_option_value(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

pub fn sql_string(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\0', "")
}

fn validate_word(value: &str, label: &str) -> Result<()> {
    let ok = !value.is_empty()
        && value.len() <= 64
        && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(AppError::validation(format!("{label}不合法")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sql_string_escapes_quotes() {
        assert_eq!(sql_string("a'b\\c"), "a\\'b\\\\c");
        assert_eq!(sql_string("plain"), "plain");
    }

    #[test]
    fn option_value_escapes_quotes() {
        assert_eq!(escape_option_value("p\"w\\d"), "p\\\"w\\\\d");
    }
}
