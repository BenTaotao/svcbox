use axum::extract::{Path as AxPath, Query, Request, State};
use axum::http::header::{HOST, ORIGIN};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::core::{cert, doctor, instance, service, version, AppContext};
use crate::error::AppError;
use crate::models::{CreateInstanceOptions, RedisConfigInput, ServiceKind, Settings, SiteConfig};

struct ApiError(AppError);

impl From<AppError> for ApiError {
    fn from(value: AppError) -> Self {
        Self(value)
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(value: serde_json::Error) -> Self {
        Self(AppError::Json(value))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            AppError::Validation(_) => StatusCode::BAD_REQUEST,
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        (status, Json(json!({"error": self.0.to_string()}))).into_response()
    }
}

type ApiResult<T> = std::result::Result<Json<T>, ApiError>;

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct DeleteInstanceQuery {
    delete_data: bool,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct LogQuery {
    lines: Option<usize>,
}

fn kind(s: &str) -> Result<ServiceKind, ApiError> {
    s.parse::<ServiceKind>().map_err(ApiError)
}

/// 阻止 DNS rebinding 与浏览器跨站请求：Host 必须是本机，Origin（若有）必须是本机或 Tauri。
async fn local_only(req: Request, next: Next) -> Response {
    let host_ok = req
        .headers()
        .get(HOST)
        .and_then(|v| v.to_str().ok())
        .map(|host| {
            let host = host.trim().to_ascii_lowercase();
            host == "127.0.0.1"
                || host.starts_with("127.0.0.1:")
                || host == "localhost"
                || host.starts_with("localhost:")
        })
        .unwrap_or(false);
    let origin_ok = match req.headers().get(ORIGIN).and_then(|v| v.to_str().ok()) {
        None => true,
        Some(origin) => {
            let origin = origin.to_ascii_lowercase();
            origin.starts_with("http://127.0.0.1")
                || origin.starts_with("http://localhost")
                || origin.starts_with("https://tauri.localhost")
                || origin.starts_with("http://tauri.localhost")
                || origin == "tauri://localhost"
        }
    };
    if !host_ok || !origin_ok {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error": "仅允许本机程序访问 HTTP API"})),
        )
            .into_response();
    }
    next.run(req).await
}

pub fn router(ctx: Arc<AppContext>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/doctor", get(doctor_handler))
        .route("/api/settings", get(get_settings).put(update_settings))
        .route("/api/versions", get(list_versions))
        .route("/api/versions/refresh", post(refresh_manifest))
        .route("/api/versions/prune", post(prune_versions))
        .route("/api/versions/install", post(install_version))
        .route("/api/versions/import", post(import_version))
        .route("/api/versions/:kind/:version", delete(remove_version))
        .route("/api/instances", get(list_instances).post(create_instance))
        .route(
            "/api/instances/:id",
            get(get_instance).delete(delete_instance),
        )
        .route("/api/instances/:id/start", post(start_instance))
        .route("/api/instances/:id/stop", post(stop_instance))
        .route("/api/instances/:id/restart", post(restart_instance))
        .route("/api/instances/:id/logs", get(read_logs))
        .route(
            "/api/instances/:id/config",
            get(read_config).put(save_config),
        )
        .route(
            "/api/instances/:id/nginx/sites",
            get(list_sites).post(create_site).put(update_site),
        )
        .route("/api/instances/:id/nginx/sites/:name", delete(delete_site))
        .route("/api/instances/:id/nginx/reload", post(reload_nginx))
        .route(
            "/api/instances/:id/nginx/global",
            get(get_nginx_global).put(update_nginx_global),
        )
        .route("/api/instances/:id/nginx/https", post(apply_https))
        .route(
            "/api/instances/:id/mysql/dbs",
            get(list_dbs).post(create_db),
        )
        .route("/api/instances/:id/mysql/dbs/:name", delete(drop_db))
        .route(
            "/api/instances/:id/mysql/users",
            get(list_users).post(create_user),
        )
        .route(
            "/api/instances/:id/mysql/users/:user/:host",
            delete(drop_user),
        )
        .route(
            "/api/instances/:id/mysql/users/:user/:host/password",
            put(alter_user_password),
        )
        .route(
            "/api/instances/:id/mysql/root-password",
            post(reset_root_password),
        )
        .route("/api/instances/:id/redis", get(get_redis).put(update_redis))
        .route("/api/certs", get(list_certs))
        .route("/api/certs/:instance_id/:domain/renew", post(renew_cert))
        .with_state(ctx)
        .layer(middleware::from_fn(local_only))
}

pub async fn serve(ctx: Arc<AppContext>, port: u16) -> crate::error::Result<()> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("HTTP API 监听 http://127.0.0.1:{port}");
    axum::serve(listener, router(ctx))
        .await
        .map_err(|e| AppError::other(format!("HTTP API 退出: {e}")))
}

async fn health() -> Json<serde_json::Value> {
    Json(json!({"ok": true, "name": "svcbox"}))
}

async fn doctor_handler(State(ctx): State<Arc<AppContext>>) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(doctor::run(&ctx).await)?))
}

async fn get_settings(State(ctx): State<Arc<AppContext>>) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(ctx.settings()?)?))
}

async fn update_settings(
    State(ctx): State<Arc<AppContext>>,
    Json(settings): Json<Settings>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(ctx.update_settings(settings)?)?))
}

async fn list_versions(State(ctx): State<Arc<AppContext>>) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(
        version::list_available(&ctx).await?,
    )?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ManifestBody {
    url: Option<String>,
}

async fn refresh_manifest(
    State(ctx): State<Arc<AppContext>>,
    Json(body): Json<ManifestBody>,
) -> ApiResult<serde_json::Value> {
    let url = body
        .url
        .or_else(|| ctx.settings().ok().and_then(|s| s.manifest_url))
        .ok_or_else(|| ApiError(AppError::validation("未配置远端清单地址")))?;
    let count = version::refresh_remote(&ctx, &url).await?;
    Ok(Json(json!({"ok": true, "count": count})))
}

async fn prune_versions(State(ctx): State<Arc<AppContext>>) -> ApiResult<serde_json::Value> {
    let removed = version::prune(&ctx)?;
    Ok(Json(json!({"removed": removed})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct VersionBody {
    kind: ServiceKind,
    version: String,
}

async fn install_version(
    State(ctx): State<Arc<AppContext>>,
    Json(body): Json<VersionBody>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(
        version::install(&ctx, body.kind, &body.version).await?,
    )?))
}

async fn import_version(
    State(ctx): State<Arc<AppContext>>,
    Json(body): Json<ImportBody>,
) -> ApiResult<serde_json::Value> {
    let info = version::import(
        &ctx,
        body.kind,
        &body.version,
        std::path::Path::new(&body.file),
        body.sha256,
    )
    .await?;
    Ok(Json(serde_json::to_value(info)?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportBody {
    kind: ServiceKind,
    version: String,
    file: String,
    sha256: Option<String>,
}

async fn remove_version(
    State(ctx): State<Arc<AppContext>>,
    AxPath((kind_name, ver)): AxPath<(String, String)>,
) -> ApiResult<serde_json::Value> {
    version::remove(&ctx, kind(&kind_name)?, &ver)?;
    Ok(Json(json!({"ok": true})))
}

async fn list_instances(State(ctx): State<Arc<AppContext>>) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(instance::list(&ctx).await?)?))
}

async fn create_instance(
    State(ctx): State<Arc<AppContext>>,
    Json(opts): Json<CreateInstanceOptions>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(
        instance::create(&ctx, opts).await?,
    )?))
}

async fn get_instance(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(instance::get(&ctx, &id).await?)?))
}

async fn start_instance(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(
        instance::start(&ctx, &id).await?,
    )?))
}

async fn stop_instance(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(
        instance::stop(&ctx, &id).await?,
    )?))
}

async fn restart_instance(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(
        instance::restart(&ctx, &id).await?,
    )?))
}

async fn delete_instance(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
    Query(q): Query<DeleteInstanceQuery>,
) -> ApiResult<serde_json::Value> {
    instance::remove(&ctx, &id, q.delete_data).await?;
    Ok(Json(json!({"ok": true})))
}

async fn read_logs(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
    Query(q): Query<LogQuery>,
) -> ApiResult<serde_json::Value> {
    let lines = q.lines.unwrap_or(300);
    Ok(Json(serde_json::to_value(instance::read_logs(
        &ctx, &id, lines,
    )?)?))
}

async fn read_config(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(
        json!({"content": instance::read_main_config(&ctx, &id)?}),
    ))
}

#[derive(Deserialize)]
struct ConfigBody {
    content: String,
}

async fn save_config(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
    Json(body): Json<ConfigBody>,
) -> ApiResult<serde_json::Value> {
    instance::save_raw_config(&ctx, &id, &body.content).await?;
    Ok(Json(json!({"ok": true})))
}

async fn list_sites(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(service::nginx::list_sites(
        &ctx, &id,
    )?)?))
}

async fn create_site(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
    Json(site): Json<SiteConfig>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(
        service::nginx::create_site(&ctx, &id, site).await?,
    )?))
}

async fn update_site(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
    Json(site): Json<SiteConfig>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(
        service::nginx::update_site(&ctx, &id, site).await?,
    )?))
}

async fn delete_site(
    State(ctx): State<Arc<AppContext>>,
    AxPath((id, name)): AxPath<(String, String)>,
) -> ApiResult<serde_json::Value> {
    service::nginx::delete_site(&ctx, &id, &name).await?;
    Ok(Json(json!({"ok": true})))
}

async fn reload_nginx(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
) -> ApiResult<serde_json::Value> {
    service::nginx::reload(&ctx, &id).await?;
    Ok(Json(json!({"ok": true})))
}

async fn get_nginx_global(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(service::nginx::global_config(
        &ctx, &id,
    )?)?))
}

async fn update_nginx_global(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
    Json(cfg): Json<crate::models::NginxGlobalConfig>,
) -> ApiResult<serde_json::Value> {
    service::nginx::update_global(&ctx, &id, cfg).await?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HttpsBody {
    server_name: String,
    domain: String,
}

async fn apply_https(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
    Json(body): Json<HttpsBody>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(
        service::nginx::apply_https(&ctx, &id, &body.server_name, &body.domain).await?,
    )?))
}

async fn list_dbs(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(service::mysql::list_databases(
        &ctx, &id,
    )?)?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateDbBody {
    name: String,
    charset: Option<String>,
    collate: Option<String>,
}

async fn create_db(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
    Json(body): Json<CreateDbBody>,
) -> ApiResult<serde_json::Value> {
    service::mysql::create_database(&ctx, &id, &body.name, body.charset, body.collate)?;
    Ok(Json(json!({"ok": true})))
}

async fn drop_db(
    State(ctx): State<Arc<AppContext>>,
    AxPath((id, name)): AxPath<(String, String)>,
) -> ApiResult<serde_json::Value> {
    service::mysql::drop_database(&ctx, &id, &name)?;
    Ok(Json(json!({"ok": true})))
}

async fn list_users(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(service::mysql::list_users(
        &ctx, &id,
    )?)?))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateUserBody {
    user: String,
    host: String,
    password: String,
    #[serde(default)]
    dbs: Vec<String>,
}

async fn create_user(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
    Json(body): Json<CreateUserBody>,
) -> ApiResult<serde_json::Value> {
    service::mysql::create_user(&ctx, &id, &body.user, &body.host, &body.password, body.dbs)?;
    Ok(Json(json!({"ok": true})))
}

#[derive(Deserialize)]
struct PasswordBody {
    password: String,
}

async fn alter_user_password(
    State(ctx): State<Arc<AppContext>>,
    AxPath((id, user, host)): AxPath<(String, String, String)>,
    Json(body): Json<PasswordBody>,
) -> ApiResult<serde_json::Value> {
    service::mysql::alter_user_password(&ctx, &id, &user, &host, &body.password)?;
    Ok(Json(json!({"ok": true})))
}

async fn drop_user(
    State(ctx): State<Arc<AppContext>>,
    AxPath((id, user, host)): AxPath<(String, String, String)>,
) -> ApiResult<serde_json::Value> {
    service::mysql::drop_user(&ctx, &id, &user, &host)?;
    Ok(Json(json!({"ok": true})))
}

async fn reset_root_password(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
    Json(body): Json<PasswordBody>,
) -> ApiResult<serde_json::Value> {
    service::mysql::reset_root_password(&ctx, &id, &body.password).await?;
    Ok(Json(json!({"ok": true})))
}

async fn get_redis(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(service::redis::config_view(
        &ctx, &id,
    )?)?))
}

async fn update_redis(
    State(ctx): State<Arc<AppContext>>,
    AxPath(id): AxPath<String>,
    Json(input): Json<RedisConfigInput>,
) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(
        service::redis::update_config(&ctx, &id, input).await?,
    )?))
}

async fn list_certs(State(ctx): State<Arc<AppContext>>) -> ApiResult<serde_json::Value> {
    Ok(Json(serde_json::to_value(cert::list(&ctx)?)?))
}

async fn renew_cert(
    State(ctx): State<Arc<AppContext>>,
    AxPath((id, domain)): AxPath<(String, String)>,
) -> ApiResult<serde_json::Value> {
    let rec = ctx.require_instance(&id)?;
    Ok(Json(serde_json::to_value(
        cert::renew(&ctx, &rec, &domain).await?,
    )?))
}
