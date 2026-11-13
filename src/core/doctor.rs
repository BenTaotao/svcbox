use std::fs;

use crate::core::{version, AppContext};
use crate::models::{DoctorCheck, DoctorReport, InstanceStatus};
use crate::utils;

fn check(name: &str, ok: bool, detail: impl Into<String>) -> DoctorCheck {
    DoctorCheck {
        name: name.to_string(),
        ok,
        detail: detail.into(),
    }
}

pub async fn run(ctx: &AppContext) -> DoctorReport {
    let mut checks = Vec::new();

    let probe = ctx
        .paths
        .home
        .join(format!(".doctor-{}", std::process::id()));
    let writable = fs::write(&probe, b"ok").is_ok() && fs::remove_file(&probe).is_ok();
    checks.push(check(
        "数据目录可写",
        writable,
        ctx.paths.home.to_string_lossy().to_string(),
    ));

    let db_ok = ctx.settings().is_ok();
    checks.push(check(
        "SQLite 可读写",
        db_ok,
        ctx.paths.db_file.to_string_lossy().to_string(),
    ));

    match utils::find_bash() {
        Some(path) => checks.push(check("bash（acme.sh 依赖）", true, path.to_string_lossy())),
        None => checks.push(check(
            "bash（acme.sh 依赖）",
            false,
            "未找到 bash，请安装 Git for Windows 后再申请 HTTPS 证书",
        )),
    }

    let versions = ctx.db.list_versions().unwrap_or_default();
    if versions.is_empty() {
        checks.push(check("已安装版本", true, "暂无已安装版本"));
    }
    let manifest = version::Manifest::load(ctx).ok();
    for v in versions {
        let in_manifest = manifest
            .as_ref()
            .map(|m| {
                m.entries()
                    .iter()
                    .any(|e| e.kind == v.kind && e.version == v.version)
            })
            .unwrap_or(false);
        let bin = version::installed_binary(&v);
        if bin.is_file() {
            checks.push(check(
                &format!("{} {}", v.kind, v.version),
                true,
                format!(
                    "{}{}",
                    bin.display(),
                    if in_manifest {
                        ""
                    } else {
                        "（旧版本，不在当前清单）"
                    }
                ),
            ));
        } else {
            checks.push(check(
                &format!("{} {}", v.kind, v.version),
                false,
                format!("主程序缺失: {}；可在版本仓库删除该版本", bin.display()),
            ));
        }
    }

    let instances = ctx.db.list_instances().unwrap_or_default();
    for inst in &instances {
        if inst.status == InstanceStatus::Stopped {
            match utils::port_available(inst.port) {
                Ok(true) => checks.push(check(
                    &format!("实例 {} 端口 {}", inst.id, inst.port),
                    true,
                    "端口空闲",
                )),
                Ok(false) => checks.push(check(
                    &format!("实例 {} 端口 {}", inst.id, inst.port),
                    false,
                    "端口被其他程序占用，启动前请处理",
                )),
                Err(e) => checks.push(check(
                    &format!("实例 {} 端口 {}", inst.id, inst.port),
                    false,
                    e.to_string(),
                )),
            }
        } else {
            let open = utils::tcp_open(inst.port).await;
            checks.push(check(
                &format!("实例 {} 端口 {}", inst.id, inst.port),
                open,
                if open {
                    "端口可连通"
                } else {
                    "端口未监听"
                },
            ));
        }
    }

    let has_certs = ctx
        .db
        .list_certificates()
        .map(|c| !c.is_empty())
        .unwrap_or(false);
    if has_certs {
        let script = ctx.paths.cache_acme.join("acme.sh");
        checks.push(check(
            "acme.sh",
            script.is_file(),
            if script.is_file() {
                script.to_string_lossy().to_string()
            } else {
                "首次申请证书时会自动下载".to_string()
            },
        ));
    }

    let ok = checks.iter().all(|c| c.ok);
    DoctorReport { ok, checks }
}
