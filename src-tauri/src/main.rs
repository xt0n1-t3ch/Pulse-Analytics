#![windows_subsystem = "windows"]

use pulse::{accounts, commands, update_check};

use tauri::{
    Emitter, Listener, Manager,
    image::Image,
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
};

const TRAY_ID: &str = "pulse-tray";
const OPEN_NOTIFICATIONS_EVENT: &str = "pulse://open-notifications";

fn pulse_icon() -> Image<'static> {
    tauri::include_image!("../assets/icon.png")
}

fn show_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.set_title("Pulse");
        let _ = win.set_icon(pulse_icon());
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let _ = tray.set_visible(false);
    }
}

fn unread_notification_count() -> Result<u32, String> {
    let connection =
        pulse::notifications::open_default_database().map_err(|error| error.to_string())?;
    pulse::notifications::NotificationStore::new(&connection)
        .unread_count()
        .map_err(|error| error.to_string())
}

fn build_tray_menu<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    unread_count: u32,
) -> tauri::Result<tauri::menu::Menu<R>> {
    let show = MenuItemBuilder::with_id("show", "Show Pulse").build(app)?;
    let notifications = MenuItemBuilder::with_id(
        "notifications",
        pulse::notifications::tray_presentation(unread_count).menu_label,
    )
    .build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
    MenuBuilder::new(app)
        .items(&[&show, &notifications, &quit])
        .build()
}

fn refresh_tray_presentation(app: &tauri::AppHandle) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    let unread_count = match unread_notification_count() {
        Ok(count) => count,
        Err(error) => {
            tracing::warn!(%error, "failed to read Pulse unread notification count; preserving tray state");
            return;
        }
    };
    let presentation = pulse::notifications::tray_presentation(unread_count);
    if presentation.capabilities.tooltip {
        let _ = tray.set_tooltip(Some(presentation.tooltip.as_str()));
    }
    if presentation.capabilities.title {
        let _ = tray.set_title(Some(presentation.title.as_str()));
    }
    match build_tray_menu(app, unread_count) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(error) => tracing::warn!(%error, "failed to refresh Pulse tray menu"),
    }
    #[cfg(target_os = "macos")]
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.set_badge_label(presentation.badge_text);
    }
}

fn create_or_show_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        tray.set_visible(true)?;
        refresh_tray_presentation(app);
        return Ok(());
    }

    let unread_count = match unread_notification_count() {
        Ok(count) => count,
        Err(error) => {
            tracing::warn!(%error, "failed to read Pulse unread notification count; creating degraded tray");
            0
        }
    };
    let menu = build_tray_menu(app, unread_count)?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(pulse_icon())
        .tooltip("Pulse")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_window(app),
            "notifications" => {
                show_window(app);
                if let Err(error) = app.emit(OPEN_NOTIFICATIONS_EVENT, ()) {
                    tracing::warn!(%error, "failed to open Pulse notification center");
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| match event {
            // Restore on a single left-click (the common Windows gesture) as
            // well as a double-click; show_window is idempotent so the extra
            // Click emitted during a double-click is harmless.
            tauri::tray::TrayIconEvent::Click {
                button: tauri::tray::MouseButton::Left,
                button_state: tauri::tray::MouseButtonState::Up,
                ..
            }
            | tauri::tray::TrayIconEvent::DoubleClick { .. } => {
                show_window(tray.app_handle());
            }
            _ => {}
        })
        .build(app)?;
    refresh_tray_presentation(app);
    Ok(())
}

fn main() {
    if let Err(error) = cc_discord_presence::storage::initialize() {
        eprintln!("Pulse storage initialization failed: {error:#}");
        std::process::exit(1);
    }
    cc_discord_presence::power::apply_from_env("PULSE_EFFICIENCY_MODE");
    // Debug builds also serve the read-only dev bridge so the UI can be
    // reviewed in a browser against real backend data.
    #[cfg(debug_assertions)]
    pulse::dev_bridge::spawn();

    let builder = tauri::Builder::default();
    // Register single-instance FIRST, before every other plugin and the
    // poller. The repo-owned native E2E probe is the only exception: debug
    // builds with an isolated PULSE_E2E_RUN_ID must coexist with an installed
    // Pulse process so validation never has to stop the user's app.
    let builder = if native_e2e_probe_requested() {
        builder
    } else {
        builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_window(app);
        }))
    };

    builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        // Native Windows/macOS/Linux notifications are delivered by the
        // official Tauri plugin; the notification center persists its own
        // records so delivery failures never erase unread state.
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_shell::init())
        // In-app updates: downloads and installs signed releases without
        // sending the user to the GitHub releases page.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // Load persisted preferences into their synchronous mirror before
            // the window can emit a close event.
            pulse::app_settings::init();
            // A running Pulse GUI owns the analytics DB and Discord presence
            // outright. Reap any stray CLI / dev-bridge poller so nothing
            // double-writes in the background. Release-only: the dev
            // browser-review flow intentionally runs the standalone bridge.
            #[cfg(not(debug_assertions))]
            cc_discord_presence::process_guard::reap_stray_pollers();
            commands::start_background_poller(app.handle().clone());
            commands::refresh_usage();
            let tray_app = app.handle().clone();
            app.listen("pulse://snapshot", move |_| {
                refresh_tray_presentation(&tray_app);
            });
            let notification_tray_app = app.handle().clone();
            app.listen("pulse://notification", move |_| {
                refresh_tray_presentation(&notification_tray_app);
            });
            let notification_state_tray_app = app.handle().clone();
            app.listen("pulse://notification-state-changed", move |_| {
                refresh_tray_presentation(&notification_state_tray_app);
            });
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_title("Pulse");
                let _ = win.set_icon(pulse_icon());
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            accounts::list_accounts,
            accounts::discover_accounts,
            accounts::connect_account,
            accounts::cancel_account_connection,
            accounts::remove_account,
            accounts::refresh_account,
            commands::get_health,
            commands::get_app_snapshot,
            commands::get_access_snapshot,
            commands::get_notifications,
            commands::get_unread_notification_count,
            commands::mark_notification_read,
            commands::mark_all_notifications_read,
            commands::dismiss_notification,
            commands::mark_notification_unread,
            commands::mark_all_notifications_unread,
            commands::dismiss_all_notifications,
            commands::restore_notifications,
            commands::get_metrics,
            commands::get_live_sessions,
            commands::get_discord_preview,
            commands::get_discord_settings,
            commands::get_rate_limits,
            commands::refresh_usage,
            commands::get_discord_user,
            commands::set_discord_enabled,
            commands::set_discord_display_prefs,
            commands::set_discord_field_order,
            commands::set_codex_desktop_design,
            commands::get_plan_info,
            commands::set_plan_override,
            commands::get_active_provider,
            commands::get_provider_copy,
            commands::set_active_provider,
            commands::get_app_settings,
            commands::set_close_to_tray,
            commands::get_dashboard_bundle,
            commands::get_session_history,
            commands::get_session_history_filtered,
            commands::get_sessions_by_hour_range,
            commands::search_sessions,
            commands::get_daily_stats,
            commands::get_analytics_summary,
            commands::get_context_breakdown,
            commands::get_context_breakdowns,
            commands::get_sessions_context_usage,
            commands::get_project_stats,
            commands::get_hourly_activity,
            commands::get_top_sessions,
            commands::get_cost_forecast,
            commands::get_cost_totals,
            commands::get_costs_bundle,
            commands::get_budget_status,
            commands::set_budget,
            commands::get_model_distribution,
            commands::get_model_distribution_v2,
            commands::export_all_data,
            commands::clear_history,
            commands::get_db_size,
            commands::generate_html_report,
            commands::generate_markdown_report,
            commands::get_cache_health,
            commands::get_recommendations,
            commands::get_inflection_points,
            commands::get_model_routing,
            commands::get_tool_frequency,
            commands::get_trace_overview,
            commands::get_prompt_complexity,
            commands::get_session_health,
            commands::copy_fix_prompt,
            commands::get_reports_bundle,
            update_check::check_app_update,
            update_check::open_app_release_page,
        ])
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if !pulse::app_settings::close_to_tray_enabled() {
                    // Preference is "quit on close": exit the whole app so no
                    // poller/thread lingers in the background.
                    window.app_handle().exit(0);
                    return;
                }
                api.prevent_close();
                match create_or_show_tray(window.app_handle()) {
                    Ok(()) => {
                        let _ = window.hide();
                    }
                    Err(error) => {
                        tracing::warn!(%error, "failed to create Pulse tray; keeping window visible");
                        let _ = window.show();
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("failed to run Pulse");
}

#[cfg(debug_assertions)]
fn native_e2e_probe_requested() -> bool {
    std::env::var_os("PULSE_E2E_RUN_ID").is_some()
}

#[cfg(not(debug_assertions))]
fn native_e2e_probe_requested() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::OPEN_NOTIFICATIONS_EVENT;

    #[test]
    fn tray_notifications_have_a_dedicated_open_event() {
        assert_eq!(OPEN_NOTIFICATIONS_EVENT, "pulse://open-notifications");
    }
}
