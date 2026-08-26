//! DeepSeek Harness desktop shell.
//!
//! The shell embeds the dsh web UI (served by the bundled `dsh` process on the
//! loopback `INTERNAL_PORT`) inside a native window. It owns the dsh child
//! process lifecycle (spawn, health probe, crash restart), a tray icon that
//! keeps the app resident when the window closes, and the first-run wizard
//! that picks an agent preset and installs plugins.

mod dsh_runner;
mod setup;
mod tray;

use tauri::Manager;
use tauri::webview::{NewWindowFeatures, NewWindowResponse};
use tauri_plugin_opener::OpenerExt;

use dsh_runner::DshHandle;

/// Port the bundled `dsh web` process listens on. The webview is the only
/// client; CLI users who run their own `dsh web` keep the default 3080.
pub const INTERNAL_PORT: u16 = 3081;

/// The main window is declared in `tauri.conf.json` with `"create": false`
/// and built here instead: the `on_new_window` handler that routes popup
/// requests to the system browser is only reachable through the builder, and
/// the main window needs it as much as any window the app creates later.
fn build_main_window(app: &tauri::AppHandle) -> Result<(), String> {
    let window_config = app
        .config()
        .app
        .windows
        .iter()
        .find(|window| window.label == "main")
        .cloned()
        .ok_or_else(|| "No \"main\" window in the Tauri config".to_string())?;

    let handle = app.clone();
    tauri::WebviewWindowBuilder::from_config(app, &window_config)
        .map_err(|e| e.to_string())?
        .on_new_window(move |url, _features: NewWindowFeatures| {
            open_new_window_externally(&handle, &url)
        })
        .build()
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Schemes we hand to the OS opener. The URL comes from web content, so only
/// the well-known link schemes pass; anything else is refused outright.
fn is_externally_openable(url: &tauri::Url) -> bool {
    matches!(url.scheme(), "http" | "https" | "mailto" | "tel")
}

/// Send every popup request — `window.open` and `target="_blank"` alike, from
/// the shell or from inside the dsh iframe — to the user's browser and refuse
/// the popup itself.
///
/// Without a handler the platform webview drops these requests on the floor,
/// which is what made `target="_blank"` links dead in the app: WebView2 marks
/// the request handled with nothing to show, so the click does nothing at all.
fn open_new_window_externally(app: &tauri::AppHandle, url: &tauri::Url) -> NewWindowResponse<tauri::Wry> {
    if is_externally_openable(url) && app.opener().open_url(url.as_str(), None::<&str>).is_err() {
        eprintln!("[desktop] failed to open external URL in default browser: {url}");
    }
    NewWindowResponse::Deny
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            build_main_window(app.handle())?;
            let handle = DshHandle::spawn(app.handle())?;
            app.manage(handle);
            tray::build(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window hides the app to the tray; the dsh process
            // keeps serving. Only the tray "Quit" action exits for real.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            setup::is_initialized,
            setup::first_run_state,
            setup::set_preset,
            setup::install_plugin,
            setup::apply_first_run,
            dsh_runner::dsh_status,
            dsh_runner::restart_dsh,
            dsh_runner::open_ui,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::Exit = event {
                if let Some(dsh) = app_handle.try_state::<DshHandle>() {
                    dsh.stop();
                }
            }
        });
}
