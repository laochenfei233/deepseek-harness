//! Owns the bundled `dsh web` child process: spawn, port health probe,
//! automatic crash restart, and clean teardown on app exit.

use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::Arc;
use std::sync::Mutex as LaunchUrlMutex;
use std::time::Duration;

use serde::Serialize;
use tauri::webview::{
    cookie::{time::Duration as CookieDuration, SameSite},
    Cookie,
};
use tauri::{AppHandle, Emitter, Manager, Url};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use crate::INTERNAL_PORT;

const RESTART_LIMIT: u32 = 3;
const LOG_NAME: &str = "dsh-web.log";
/// Upper bound of the fallback port scan when `INTERNAL_PORT` is occupied.
const PORT_SCAN_LIMIT: u16 = 50;

/// Handle to the managed dsh child process. `child` is `None` while the
/// process is down and about to be (or already) restarted.
pub struct DshHandle {
    child: Arc<Mutex<Option<Child>>>,
    stopping: Arc<AtomicBool>,
    port: Arc<AtomicU16>,
    launch_url: Arc<LaunchUrlMutex<Option<String>>>,
    node_path: PathBuf,
    dsh_bin: PathBuf,
}

/// The authenticated Web origin handed to the desktop iframe.
#[derive(Clone, Serialize)]
pub struct DshEndpoint {
    pub url: Option<String>,
}

impl DshHandle {
    /// Resolves the bundled runtime and spawns the supervisor loop.
    pub fn spawn(app: &AppHandle) -> Result<Self, String> {
        let (node_path, dsh_bin) = resolve_runtime(app)?;
        let handle = Self {
            child: Arc::new(Mutex::new(None)),
            stopping: Arc::new(AtomicBool::new(false)),
            port: Arc::new(AtomicU16::new(INTERNAL_PORT)),
            launch_url: Arc::new(LaunchUrlMutex::new(None)),
            node_path,
            dsh_bin,
        };
        let loop_handle = Self {
            child: handle.child.clone(),
            stopping: handle.stopping.clone(),
            port: handle.port.clone(),
            launch_url: handle.launch_url.clone(),
            node_path: handle.node_path.clone(),
            dsh_bin: handle.dsh_bin.clone(),
        };
        let app_owned = (*app).clone();
        tauri::async_runtime::spawn(async move { loop_handle.supervise(app_owned).await });
        Ok(handle)
    }

    /// Supervision loop: spawn, wait for ready or exit, then restart on
    /// unexpected exit (unless another process already took the port, which
    /// happens when the dsh-plugin market restarts the server itself).
    async fn supervise(&self, app: AppHandle) {
        let mut restarts = 0u32;
        loop {
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }
            let (child, port) = match self.spawn_child(app.clone()).await {
                Ok(owned) => owned,
                Err(err) => {
                    let _ = app.emit("dsh://failed", err);
                    return;
                }
            };
            *self.child.lock().await = Some(child);
            self.port.store(port, Ordering::SeqCst);
            *self.launch_url.lock().expect("launch-url mutex") = None;

            let ready = {
                let mut guard = self.child.lock().await;
                match guard.as_mut() {
                    Some(child) => {
                        wait_ready(port, child, self.stopping.clone(), &self.launch_url).await
                    }
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
                *self.launch_url.lock().expect("launch-url mutex") = None;
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            let launch_url = self.launch_url.lock().expect("launch-url mutex").clone();
            let _ = app.emit("dsh://ready", DshEndpoint { url: launch_url });

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
            *self.launch_url.lock().expect("launch-url mutex") = None;
            if self.stopping.load(Ordering::SeqCst) {
                return;
            }

            // The plugin market restarts `dsh` as a detached process whose stdout
            // goes to its own log. Capture the launch URL from the bytes it adds;
            // a stale token in that log must never reach the iframe.
            if port_open(port) {
                let baseline = external_log_offset(port).await;
                let url = wait_external_launch_url(
                    port,
                    baseline,
                    self.stopping.clone(),
                    &self.launch_url,
                )
                .await;
                if self.stopping.load(Ordering::SeqCst) {
                    return;
                }
                let Some(url) = url else {
                    let _ = app.emit(
                        "dsh://failed",
                        "the plugin-restarted dsh did not print an authenticated URL",
                    );
                    wait_port_closed(port, self.stopping.clone()).await;
                    continue;
                };
                let _ = app.emit("dsh://ready", DshEndpoint { url: Some(url) });
                wait_port_closed(port, self.stopping.clone()).await;
                *self.launch_url.lock().expect("launch-url mutex") = None;
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

    async fn spawn_child(&self, app: AppHandle) -> Result<(Child, u16), String> {
        append_startup_log("spawn_child: begin");
        self.ensure_home_node_modules()?;
        crate::setup::ensure_default_plugins(&app)?;
        append_startup_log("spawn_child: default plugins ready");
        let port = match select_port() {
            Ok(port) => port,
            Err(err) => {
                append_startup_log(&format!("select_port failed: {err}"));
                return Err(err);
            }
        };
        let mut cmd = Command::new(&self.node_path);
        cmd.arg(&self.dsh_bin)
            .arg("web")
            .arg("--port")
            .arg(port.to_string())
            .arg("--no-open")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Run the CLI from its package root so relative config lookups work.
        let workdir = self
            .dsh_bin
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.to_path_buf());
        if let Some(root) = &workdir {
            cmd.current_dir(root);
        }
        // Put the bundled pnpm (runtime/node/node_modules/.bin) and the deploy
        // closure's bins first on PATH so `dsh plugin` works offline.
        let mut path_parts: Vec<String> = Vec::new();
        if let Some(node_dir) = self.node_path.parent() {
            path_parts.push(
                node_dir
                    .join("node_modules")
                    .join(".bin")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        if let Some(closure) = self.dsh_bin.parent().and_then(|p| p.parent()) {
            path_parts.push(
                closure
                    .join("node_modules")
                    .join(".bin")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        if let Ok(existing) = std::env::var("PATH") {
            // Split the inherited PATH back into its elements before joining:
            // join_paths rejects a single element containing the platform
            // separator (':' on POSIX, ';' on Windows), and the whole PATH
            // string always does.
            path_parts
                .extend(std::env::split_paths(&existing).map(|p| p.to_string_lossy().into_owned()));
        }
        // join_paths uses the platform separator (";" on Windows, ":" on
        // POSIX); a literal ";" join breaks every PATH lookup in the dsh web
        // process on mac/linux (osascript/npx/git all spawn with ENOENT).
        let path_value = std::env::join_paths(&path_parts).map_err(|e| e.to_string())?;
        cmd.env("PATH", &path_value);
        no_window(&mut cmd);
        // Startup log in the dsh home (provably writable — the profile patch
        // was just written there), appended to at every failure point. The
        // app log dir can silently fail on a partially-installed bundle and
        // has swallowed every diagnostic in this chain before.
        append_startup_log(&format!(
            "spawn: node={} bin={} cwd={:?} PATH={}",
            self.node_path.display(),
            self.dsh_bin.display(),
            workdir,
            path_value.to_string_lossy(),
        ));
        // Diagnostics: record exactly what is about to be spawned.
        if let Ok(log_dir) = app.path().app_log_dir() {
            let _ = std::fs::create_dir_all(&log_dir);
            let _ = std::fs::write(
                log_dir.join("spawn-debug.log"),
                format!(
                    "node={}\nbin={}\ncwd={:?}\nPATH={}\n",
                    self.node_path.display(),
                    self.dsh_bin.display(),
                    workdir,
                    path_value.to_string_lossy(),
                ),
            );
        }
        *self.launch_url.lock().expect("launch-url mutex") = None;
        let spawn_result = cmd.spawn();
        let mut child = match spawn_result {
            Ok(child) => child,
            Err(e) => {
                // Persist the failure for diagnostics: the GUI error banner is
                // easy to miss while the app is starting.
                append_startup_log(&format!(
                    "spawn FAILED: {e}\nnode={}\nbin={}",
                    self.node_path.display(),
                    self.dsh_bin.display()
                ));
                if let Ok(log_dir) = app.path().app_log_dir() {
                    let _ = std::fs::create_dir_all(&log_dir);
                    let _ = std::fs::write(
                        log_dir.join("spawn-error.log"),
                        format!(
                            "spawn dsh failed: {e}\nnode={}\nbin={}\n",
                            self.node_path.display(),
                            self.dsh_bin.display()
                        ),
                    );
                }
                return Err(format!("spawn dsh failed: {e}"));
            }
        };
        forward_stdout(
            child.stdout.take(),
            app.path().app_log_dir().ok().map(|d| d.join(LOG_NAME)),
            port,
            self.launch_url.clone(),
        );
        forward_to_log(
            child.stderr.take(),
            app.path()
                .app_log_dir()
                .ok()
                .map(|d| d.join("dsh-web.err.log")),
        );
        Ok((child, port))
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
        self.port.load(Ordering::SeqCst)
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
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            let mut mklink = std::process::Command::new("cmd");
            mklink.creation_flags(CREATE_NO_WINDOW);
            let status = mklink
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

/// Choose the port for the next `dsh web` spawn: the default `INTERNAL_PORT`
/// when it accepts a bind, otherwise the first free port in the scan range.
/// Selection runs on every spawn, so a restart returns to the default as soon
/// as the external occupier releases it. The probe listener is dropped before
/// the process binds, leaving a tiny bind race that the supervisor's
/// `wait_ready` restart path absorbs.
fn select_port() -> Result<u16, String> {
    if port_free(INTERNAL_PORT) {
        return Ok(INTERNAL_PORT);
    }
    let last = INTERNAL_PORT + PORT_SCAN_LIMIT;
    for port in (INTERNAL_PORT + 1)..=last {
        if port_free(port) {
            return Ok(port);
        }
    }
    Err(format!(
        "no free port in {}..={last} for the dsh web",
        INTERNAL_PORT + 1
    ))
}

/// A port is free when a loopback bind succeeds; the listener is dropped
/// immediately, leaving the port for the spawned process.
fn port_free(port: u16) -> bool {
    std::net::TcpListener::bind(("127.0.0.1", port)).is_ok()
}

async fn wait_ready(
    port: u16,
    child: &mut Child,
    stopping: Arc<AtomicBool>,
    launch_url: &Arc<LaunchUrlMutex<Option<String>>>,
) -> bool {
    let mut attempts = 0u32;
    while attempts < 120 {
        if stopping.load(Ordering::SeqCst) {
            return false;
        }
        let authenticated = launch_url.lock().expect("launch-url mutex").is_some();
        if authenticated && port_open(port) {
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

/// Extract the loopback launch URL printed by `dsh web`; it is the only
/// stdout value the desktop shell may trust.
fn launch_url_from_line(line: &str, port: u16) -> Option<String> {
    let candidate = line
        .split_whitespace()
        .find(|value| value.starts_with("http://") && value.contains("token="))?;
    let url = Url::parse(candidate).ok()?;
    if url.scheme() != "http"
        || url.host_str() != Some("127.0.0.1")
        || url.port() != Some(port)
        || url.query_pairs().filter(|(key, _)| key == "token").count() != 1
    {
        return None;
    }
    Some(candidate.to_owned())
}

/// Forward stdout while retaining the current process's launch URL.
fn forward_stdout<S>(
    stream: Option<S>,
    path: Option<PathBuf>,
    port: u16,
    launch_url: Arc<LaunchUrlMutex<Option<String>>>,
) where
    S: AsyncRead + Unpin + Send + 'static,
{
    let Some(stream) = stream else { return };
    let Some(path) = path else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
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
        let mut lines = tokio::io::BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(url) = launch_url_from_line(&line, port) {
                *launch_url.lock().expect("launch-url mutex") = Some(url);
            }
            let _ = log.write_all(format!("{line}\n").as_bytes()).await;
        }
    });
}

/// Byte offset of the plugin market's restart log, so its next process can add
/// a new URL without the reader accepting a token from an older process.
async fn external_log_offset(port: u16) -> u64 {
    let path = crate::setup::dsh_home()
        .join("logs")
        .join(format!("dsh-web-{port}.log"));
    tokio::fs::metadata(&path)
        .await
        .map(|metadata| metadata.len())
        .unwrap_or(0)
}

/// Read only the bytes added to the plugin restart log after `offset`.
async fn wait_external_launch_url(
    port: u16,
    offset: u64,
    stopping: Arc<AtomicBool>,
    launch_url: &Arc<LaunchUrlMutex<Option<String>>>,
) -> Option<String> {
    let path = crate::setup::dsh_home()
        .join("logs")
        .join(format!("dsh-web-{port}.log"));
    for _ in 0..120 {
        if stopping.load(Ordering::SeqCst) {
            return None;
        }
        if let Ok(mut file) = tokio::fs::OpenOptions::new().read(true).open(&path).await {
            if let Ok(metadata) = file.metadata().await {
                if metadata.len() > offset {
                    if file.seek(std::io::SeekFrom::Start(offset)).await.is_ok() {
                        let mut added = String::new();
                        if file.read_to_string(&mut added).await.is_ok() {
                            let url = added
                                .lines()
                                .rev()
                                .find_map(|line| launch_url_from_line(line, port));
                            if let Some(url) = url {
                                *launch_url.lock().expect("launch-url mutex") = Some(url.clone());
                                return Some(url);
                            }
                        }
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    None
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
        return Ok((plain_path(&bundled_node), plain_path(&bundled_bin)));
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
    Ok((
        plain_path(&PathBuf::from(node)),
        plain_path(&PathBuf::from(dsh_bin)),
    ))
}

/// The app is a GUI process (windows_subsystem = windows); spawned children
/// otherwise create a visible console window. Suppress it.
fn no_window(cmd: &mut tokio::process::Command) {
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

/// Append one line to `~/.dsh/desktop-startup.log`, which the shell can
/// write even when the app log dir is unavailable; never fails the caller.
fn append_startup_log(message: &str) {
    let startup_log = crate::setup::dsh_home().join("desktop-startup.log");
    let _ = std::fs::create_dir_all(&crate::setup::dsh_home());
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&startup_log)
        .and_then(|mut file| {
            use std::io::Write;
            writeln!(file, "{message}")
        });
}

/// Windows verbatim paths (`\\?\C:\...`, produced by canonicalization inside
/// Tauri's resource_dir) crash node's main-script resolver with
/// `EISDIR ... lstat 'C:'`. Strip the prefix so node receives a normal path.
fn plain_path(p: &Path) -> PathBuf {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        p.to_path_buf()
    }
}

/// Cookie fields needed to install the launch exchange result in WebView2.
#[derive(Debug)]
struct AuthCookie {
    name: String,
    value: String,
    max_age: i64,
}

/// The clean Web origin to load after the launch-token exchange.
#[derive(Serialize)]
pub struct WebviewSession {
    pub url: String,
}

/// Read the sole dsh browser-session cookie from the token-exchange response.
fn auth_cookie_from_response(response: &str) -> Option<AuthCookie> {
    let header_block = response.split("\r\n\r\n").next()?;
    let set_cookie = header_block.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("set-cookie")
            .then(|| value.trim())
    })?;
    let (name, value) = set_cookie.split(';').next()?.split_once('=')?;
    let name = name.trim();
    if !name.starts_with("dsh-auth-") {
        return None;
    }
    let max_age = set_cookie
        .split(';')
        .find_map(|attribute| {
            let (key, value) = attribute.trim().split_once('=')?;
            key.eq_ignore_ascii_case("max-age")
                .then(|| value.trim().parse::<i64>().ok())
                .flatten()
        })
        .unwrap_or(30 * 24 * 60 * 60);
    Some(AuthCookie {
        name: name.to_owned(),
        value: value.trim().to_owned(),
        max_age,
    })
}

/// Exchange the one-time URL token for a browser cookie in the native store.
/// The cookie is made usable by the embedded cross-origin iframe; a strict
/// cookie minted by an iframe would not be sent back to the loopback server.
#[tauri::command]
pub async fn authenticate_webview(app: AppHandle, url: String) -> Result<WebviewSession, String> {
    let parsed = url
        .parse::<Url>()
        .ok()
        .filter(|parsed| {
            parsed.scheme() == "http"
                && parsed.host_str() == Some("127.0.0.1")
                && parsed.port().is_some()
        })
        .ok_or("invalid dsh web launch URL")?;
    let (cookie_parts, clean_url) = tauri::async_runtime::spawn_blocking(move || {
        let port = parsed.port().ok_or("invalid dsh web launch URL")?;
        let target = match parsed.query() {
            Some(query) => format!("{}?{query}", parsed.path()),
            None => parsed.path().to_owned(),
        };
        let mut stream = TcpStream::connect(("127.0.0.1", port))
            .map_err(|e| format!("launch-token exchange failed: {e}"))?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| format!("launch-token exchange failed: {e}"))?;
        use std::io::Write;
        stream
            .write_all(
                format!(
                    "GET {target} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .map_err(|e| format!("launch-token exchange failed: {e}"))?;
        let mut response = String::new();
        std::io::Read::read_to_string(&mut stream, &mut response)
            .map_err(|e| format!("launch-token exchange failed: {e}"))?;
        let cookie = auth_cookie_from_response(&response)
            .ok_or_else(|| "launch-token response contained no dsh browser cookie".to_owned())?;
        let mut clean = parsed;
        clean.set_query(None);
        clean.set_fragment(None);
        Ok::<_, String>((cookie, clean.to_string()))
    })
    .await
    .map_err(|e| format!("launch-token exchange task failed: {e}"))??;
    let cookie = Cookie::build((cookie_parts.name, cookie_parts.value))
        .domain("127.0.0.1")
        .path("/")
        .http_only(true)
        .same_site(SameSite::None)
        .secure(true)
        .max_age(CookieDuration::seconds(cookie_parts.max_age))
        .build();
    let window = app
        .get_webview_window("main")
        .ok_or("main webview unavailable")?;
    window
        .set_cookie(cookie)
        .map_err(|e| format!("browser-cookie installation failed: {e}"))?;
    Ok(WebviewSession { url: clean_url })
}
#[derive(Serialize)]
pub struct DshStatus {
    pub running: bool,
    pub port: u16,
    pub url: Option<String>,
}

#[tauri::command]
pub fn dsh_status(state: tauri::State<'_, DshHandle>) -> DshStatus {
    DshStatus {
        running: port_open(state.port()),
        port: state.port(),
        url: state.launch_url.lock().expect("launch-url mutex").clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_url_parser_accepts_only_the_current_loopback_token() {
        let good = launch_url_from_line(
            "dsh web: http://127.0.0.1:3081/?token=abc (LAN: http://192.0.2.1:3081/?token=abc)",
            3081,
        );
        assert_eq!(good.as_deref(), Some("http://127.0.0.1:3081/?token=abc"),);
        assert_eq!(
            launch_url_from_line("dsh web: http://127.0.0.1:3081/", 3081),
            None
        );
        assert_eq!(
            launch_url_from_line("dsh web: http://127.0.0.1:3082/?token=abc", 3081),
            None
        );
        assert_eq!(
            launch_url_from_line("dsh web: http://127.0.0.1:3081/?token=abc&token=def", 3081),
            None
        );
    }
    #[test]
    fn auth_cookie_parser_accepts_only_dsh_session_cookies() {
        let cookie = auth_cookie_from_response(
            "HTTP/1.1 200 OK\r\nSet-Cookie: dsh-auth-abc123=tok; Max-Age=604800; Path=/\r\n\r\nbody",
        )
        .unwrap();
        assert_eq!(cookie.name, "dsh-auth-abc123");
        assert_eq!(cookie.value, "tok");
        assert_eq!(cookie.max_age, 604800);

        // Only dsh-auth-* cookies may reach the WebView store.
        assert!(auth_cookie_from_response(
            "HTTP/1.1 200 OK\r\nSet-Cookie: other=tok; Max-Age=60\r\n\r\n",
        )
        .is_none());

        // A malformed Max-Age falls back to the session default rather than
        // installing an immediately expired cookie.
        let cookie = auth_cookie_from_response(
            "HTTP/1.1 200 OK\r\nSet-Cookie: dsh-auth-abc123=tok; Max-Age=soon\r\n\r\n",
        )
        .unwrap();
        assert_eq!(cookie.max_age, 30 * 24 * 60 * 60);
    }

    #[test]
    fn select_port_returns_default_when_free() {
        if port_free(INTERNAL_PORT) {
            assert_eq!(select_port().unwrap(), INTERNAL_PORT);
        }
    }

    #[test]
    fn select_port_skips_occupied_ports() {
        // When both the default and the first fallback are occupied, the scan
        // must move past them and return a port that is actually bindable.
        let default = std::net::TcpListener::bind(("127.0.0.1", INTERNAL_PORT));
        let next = std::net::TcpListener::bind(("127.0.0.1", INTERNAL_PORT + 1));
        let (Ok(default), Ok(next)) = (default, next) else {
            return;
        };
        let port = select_port().unwrap();
        assert_ne!(port, INTERNAL_PORT);
        assert_ne!(port, INTERNAL_PORT + 1);
        assert!(port_free(port));
        drop(default);
        drop(next);
    }

    #[test]
    fn join_paths_accepts_split_inherited_path() {
        // Regression for the hub-never-spawned bug: the spawn PATH builder
        // must split the inherited PATH into elements before joining. POSIX
        // join_paths rejects an element containing ':' — and the whole PATH
        // string always does — which silently killed the hub spawn on
        // mac/linux while Windows (no ';' check) kept working.
        let Ok(existing) = std::env::var("PATH") else {
            return;
        };
        #[cfg(not(windows))]
        assert!(std::env::join_paths([&existing]).is_err());
        let elements: Vec<_> = std::env::split_paths(&existing).collect();
        assert!(std::env::join_paths(elements).is_ok());
    }
}
