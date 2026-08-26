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
    let openable = is_externally_openable(url);
    if openable {
        let opened = app.opener().open_url(url.as_str(), None::<&str>).is_ok()
            || open_with_os(url.as_str()).is_ok();
        log_new_window(app, url, openable, opened);
    }
    NewWindowResponse::Deny
}

/// OS-level fallback for the opener plugin: the shell runs without a console,
/// so an opener failure is invisible; `cmd start` / `open` / `xdg-open` are
/// the same calls the plugin wraps.
fn open_with_os(url: &str) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .creation_flags(CREATE_NO_WINDOW)
            .status()
            .map(|_| ())
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(url).status().map(|_| ())
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        std::process::Command::new("xdg-open").arg(url).status().map(|_| ())
    }
}

/// Append one popup-routing record to the shell's debug log. The request is
/// denied by design, so this file is the only place a dead link leaves a trace.
fn log_new_window(app: &tauri::AppHandle, url: &tauri::Url, openable: bool, opened: bool) {
    if let Ok(log_dir) = app.path().app_log_dir() {
        let _ = std::fs::create_dir_all(&log_dir);
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join("new-window.log"))
        {
            use std::io::Write;
            use std::time::{SystemTime, UNIX_EPOCH};
            let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
            let _ = writeln!(file, "{ts} {url} openable={openable} opened={opened}");
        }
    }
}

/// The app is a GUI process, so every console child (cmd.exe from pnpm's
/// .cmd shims, the market's netstat/taskkill probes, git, node-gyp, …) would
/// otherwise create its own visible console window — Node's windowsHide
/// defaults to false and CREATE_NO_WINDOW does not propagate. Giving the app
/// a hidden console makes every descendant attach to it instead.
#[cfg(windows)]
fn hide_child_consoles() {
    unsafe extern "system" {
        fn AllocConsole() -> i32;
        fn GetConsoleWindow() -> *mut core::ffi::c_void;
        fn ShowWindow(hwnd: *mut core::ffi::c_void, n_cmd_show: i32) -> i32;
    }
    unsafe {
        if AllocConsole() != 0 {
            ShowWindow(GetConsoleWindow(), 0); // SW_HIDE
        }
    }
}

#[cfg(not(windows))]
fn hide_child_consoles() {}

pub fn run() {
    hide_child_consoles();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
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
