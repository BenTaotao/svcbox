#![cfg_attr(
    all(not(debug_assertions), feature = "desktop", target_os = "windows"),
    windows_subsystem = "windows"
)]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli_mode = args.first().map(|a| a.as_str()) == Some("cli");
    let internal_mode = args.first().map(|a| a.as_str()) == Some("__supervise");
    let rest = if cli_mode { &args[1..] } else { &args[..] };

    #[cfg(feature = "desktop")]
    {
        if cli_mode || internal_mode {
            init_console_logging();
            std::process::exit(benv::cli::run(rest));
        }
        run_desktop();
    }

    #[cfg(not(feature = "desktop"))]
    {
        let _ = cli_mode;
        let _ = internal_mode;
        init_console_logging();
        std::process::exit(benv::cli::run(rest));
    }
}

/// GUI 模式也支持 `--home <目录>` / `--data-dir <目录>`，优先级高于设置页的 bootstrap。
#[cfg(feature = "desktop")]
fn desktop_home_arg() -> Option<std::path::PathBuf> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--home" || arg == "--data-dir" {
            return args.next().map(std::path::PathBuf::from);
        }
        if let Some(value) = arg
            .strip_prefix("--home=")
            .or_else(|| arg.strip_prefix("--data-dir="))
        {
            return Some(std::path::PathBuf::from(value));
        }
    }
    None
}

fn init_console_logging() {
    let filter = std::env::var("SVCBOX_LOG").unwrap_or_else(|_| "info".to_string());
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

#[cfg(feature = "desktop")]
fn run_desktop() {
    use benv::core::{AppContext, EventSink};
    use std::sync::Arc;
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::TrayIconBuilder;
    use tauri::{Emitter, Manager, WindowEvent};
    use tauri_plugin_autostart::ManagerExt;
    use tauri_plugin_notification::NotificationExt;

    struct TauriSink(tauri::AppHandle);

    impl EventSink for TauriSink {
        fn emit(&self, event: &str, payload: serde_json::Value) {
            let _ = self.0.emit(event, payload);
        }

        fn notify(&self, title: &str, body: &str) {
            let _ = self
                .0
                .notification()
                .builder()
                .title(title)
                .body(body)
                .show();
        }
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--silent"]),
        ))
        .setup(|app| {
            let handle = app.handle().clone();
            let paths = benv::paths::AppPaths::resolve(desktop_home_arg())
                .map_err(|e| Box::<dyn std::error::Error>::from(e.to_string()))?;

            let ctx = AppContext::new(paths, Arc::new(TauriSink(handle.clone())))
                .map_err(|e| Box::<dyn std::error::Error>::from(e.to_string()))?;
            ctx.reconcile()
                .map_err(|e| Box::<dyn std::error::Error>::from(e.to_string()))?;
            let settings = ctx.settings().unwrap_or_default();

            let filter = std::env::var("SVCBOX_LOG").unwrap_or_else(|_| settings.log_level.clone());
            let filter = tracing_subscriber::EnvFilter::try_new(&filter)
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
            let file_appender = tracing_appender::rolling::daily(&ctx.paths.logs, "svcbox.log");
            let _ = tracing_subscriber::fmt()
                .with_writer(file_appender)
                .with_ansi(false)
                .with_env_filter(filter)
                .try_init();

            if settings.autostart {
                let _ = app.autolaunch().enable();
            } else {
                let _ = app.autolaunch().disable();
            }

            if settings.http_api_enabled {
                let api_ctx = ctx.clone();
                let port = settings.http_api_port;
                tauri::async_runtime::spawn(async move {
                    if let Err(e) = benv::api::serve(api_ctx, port).await {
                        tracing::error!("HTTP API 启动失败: {e}");
                    }
                });
            }

            let renew_ctx = ctx.clone();
            app.manage(ctx.clone());
            tauri::async_runtime::spawn(benv::core::cert::renew_loop(renew_ctx));

            let open = MenuItem::with_id(app, "open", "打开面板", true, None::<&str>)?;
            let start_all = MenuItem::with_id(app, "start-all", "全部启动", true, None::<&str>)?;
            let stop_all = MenuItem::with_id(app, "stop-all", "全部停止", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &start_all, &stop_all, &quit])?;

            TrayIconBuilder::with_id("main-tray")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .icon(app.default_window_icon().unwrap().clone())
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "start-all" => {
                        let ctx = app.state::<Arc<AppContext>>().inner().clone();
                        tauri::async_runtime::spawn(async move {
                            let errors = benv::core::instance::start_all(&ctx).await;
                            for (id, err) in errors {
                                tracing::warn!("启动实例 {id} 失败: {err}");
                            }
                        });
                    }
                    "stop-all" => {
                        let ctx = app.state::<Arc<AppContext>>().inner().clone();
                        tauri::async_runtime::spawn(async move {
                            let errors = benv::core::instance::stop_all(&ctx).await;
                            for (id, err) in errors {
                                tracing::warn!("停止实例 {id} 失败: {err}");
                            }
                        });
                    }
                    "quit" => {
                        let app = app.clone();
                        let ctx = app.state::<Arc<AppContext>>().inner().clone();
                        tauri::async_runtime::spawn(async move {
                            // 显式退出时连 CLI 守护进程托管的实例一起停掉
                            let _ = benv::core::instance::stop_all(&ctx).await;
                            app.exit(0);
                        });
                    }
                    _ => {}
                })
                .build(app)?;

            if std::env::args().any(|a| a == "--silent") {
                if let Some(win) = app.get_webview_window("main") {
                    let _ = win.hide();
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            benv::commands::get_settings,
            benv::commands::update_settings,
            benv::commands::get_paths,
            benv::commands::get_doctor,
            benv::commands::list_versions,
            benv::commands::refresh_manifest,
            benv::commands::prune_versions,
            benv::commands::install_version,
            benv::commands::remove_version,
            benv::commands::import_version,
            benv::commands::list_instances,
            benv::commands::get_instance,
            benv::commands::create_instance,
            benv::commands::start_instance,
            benv::commands::stop_instance,
            benv::commands::restart_instance,
            benv::commands::remove_instance,
            benv::commands::read_logs,
            benv::commands::read_raw_config,
            benv::commands::save_raw_config,
            benv::commands::start_all_instances,
            benv::commands::stop_all_instances,
            benv::commands::list_nginx_sites,
            benv::commands::create_nginx_site,
            benv::commands::update_nginx_site,
            benv::commands::delete_nginx_site,
            benv::commands::reload_nginx,
            benv::commands::get_nginx_global,
            benv::commands::update_nginx_global,
            benv::commands::apply_site_https,
            benv::commands::renew_certificate,
            benv::commands::list_mysql_dbs,
            benv::commands::create_mysql_db,
            benv::commands::drop_mysql_db,
            benv::commands::list_mysql_users,
            benv::commands::create_mysql_user,
            benv::commands::alter_mysql_user_password,
            benv::commands::drop_mysql_user,
            benv::commands::reset_mysql_root_password,
            benv::commands::get_redis_config,
            benv::commands::update_redis_config,
            benv::commands::list_certificates,
        ])
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
                let state = window.app_handle().state::<Arc<AppContext>>();
                if state
                    .settings()
                    .map(|s| s.stop_instances_on_close)
                    .unwrap_or(false)
                {
                    let ctx = state.inner().clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = benv::core::instance::stop_all(&ctx).await;
                    });
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("svcbox 启动失败");
}
