use serde_json::json;
use std::sync::Arc;
use tauri::State;

use crate::core::{cert, doctor, instance, service, version, AppContext};
use crate::models::{
    CreateInstanceOptions, DoctorReport, InstanceView, NginxGlobalConfig, RedisConfigInput,
    RedisConfigView, ServiceKind, Settings, SettingsResult, SiteConfig, VersionInfo,
};

fn core<'a>(state: &'a State<'_, Arc<AppContext>>) -> &'a AppContext {
    state.inner().as_ref()
}

fn err(e: crate::error::AppError) -> String {
    e.to_string()
}

#[tauri::command]
pub fn get_settings(state: State<'_, Arc<AppContext>>) -> Result<Settings, String> {
    core(&state).settings().map_err(err)
}

#[tauri::command]
pub fn update_settings(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppContext>>,
    settings: Settings,
) -> Result<SettingsResult, String> {
    let ctx = core(&state);
    let before = ctx.settings().map(|s| s.autostart).unwrap_or(false);
    let result = ctx.update_settings(settings).map_err(err)?;
    if result.settings.autostart != before {
        use tauri_plugin_autostart::ManagerExt;
        if result.settings.autostart {
            let _ = app.autolaunch().enable();
        } else {
            let _ = app.autolaunch().disable();
        }
    }
    Ok(result)
}

#[tauri::command]
pub fn get_paths(state: State<'_, Arc<AppContext>>) -> serde_json::Value {
    let p = &core(&state).paths;
    json!({
        "home": p.home,
        "dbFile": p.db_file,
        "versions": p.versions,
        "instances": p.instances,
        "cache": p.cache,
        "logs": p.logs,
    })
}

#[tauri::command]
pub async fn get_doctor(state: State<'_, Arc<AppContext>>) -> Result<DoctorReport, String> {
    Ok(doctor::run(core(&state)).await)
}

#[tauri::command]
pub async fn list_versions(state: State<'_, Arc<AppContext>>) -> Result<Vec<VersionInfo>, String> {
    version::list_available(core(&state)).await.map_err(err)
}

#[tauri::command]
pub fn prune_versions(state: State<'_, Arc<AppContext>>) -> Result<Vec<String>, String> {
    crate::core::version::prune(core(&state)).map_err(err)
}

#[tauri::command]
pub async fn refresh_manifest(
    state: State<'_, Arc<AppContext>>,
    url: Option<String>,
) -> Result<usize, String> {
    let ctx = core(&state);
    let url = url
        .or_else(|| ctx.settings().ok().and_then(|s| s.manifest_url))
        .ok_or_else(|| "请先配置远端清单地址".to_string())?;
    version::refresh_remote(ctx, &url).await.map_err(err)
}

#[tauri::command]
pub async fn install_version(
    state: State<'_, Arc<AppContext>>,
    kind: ServiceKind,
    version: String,
) -> Result<VersionInfo, String> {
    crate::core::version::install(core(&state), kind, &version)
        .await
        .map_err(err)
}

#[tauri::command]
pub fn remove_version(
    state: State<'_, Arc<AppContext>>,
    kind: ServiceKind,
    version: String,
) -> Result<(), String> {
    crate::core::version::remove(core(&state), kind, &version).map_err(err)
}

#[tauri::command]
pub async fn import_version(
    state: State<'_, Arc<AppContext>>,
    kind: ServiceKind,
    version: String,
    path: String,
    sha256: Option<String>,
) -> Result<VersionInfo, String> {
    crate::core::version::import(
        core(&state),
        kind,
        &version,
        std::path::Path::new(&path),
        sha256,
    )
    .await
    .map_err(err)
}

#[tauri::command]
pub async fn list_instances(
    state: State<'_, Arc<AppContext>>,
) -> Result<Vec<InstanceView>, String> {
    instance::list(core(&state)).await.map_err(err)
}

#[tauri::command]
pub async fn get_instance(
    state: State<'_, Arc<AppContext>>,
    id: String,
) -> Result<InstanceView, String> {
    instance::get(core(&state), &id).await.map_err(err)
}

#[tauri::command]
pub async fn create_instance(
    state: State<'_, Arc<AppContext>>,
    opts: CreateInstanceOptions,
) -> Result<crate::models::CreateInstanceResult, String> {
    instance::create(core(&state), opts).await.map_err(err)
}

#[tauri::command]
pub async fn start_instance(
    state: State<'_, Arc<AppContext>>,
    id: String,
) -> Result<InstanceView, String> {
    instance::start(core(&state), &id).await.map_err(err)
}

#[tauri::command]
pub async fn stop_instance(
    state: State<'_, Arc<AppContext>>,
    id: String,
) -> Result<InstanceView, String> {
    instance::stop(core(&state), &id).await.map_err(err)
}

#[tauri::command]
pub async fn restart_instance(
    state: State<'_, Arc<AppContext>>,
    id: String,
) -> Result<InstanceView, String> {
    instance::restart(core(&state), &id).await.map_err(err)
}

#[tauri::command]
pub async fn remove_instance(
    state: State<'_, Arc<AppContext>>,
    id: String,
    delete_data: bool,
) -> Result<(), String> {
    instance::remove(core(&state), &id, delete_data)
        .await
        .map_err(err)
}

#[tauri::command]
pub fn read_logs(
    state: State<'_, Arc<AppContext>>,
    id: String,
    lines: Option<usize>,
) -> Result<crate::models::LogChunk, String> {
    instance::read_logs(core(&state), &id, lines.unwrap_or(300)).map_err(err)
}

#[tauri::command]
pub fn read_raw_config(state: State<'_, Arc<AppContext>>, id: String) -> Result<String, String> {
    instance::read_main_config(core(&state), &id).map_err(err)
}

#[tauri::command]
pub async fn save_raw_config(
    state: State<'_, Arc<AppContext>>,
    id: String,
    content: String,
) -> Result<(), String> {
    instance::save_raw_config(core(&state), &id, &content)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn start_all_instances(state: State<'_, Arc<AppContext>>) -> Result<Vec<String>, String> {
    Ok(instance::start_all(core(&state))
        .await
        .into_iter()
        .map(|(id, e)| format!("{id}: {e}"))
        .collect())
}

#[tauri::command]
pub async fn stop_all_instances(state: State<'_, Arc<AppContext>>) -> Result<Vec<String>, String> {
    Ok(instance::stop_all(core(&state))
        .await
        .into_iter()
        .map(|(id, e)| format!("{id}: {e}"))
        .collect())
}

#[tauri::command]
pub fn list_nginx_sites(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
) -> Result<Vec<SiteConfig>, String> {
    service::nginx::list_sites(core(&state), &instance_id).map_err(err)
}

#[tauri::command]
pub async fn create_nginx_site(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
    site: SiteConfig,
) -> Result<SiteConfig, String> {
    service::nginx::create_site(core(&state), &instance_id, site)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn update_nginx_site(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
    site: SiteConfig,
) -> Result<SiteConfig, String> {
    service::nginx::update_site(core(&state), &instance_id, site)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn delete_nginx_site(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
    server_name: String,
) -> Result<(), String> {
    service::nginx::delete_site(core(&state), &instance_id, &server_name)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn reload_nginx(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
) -> Result<(), String> {
    service::nginx::reload(core(&state), &instance_id)
        .await
        .map_err(err)
}

#[tauri::command]
pub fn get_nginx_global(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
) -> Result<NginxGlobalConfig, String> {
    service::nginx::global_config(core(&state), &instance_id).map_err(err)
}

#[tauri::command]
pub async fn update_nginx_global(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
    cfg: NginxGlobalConfig,
) -> Result<(), String> {
    service::nginx::update_global(core(&state), &instance_id, cfg)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn apply_site_https(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
    server_name: String,
    domain: String,
) -> Result<crate::models::CertificateInfo, String> {
    service::nginx::apply_https(core(&state), &instance_id, &server_name, &domain)
        .await
        .map_err(err)
}

#[tauri::command]
pub async fn renew_certificate(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
    domain: String,
) -> Result<crate::models::CertificateInfo, String> {
    let ctx = core(&state);
    let rec = ctx.require_instance(&instance_id).map_err(err)?;
    cert::renew(ctx, &rec, &domain).await.map_err(err)
}

#[tauri::command]
pub fn list_mysql_dbs(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
) -> Result<Vec<String>, String> {
    service::mysql::list_databases(core(&state), &instance_id).map_err(err)
}

#[tauri::command]
pub fn create_mysql_db(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
    db_name: String,
    charset: Option<String>,
    collate: Option<String>,
) -> Result<(), String> {
    service::mysql::create_database(core(&state), &instance_id, &db_name, charset, collate)
        .map_err(err)
}

#[tauri::command]
pub fn drop_mysql_db(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
    db_name: String,
) -> Result<(), String> {
    service::mysql::drop_database(core(&state), &instance_id, &db_name).map_err(err)
}

#[tauri::command]
pub fn list_mysql_users(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
) -> Result<Vec<crate::models::MysqlUser>, String> {
    service::mysql::list_users(core(&state), &instance_id).map_err(err)
}

#[tauri::command]
pub fn create_mysql_user(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
    user: String,
    host: String,
    password: String,
    dbs: Vec<String>,
) -> Result<(), String> {
    service::mysql::create_user(core(&state), &instance_id, &user, &host, &password, dbs)
        .map_err(err)
}

#[tauri::command]
pub fn alter_mysql_user_password(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
    user: String,
    host: String,
    new_password: String,
) -> Result<(), String> {
    service::mysql::alter_user_password(core(&state), &instance_id, &user, &host, &new_password)
        .map_err(err)
}

#[tauri::command]
pub fn drop_mysql_user(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
    user: String,
    host: String,
) -> Result<(), String> {
    service::mysql::drop_user(core(&state), &instance_id, &user, &host).map_err(err)
}

#[tauri::command]
pub async fn reset_mysql_root_password(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
    new_password: String,
) -> Result<(), String> {
    service::mysql::reset_root_password(core(&state), &instance_id, &new_password)
        .await
        .map_err(err)
}

#[tauri::command]
pub fn get_redis_config(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
) -> Result<RedisConfigView, String> {
    service::redis::config_view(core(&state), &instance_id).map_err(err)
}

#[tauri::command]
pub async fn update_redis_config(
    state: State<'_, Arc<AppContext>>,
    instance_id: String,
    input: RedisConfigInput,
) -> Result<RedisConfigView, String> {
    service::redis::update_config(core(&state), &instance_id, input)
        .await
        .map_err(err)
}

#[tauri::command]
pub fn list_certificates(
    state: State<'_, Arc<AppContext>>,
) -> Result<Vec<crate::models::CertificateInfo>, String> {
    cert::list(core(&state)).map_err(err)
}
