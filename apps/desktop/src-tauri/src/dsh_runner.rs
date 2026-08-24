//! Owns the bundled `dsh web` child process: spawn, port health probe,
//! automatic crash restart, and clean teardown on app exit.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncRead;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::INTERNAL_PORT;

const RESTART_LIMIT: u32 = 3;
const LOG_NAME: &str = "dsh-web.log";

/// Handle to the managed dsh child process. `child` is `None` while the
/// process is down and about to be (or already) restarted.
pub struct DshHandle {
    child: Arc<Mutex<Option<Child>>>,
    stopping: Arc<AtomicBool>,
    node_path: PathBuf,
    dsh_bin: PathBuf,
}

impl DshHandle {
    /// Resolves the bundled runtime and spawns the supervisor loop.
    pub fn spawn(app: &AppHandle) -> Result<Self, String> {
        let (node_path, dsh_bin) = resolve_runtime(app)?;
        let handle = Self {
            child: Arc::new(Mutex::new(None)),
            stopping: Arc::new(AtomicBool::new(false)),
            node_path,
            dsh_bin,
        };
        let loop_handle = Self {
            child: handle.child.clone(),
            stopping: handle.stopping.clone(),
            node_path: handle.node_path.clone(),
            dsh_bin: handle.dsh_bin.clone(),
        };
        let app_owned = (*app).clone();
        tauri::async_runtime::spawn(async move { loop_handle.supervise(app_owned).await });
        Ok(handle)
    }

    /// Supervision loop: spawn, wait for ready or exit, then restart on
    /// unexpected exit (unless another process already took the port, which
    /// happens when dsh-market restarts the server itself).
    async fn supervise(&self, app: AppHandle) {
        let mut restarts = 0u32;
        loop {
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            let child = match self.spawn_child(app.clone()).await {
                Ok(child) => child,
                Err(err) => {
                    let _ = app.emit("dsh://failed", err);
                    return;
                }
            };
            *self.child.lock().await = Some(child);

            let ready = {
                let mut guard = self.child.lock().await;
                match guard.as_mut() {
                    Some(child) => wait_ready(INTERNAL_PORT, child, self.stopping.clone()).await,
                    None => false,
                }
            };
            if !ready {
                if self.stopping.load(Ordering::SeqCst) {
                    return;
                }
                // Process died before the port ever answered.
                let _ = app.emit("dsh://failed", "dsh exited during startup");
                self.child.lock().await.take();
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            let _ = app.emit("dsh://ready", ());

            // Wait for the child to exit (or the stopping flag).
            loop {
                if self.stopping.load(Ordering::SeqCst) {
                    return;
                }
                let exited = {
                    let mut guard = self.child.lock().await;
                    match guard.as_mut() {
                        Some(child) => child.try_wait().ok().flatten().is_some(),
                        None => true,
                    }
                };
                if exited {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            self.child.lock().await.take();
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }

            // dsh-market may restart the server itself; in that case the port
            // stays occupied and we must not spawn a rival.
            if port_open(INTERNAL_PORT) {
                let _ = app.emit("dsh://restarted-by-plugin", ());
                wait_port_closed(INTERNAL_PORT, self.stopping.clone()).await;
                continue;
            }
            if restarts >= RESTART_LIMIT {
                let _ = app.emit("dsh://failed", "dsh crashed repeatedly");
                return;
            }
            restarts += 1;
            let _ = app.emit("dsh://restarting", restarts);
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    async fn spawn_child(&self, app: AppHandle) -> Result<Child, String> {
        self.ensure_home_node_modules()?;
        let mut cmd = Command::new(&self.node_path);
        cmd.arg(&self.dsh_bin)
            .arg("web")
            .arg("--port")
            .arg(INTERNAL_PORT.to_string())
            .arg("--no-open")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Run the CLI from its package root so relative config lookups work.
        if let Some(root) = self.dsh_bin.parent().and_then(|p| p.parent()) {
            cmd.current_dir(root);
        }
        // Put the bundled pnpm (runtime/node/node_modules/.bin) and the deploy
        // closure's bins first on PATH so `dsh plugin` works offline.
        let mut path_parts: Vec<String> = Vec::new();
        if let Some(node_dir) = self.node_path.parent() {
            path_parts.push(node_dir.join("node_modules").join(".bin").to_string_lossy().into_owned());
        }
        if let Some(closure) = self.dsh_bin.parent().and_then(|p| p.parent()) {
            path_parts.push(closure.join("node_modules").join(".bin").to_string_lossy().into_owned());
        }
        if let Ok(existing) = std::env::var("PATH") {
            path_parts.push(existing);
        }
        cmd.env("PATH", path_parts.join(";"));
        let spawn_result = cmd.spawn();
        let mut child = match spawn_result {
            Ok(child) => child,
            Err(e) => {
                // Persist the failure for diagnostics: the GUI error banner is
                // easy to miss while the app is starting.
                if let Ok(log_dir) = app.path().app_log_dir() {
                    let _ = std::fs::create_dir_all(&log_dir);
                    let _ = std::fs::write(
                        log_dir.join("spawn-error.log"),
                        format!("spawn dsh failed: {e}\nnode={}\nbin={}\n", self.node_path.display(), self.dsh_bin.display()),
                    );
                }
                return Err(format!("spawn dsh failed: {e}"));
            }
        };
        forward_to_log(
            child.stdout.take(),
            app.path().app_log_dir().ok().map(|d| d.join(LOG_NAME)),
        );
        forward_to_log(
            child.stderr.take(),
            app.path().app_log_dir().ok().map(|d| d.join("dsh-web.err.log")),
        );
        Ok(child)
    }

    /// Stop the dsh process for good (app exit, tray Quit, or explicit
    /// restart request).
    pub fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let child = self.child.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(mut child) = child.lock().await.take() {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        });
    }

    /// Graceful restart requested from the UI ("Restart service").
    pub fn request_restart(&self) {
        let child = self.child.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(mut child) = child.lock().await.take() {
                let _ = child.kill().await;
                let _ = child.wait().await;
            }
        });
    }

    pub fn port(&self) -> u16 {
        INTERNAL_PORT
    }

    /// The web profile's loader resolves bundle rows (`@deepseek-ai/dsh-base`,
    /// `@deepseek-ai/dsh-web-app`) by walking node_modules upward from the
    /// profile directory. A normal npm install reaches them through the global
    /// node_modules chain; the bundled runtime has no such chain. Junction the
    /// closure's node_modules at `$DSH_HOME/node_modules` (a stop on that walk
    /// and outside the profile dir, so pnpm plugin management inside the
    /// profile never touches it). The junction target is the app's resource
    /// directory, so an app update replaces the closure in place and the
    /// junction keeps working.
    fn ensure_home_node_modules(&self) -> Result<(), String> {
        let home = crate::setup::dsh_home();
        let link = home.join("node_modules");
        let closure_root = self.dsh_bin.parent().and_then(|p| p.parent());
        let Some(closure_root) = closure_root else {
            return Err("cannot derive closure root from dsh bin path".into());
        };
        let target = closure_root.join("node_modules");
        if !target.exists() {
            return Err(format!("closure node_modules missing at {target:?}"));
        }
        if link.exists() {
            return Ok(());
        }
        std::fs::create_dir_all(&home).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        {
            // Directory junction (no admin rights needed, unlike symlink).
            let status = std::process::Command::new("cmd")
                .args(["/c", "mklink", "/J"])
                .arg(&link)
                .arg(&target)
                .status()
                .map_err(|e| format!("mklink failed: {e}"))?;
            if !status.success() {
                return Err(format!("failed to link DSH_HOME node_modules ({status})"));
            }
        }
        #[cfg(not(windows))]
        {
            std::os::unix::fs::symlink(&target, &link).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

fn port_open(port: u16) -> bool {
    std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
}

async fn wait_ready(port: u16, child: &mut Child, stopping: Arc<AtomicBool>) -> bool {
    let mut attempts = 0u32;
    while attempts < 120 {
        if stopping.load(Ordering::SeqCst) {
            return false;
        }
        if port_open(port) {
            return true;
        }
        if let Ok(Some(_)) = child.try_wait() {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        attempts += 1;
    }
    false
}

async fn wait_port_closed(port: u16, stopping: Arc<AtomicBool>) {
    loop {
        if stopping.load(Ordering::SeqCst) {
            return;
        }
        if !port_open(port) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn forward_to_log<S>(stream: Option<S>, path: Option<PathBuf>)
where
    S: AsyncRead + Unpin + Send + 'static,
{
    if stream.is_none() {
        return;
    }
    let Some(path) = path else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let stream = stream.expect("checked above");
    tauri::async_runtime::spawn(async move {
        let mut log = match tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
        {
            Ok(f) => f,
            Err(_) => return,
        };
        let mut reader = tokio::io::BufReader::new(stream);
        let _ = tokio::io::copy(&mut reader, &mut log).await;
    });
}

/// Resolves the Node executable and the dsh CLI entry.
///
/// Production layout (bundled by `scripts/bundle-runtime.mjs`):
///   <resource>/runtime/node/<node>          node binary
///   <resource>/runtime/dsh/.../lib/bin.js   @deepseek-ai/dsh CLI
///
/// Dev fallback: `node` from PATH and `DSH_DESKTOP_DSH_BIN`, defaulting to
/// the repo's built `apps/cli/lib/bin.js` when present.
pub(crate) fn resolve_runtime(app: &AppHandle) -> Result<(PathBuf, PathBuf), String> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("resource dir unavailable: {e}"))?;
    let runtime_root = resource_dir.join("runtime");

    let node_exe = if cfg!(windows) { "node.exe" } else { "node" };
    let bundled_node = runtime_root.join("node").join(node_exe);
    // pnpm deploy lays the target package out at the closure root: lib/bin.js.
    let bundled_bin = runtime_root.join("dsh").join("lib").join("bin.js");

    if bundled_node.exists() && bundled_bin.exists() {
        return Ok((bundled_node, bundled_bin));
    }

    // Dev fallback.
    let node = std::env::var("DSH_DESKTOP_NODE").unwrap_or_else(|_| "node".into());
    let dsh_bin = std::env::var("DSH_DESKTOP_DSH_BIN").unwrap_or_else(|_| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("cli")
            .join("lib")
            .join("bin.js")
            .to_string_lossy()
            .into_owned()
    });
    if !PathBuf::from(&dsh_bin).exists() {
        return Err(format!(
            "no bundled runtime found at {bundled_bin:?} and no built dsh CLI at {dsh_bin}"
        ));
    }
    Ok((PathBuf::from(node), PathBuf::from(dsh_bin)))
}

#[derive(Serialize)]
pub struct DshStatus {
    pub running: bool,
    pub port: u16,
}

#[tauri::command]
pub fn dsh_status(state: tauri::State<'_, DshHandle>) -> DshStatus {
    DshStatus {
        running: port_open(state.port()),
        port: state.port(),
    }
}

#[tauri::command]
pub fn restart_dsh(state: tauri::State<'_, DshHandle>) {
    state.request_restart();
}

#[tauri::command]
pub fn open_ui(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
    Ok(())
}
