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

use dsh_runner::DshHandle;
use tauri::Manager;

/// Port the bundled `dsh web` process listens on. The webview is the only
/// client; CLI users who run their own `dsh web` keep the default 3080.
pub const INTERNAL_PORT: u16 = 3081;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
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
