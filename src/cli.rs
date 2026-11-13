use clap::{Parser, Subcommand};
use serde_json::Value;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::core::{cert, doctor, instance, service, version, AppContext, EventSink};
use crate::error::{AppError, Result};
use crate::models::{
    DownloadProgress, HealthStatus, InstanceView, RedisConfigInput, ServiceKind, SiteConfig,
};
use crate::paths::AppPaths;
use crate::utils;

#[derive(Parser, Debug)]
#[command(
    name = "svcbox",
    version,
    about = "本地开发环境 Web 服务管理器（Nginx / MySQL / Redis）"
)]
struct Cli {
    /// 自定义数据目录（默认 %APPDATA%\svcbox）
    #[arg(long, global = true, value_name = "DIR")]
    home: Option<PathBuf>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// 版本仓库管理
    Version {
        #[command(subcommand)]
        command: VersionCmd,
    },
    /// 实例生命周期
    Instance {
        #[command(subcommand)]
        command: InstanceCmd,
    },
    /// 查看实例日志，-f 持续跟随
    Logs {
        instance: String,
        #[arg(short, long)]
        follow: bool,
        #[arg(long, default_value_t = 200)]
        lines: usize,
    },
    /// 环境自检
    Doctor,
    /// Nginx 站点与 HTTPS
    Nginx {
        #[command(subcommand)]
        command: NginxCmd,
    },
    /// MySQL 库与账号
    Mysql {
        #[command(subcommand)]
        command: MysqlCmd,
    },
    /// Redis 配置
    Redis {
        #[command(subcommand)]
        command: RedisCmd,
    },
    /// 证书管理
    Cert {
        #[command(subcommand)]
        command: CertCmd,
    },
    /// 可选 HTTP API
    Api {
        #[command(subcommand)]
        command: ApiCmd,
    },
    /// 内部使用：CLI 启动的常驻守护进程
    #[command(hide = true, name = "__supervise")]
    Supervise { instance: String },
}

#[derive(Subcommand, Debug)]
enum VersionCmd {
    List,
    Install {
        kind: String,
        version: String,
    },
    Remove {
        kind: String,
        version: String,
    },
    Import {
        kind: String,
        version: String,
        file: PathBuf,
        #[arg(long)]
        sha256: Option<String>,
    },
    Refresh {
        #[arg(long)]
        url: Option<String>,
    },
    /// 清理不在当前清单、且没有实例引用的旧版本
    Prune,
}

#[derive(Subcommand, Debug)]
enum InstanceCmd {
    Create {
        kind: String,
        name: String,
        #[arg(long)]
        version: String,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        root_password: Option<String>,
    },
    Start {
        name: String,
    },
    Stop {
        name: String,
    },
    Restart {
        name: String,
    },
    List,
    Remove {
        name: String,
        #[arg(long)]
        keep_data: bool,
    },
    Config {
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum NginxCmd {
    Site {
        #[command(subcommand)]
        command: NginxSiteCmd,
    },
    Global {
        instance: String,
        #[arg(long, default_value_t = 1)]
        worker_processes: u32,
        #[arg(long, default_value_t = 1024)]
        worker_connections: u32,
        #[arg(long, default_value_t = 65)]
        keepalive_timeout: u32,
        #[arg(long, default_value = "64m")]
        client_max_body_size: String,
        #[arg(long)]
        gzip: Option<bool>,
        #[arg(long, default_value_t = 8443)]
        https_port: u16,
    },
    Reload {
        instance: String,
    },
    Https {
        #[command(subcommand)]
        command: NginxHttpsCmd,
    },
    Cert {
        #[command(subcommand)]
        command: NginxCertCmd,
    },
}

#[derive(Subcommand, Debug)]
enum NginxSiteCmd {
    Create {
        instance: String,
        domain: String,
        #[arg(long)]
        root: String,
        #[arg(long, default_value = "index.html index.htm")]
        index: String,
        #[arg(long, default_value = "")]
        custom_locations: String,
        #[arg(long)]
        https: bool,
        #[arg(long)]
        cert_domain: Option<String>,
    },
    List {
        instance: String,
    },
    Delete {
        instance: String,
        domain: String,
    },
}

#[derive(Subcommand, Debug)]
enum NginxHttpsCmd {
    Apply { instance: String, domain: String },
}

#[derive(Subcommand, Debug)]
enum NginxCertCmd {
    Renew { instance: String, domain: String },
}

#[derive(Subcommand, Debug)]
enum MysqlCmd {
    Db {
        #[command(subcommand)]
        command: MysqlDbCmd,
    },
    User {
        #[command(subcommand)]
        command: MysqlUserCmd,
    },
    RootPass {
        instance: String,
        password: String,
    },
}

#[derive(Subcommand, Debug)]
enum MysqlDbCmd {
    Create {
        instance: String,
        name: String,
        #[arg(long)]
        charset: Option<String>,
        #[arg(long)]
        collate: Option<String>,
    },
    List {
        instance: String,
    },
    Drop {
        instance: String,
        name: String,
    },
}

#[derive(Subcommand, Debug)]
enum MysqlUserCmd {
    Create {
        instance: String,
        user: String,
        host: String,
        password: String,
        #[arg(long = "db")]
        dbs: Vec<String>,
    },
    List {
        instance: String,
    },
    Pass {
        instance: String,
        user: String,
        host: String,
        password: String,
    },
    Drop {
        instance: String,
        user: String,
        host: String,
    },
}

#[derive(Subcommand, Debug)]
enum RedisCmd {
    Config {
        #[command(subcommand)]
        command: RedisConfigCmd,
    },
}

#[derive(Subcommand, Debug)]
enum RedisConfigCmd {
    Set {
        instance: String,
        #[arg(long)]
        bind: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        pass: Option<String>,
        #[arg(long)]
        aof: Option<bool>,
    },
    Show {
        instance: String,
    },
}

#[derive(Subcommand, Debug)]
enum CertCmd {
    List,
    Renew { instance: String, domain: String },
}

#[derive(Subcommand, Debug)]
enum ApiCmd {
    Serve {
        #[arg(long)]
        port: Option<u16>,
    },
}

struct CliSink;

impl EventSink for CliSink {
    fn emit(&self, event: &str, payload: Value) {
        if event == "download-progress" {
            if let Ok(p) = serde_json::from_value::<DownloadProgress>(payload) {
                let total = p
                    .total
                    .map(|t| format!("{}/{}", p.downloaded, t))
                    .unwrap_or_else(|| p.downloaded.to_string());
                print!(
                    "\r  {} {} {:.1}% [{}] {}",
                    p.kind, p.version, p.percent, p.stage, total
                );
                let _ = std::io::stdout().flush();
                if p.percent >= 100.0 || p.stage.contains("完成") || p.stage.contains("命中") {
                    println!();
                }
            }
        }
    }

    fn notify(&self, title: &str, body: &str) {
        println!("\n[通知] {title}: {body}");
    }
}

/// GUI 模式下也会把 cli 子命令转发到这里。
pub fn run(args: &[String]) -> i32 {
    match run_inner(args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("错误: {e}");
            1
        }
    }
}

fn run_inner(args: &[String]) -> Result<()> {
    let cli = match Cli::try_parse_from(
        std::iter::once("svcbox".to_string()).chain(args.iter().cloned()),
    ) {
        Ok(cli) => cli,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() {
                Err(AppError::validation("命令行参数不合法"))
            } else {
                Ok(())
            };
        }
    };
    let home = cli.home.clone();
    let command = cli.command;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| AppError::other(format!("无法创建 tokio runtime: {e}")))?;
    rt.block_on(async move {
        let paths = AppPaths::resolve(home)?;
        let ctx = AppContext::new(paths, Arc::new(CliSink))?;
        dispatch(&ctx, command).await
    })
}

async fn dispatch(ctx: &Arc<AppContext>, command: Commands) -> Result<()> {
    match command {
        Commands::Version { command } => version_cmd(ctx, command).await,
        Commands::Instance { command } => instance_cmd(ctx, command).await,
        Commands::Logs {
            instance,
            follow,
            lines,
        } => logs_cmd(ctx, &instance, follow, lines).await,
        Commands::Doctor => doctor_cmd(ctx).await,
        Commands::Nginx { command } => nginx_cmd(ctx, command).await,
        Commands::Mysql { command } => mysql_cmd(ctx, command).await,
        Commands::Redis { command } => redis_cmd(ctx, command).await,
        Commands::Cert { command } => cert_cmd(ctx, command).await,
        Commands::Api { command } => api_cmd(ctx, command).await,
        Commands::Supervise { instance } => {
            crate::core::supervisor::run(ctx.clone(), &instance).await
        }
    }
}

async fn version_cmd(ctx: &Arc<AppContext>, command: VersionCmd) -> Result<()> {
    match command {
        VersionCmd::List => {
            let list = version::list_available(ctx).await?;
            let rows = list
                .iter()
                .map(|v| {
                    vec![
                        v.kind.to_string(),
                        v.version.clone(),
                        v.platform.clone(),
                        if v.installed {
                            "已安装".into()
                        } else {
                            "可下载".into()
                        },
                        v.installed_path.clone().unwrap_or_default(),
                    ]
                })
                .collect();
            print_table(&["类型", "版本", "平台", "状态", "安装路径"], rows);
        }
        VersionCmd::Install { kind, version } => {
            let kind: ServiceKind = kind.parse()?;
            println!("开始安装 {kind} {version} ...");
            let info = version::install(ctx, kind, &version).await?;
            println!("安装完成: {}", info.installed_path.unwrap_or_default());
        }
        VersionCmd::Remove { kind, version } => {
            let kind: ServiceKind = kind.parse()?;
            version::remove(ctx, kind, &version)?;
            println!("已删除 {kind} {version}");
        }
        VersionCmd::Import {
            kind,
            version,
            file,
            sha256,
        } => {
            let kind: ServiceKind = kind.parse()?;
            let info = version::import(ctx, kind, &version, &file, sha256).await?;
            println!("导入完成: {}", info.installed_path.unwrap_or_default());
        }
        VersionCmd::Refresh { url } => {
            let url = url
                .or_else(|| ctx.settings().ok().and_then(|s| s.manifest_url))
                .ok_or_else(|| AppError::validation("请指定 --url 或在设置中配置清单地址"))?;
            let count = version::refresh_remote(ctx, &url).await?;
            println!("清单已更新，共 {count} 条");
        }
        VersionCmd::Prune => {
            let removed = version::prune(ctx)?;
            if removed.is_empty() {
                println!("没有可清理的旧版本");
            } else {
                for item in removed {
                    println!("已清理 {item}");
                }
            }
        }
    }
    Ok(())
}

fn print_view(v: &InstanceView) {
    println!(
        "{} [{}] {}  端口 {}  状态 {}",
        v.name,
        v.kind,
        v.version,
        v.port,
        health_label(v.health)
    );
}

fn health_label(h: HealthStatus) -> &'static str {
    match h {
        HealthStatus::Stopped => "已停止",
        HealthStatus::Running => "运行中",
        HealthStatus::WarningPortNotListen => "端口未监听",
        HealthStatus::ErrorLogHasError => "日志报错",
        HealthStatus::Crashed => "已崩溃",
    }
}

async fn start_with_supervisor(ctx: &Arc<AppContext>, id: &str) -> Result<InstanceView> {
    let rec = ctx.require_instance(id)?;
    if crate::core::supervisor::supervisor_alive(&rec) {
        return instance::get(ctx, id).await;
    }
    if !utils::port_available(rec.port)? {
        return Err(AppError::conflict(format!(
            "端口 {} 已被占用，实例可能已在其他 svcbox 进程中运行",
            rec.port
        )));
    }
    crate::core::supervisor::clear_runtime_files(&rec);
    crate::core::supervisor::spawn_detached(ctx, &rec)?;
    crate::core::supervisor::wait_ready(&rec, Duration::from_secs(90)).await?;
    instance::get(ctx, id).await
}

async fn instance_cmd(ctx: &Arc<AppContext>, command: InstanceCmd) -> Result<()> {
    match command {
        InstanceCmd::Create {
            kind,
            name,
            version,
            port,
            root_password,
        } => {
            let kind: ServiceKind = kind.parse()?;
            println!("创建实例 {name} ...");
            let created = instance::create(
                ctx,
                crate::models::CreateInstanceOptions {
                    kind,
                    name: name.clone(),
                    version,
                    port,
                    root_password,
                },
            )
            .await?;
            print_view(&created.instance);
            if let Some(pwd) = created.initial_root_password {
                println!();
                println!("MySQL root 初始密码（仅显示一次）: {pwd}");
            }
        }
        InstanceCmd::Start { name } => {
            let v = start_with_supervisor(ctx, &name).await?;
            print_view(&v);
            println!("后台守护进程已接管，关闭当前终端不会停止服务。");
        }
        InstanceCmd::Stop { name } => {
            let v = instance::stop(ctx, &name).await?;
            print_view(&v);
        }
        InstanceCmd::Restart { name } => {
            instance::stop(ctx, &name).await?;
            let v = start_with_supervisor(ctx, &name).await?;
            print_view(&v);
        }
        InstanceCmd::List => {
            let list = instance::list_readonly(ctx).await?;
            let rows = list
                .iter()
                .map(|v| {
                    vec![
                        v.name.clone(),
                        v.kind.to_string(),
                        v.version.clone(),
                        v.port.to_string(),
                        health_label(v.health).to_string(),
                        v.pid.map(|p| p.to_string()).unwrap_or_default(),
                    ]
                })
                .collect();
            print_table(&["实例", "类型", "版本", "端口", "状态", "PID"], rows);
        }
        InstanceCmd::Remove { name, keep_data } => {
            instance::remove(ctx, &name, !keep_data).await?;
            println!(
                "已删除实例 {name}{}",
                if keep_data {
                    "（保留数据目录）"
                } else {
                    ""
                }
            );
        }
        InstanceCmd::Config { name } => {
            print!("{}", instance::read_main_config(ctx, &name)?);
        }
    }
    Ok(())
}

async fn logs_cmd(ctx: &Arc<AppContext>, id: &str, follow: bool, lines: usize) -> Result<()> {
    if !follow {
        let chunk = instance::read_logs(ctx, id, lines)?;
        print!("{}", chunk.content);
        return Ok(());
    }
    let path = instance::log_file_path(ctx, id)?;
    let chunk = instance::read_logs(ctx, id, 100)?;
    print!("{}", chunk.content);
    let _ = std::io::stdout().flush();
    let mut file = std::fs::File::open(&path)?;
    let mut pos = file.metadata()?.len();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = tokio::time::sleep(Duration::from_millis(500)) => {
                let Ok(meta) = std::fs::metadata(&path) else { continue };
                if meta.len() < pos {
                    pos = 0;
                    file = std::fs::File::open(&path)?;
                }
                if meta.len() > pos {
                    file.seek(SeekFrom::Start(pos))?;
                    let mut buf = String::new();
                    file.read_to_string(&mut buf)?;
                    pos = meta.len();
                    print!("{buf}");
                    let _ = std::io::stdout().flush();
                }
            }
        }
    }
    Ok(())
}

async fn doctor_cmd(ctx: &Arc<AppContext>) -> Result<()> {
    let report = doctor::run(ctx).await;
    for check in &report.checks {
        println!(
            "{} {:<28} {}",
            if check.ok { "[OK]" } else { "[!!]" },
            check.name,
            check.detail
        );
    }
    if report.ok {
        println!("\n自检通过");
        Ok(())
    } else {
        Err(AppError::other("自检发现异常，请按上面提示处理"))
    }
}

async fn nginx_cmd(ctx: &Arc<AppContext>, command: NginxCmd) -> Result<()> {
    match command {
        NginxCmd::Site { command } => match command {
            NginxSiteCmd::Create {
                instance,
                domain,
                root,
                index,
                custom_locations,
                https,
                cert_domain,
            } => {
                let site = SiteConfig {
                    server_name: domain.clone(),
                    root,
                    index,
                    custom_locations,
                    https,
                    cert_domain,
                };
                service::nginx::create_site(ctx, &instance, site).await?;
                println!("站点 {domain} 已创建");
            }
            NginxSiteCmd::List { instance } => {
                let sites = service::nginx::list_sites(ctx, &instance)?;
                let rows = sites
                    .iter()
                    .map(|s| {
                        vec![
                            s.server_name.clone(),
                            s.root.clone(),
                            if s.https { "是".into() } else { "否".into() },
                            s.cert_domain.clone().unwrap_or_default(),
                        ]
                    })
                    .collect();
                print_table(&["域名", "根目录", "HTTPS", "证书域名"], rows);
            }
            NginxSiteCmd::Delete { instance, domain } => {
                service::nginx::delete_site(ctx, &instance, &domain).await?;
                println!("站点 {domain} 已删除（网站根目录未删除）");
            }
        },
        NginxCmd::Global {
            instance,
            worker_processes,
            worker_connections,
            keepalive_timeout,
            client_max_body_size,
            gzip,
            https_port,
        } => {
            service::nginx::update_global(
                ctx,
                &instance,
                crate::models::NginxGlobalConfig {
                    worker_processes,
                    worker_connections,
                    keepalive_timeout,
                    gzip: gzip.unwrap_or(true),
                    client_max_body_size,
                    https_port,
                },
            )
            .await?;
            println!("Nginx 全局配置已更新");
        }
        NginxCmd::Reload { instance } => {
            service::nginx::reload(ctx, &instance).await?;
            println!("Nginx 已 reload");
        }
        NginxCmd::Https {
            command: NginxHttpsCmd::Apply { instance, domain },
        } => {
            println!("正在为 {domain} 申请证书（HTTP-01）...");
            let info = service::nginx::apply_https(ctx, &instance, &domain, &domain).await?;
            println!("HTTPS 已启用，证书到期时间: {}", info.expires_at);
        }
        NginxCmd::Cert {
            command: NginxCertCmd::Renew { instance, domain },
        } => {
            let info = service::nginx::renew_cert(ctx, &instance, &domain).await?;
            println!("证书已续期，到期时间: {}", info.expires_at);
        }
    }
    Ok(())
}

async fn mysql_cmd(ctx: &Arc<AppContext>, command: MysqlCmd) -> Result<()> {
    match command {
        MysqlCmd::Db { command } => match command {
            MysqlDbCmd::Create {
                instance,
                name,
                charset,
                collate,
            } => {
                service::mysql::create_database(ctx, &instance, &name, charset, collate)?;
                println!("数据库 {name} 已创建");
            }
            MysqlDbCmd::List { instance } => {
                for db in service::mysql::list_databases(ctx, &instance)? {
                    println!("{db}");
                }
            }
            MysqlDbCmd::Drop { instance, name } => {
                service::mysql::drop_database(ctx, &instance, &name)?;
                println!("数据库 {name} 已删除");
            }
        },
        MysqlCmd::User { command } => match command {
            MysqlUserCmd::Create {
                instance,
                user,
                host,
                password,
                dbs,
            } => {
                service::mysql::create_user(ctx, &instance, &user, &host, &password, dbs)?;
                println!("账号 {user}@{host} 已创建");
            }
            MysqlUserCmd::List { instance } => {
                let users = service::mysql::list_users(ctx, &instance)?;
                let rows = users
                    .iter()
                    .map(|u| vec![u.user.clone(), u.host.clone(), u.dbs.join(", ")])
                    .collect();
                print_table(&["账号", "Host", "授权库"], rows);
            }
            MysqlUserCmd::Pass {
                instance,
                user,
                host,
                password,
            } => {
                service::mysql::alter_user_password(ctx, &instance, &user, &host, &password)?;
                println!("账号 {user}@{host} 密码已修改");
            }
            MysqlUserCmd::Drop {
                instance,
                user,
                host,
            } => {
                service::mysql::drop_user(ctx, &instance, &user, &host)?;
                println!("账号 {user}@{host} 已删除");
            }
        },
        MysqlCmd::RootPass { instance, password } => {
            println!("正在重置 root 密码（实例会短暂停止）...");
            service::mysql::reset_root_password(ctx, &instance, &password).await?;
            println!("root 密码已重置");
        }
    }
    Ok(())
}

async fn redis_cmd(ctx: &Arc<AppContext>, command: RedisCmd) -> Result<()> {
    match command {
        RedisCmd::Config {
            command:
                RedisConfigCmd::Set {
                    instance,
                    bind,
                    port,
                    pass,
                    aof,
                },
        } => {
            let current = service::redis::config_view(ctx, &instance)?;
            let view = service::redis::update_config(
                ctx,
                &instance,
                RedisConfigInput {
                    bind: bind.unwrap_or(current.bind),
                    port: port.unwrap_or(current.port),
                    requirepass: pass,
                    appendonly: aof.unwrap_or(current.appendonly),
                },
            )
            .await?;
            println!(
                "配置已保存：bind={} port={} password={} appendonly={}",
                view.bind,
                view.port,
                if view.has_password {
                    "已设置"
                } else {
                    "无"
                },
                view.appendonly
            );
            if view.requires_restart {
                println!("注意：bind/port/密码/AOF 变更需要重启 Redis 后生效");
            }
        }
        RedisCmd::Config {
            command: RedisConfigCmd::Show { instance },
        } => {
            let view = service::redis::config_view(ctx, &instance)?;
            println!("bind: {}", view.bind);
            println!("port: {}", view.port);
            println!(
                "requirepass: {}",
                if view.has_password {
                    "已设置"
                } else {
                    "无"
                }
            );
            println!("appendonly: {}", view.appendonly);
        }
    }
    Ok(())
}

async fn cert_cmd(ctx: &Arc<AppContext>, command: CertCmd) -> Result<()> {
    match command {
        CertCmd::List => {
            let certs = cert::list(ctx)?;
            let rows = certs
                .iter()
                .map(|c| {
                    vec![
                        c.domain.clone(),
                        c.instance_name.clone(),
                        c.expires_at.clone(),
                        c.days_left.to_string(),
                    ]
                })
                .collect();
            print_table(&["域名", "实例", "到期时间", "剩余天数"], rows);
        }
        CertCmd::Renew { instance, domain } => {
            let rec = ctx.require_instance(&instance)?;
            let info = cert::renew(ctx, &rec, &domain).await?;
            println!("证书续期成功，到期时间: {}", info.expires_at);
        }
    }
    Ok(())
}

async fn api_cmd(ctx: &Arc<AppContext>, command: ApiCmd) -> Result<()> {
    match command {
        ApiCmd::Serve { port } => {
            let port = port.unwrap_or(ctx.settings()?.http_api_port);
            println!("HTTP API: http://127.0.0.1:{port}");
            crate::api::serve(ctx.clone(), port).await?;
        }
    }
    Ok(())
}

fn print_table(headers: &[&str], rows: Vec<Vec<String>>) {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in &rows {
        for (i, cell) in row.iter().enumerate() {
            if i < widths.len() {
                widths[i] = widths[i].max(cell.chars().count());
            }
        }
    }
    let render = |cells: &[String]| {
        cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let width = widths.get(i).copied().unwrap_or(0);
                let pad = width.saturating_sub(c.chars().count());
                format!("{c}{}", " ".repeat(pad))
            })
            .collect::<Vec<_>>()
            .join("  ")
    };
    println!(
        "{}",
        render(&headers.iter().map(|h| h.to_string()).collect::<Vec<_>>())
    );
    println!(
        "{}",
        widths
            .iter()
            .map(|w| "-".repeat(*w))
            .collect::<Vec<_>>()
            .join("  ")
    );
    for row in rows {
        println!("{}", render(&row));
    }
}
