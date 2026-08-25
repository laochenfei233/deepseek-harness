//! First-run wizard support: detect initialization, choose the default agent
//! preset (written into the web profile's `cordis.patch.yml` through the
//! official patch layer), and install plugins through the `dsh plugin` CLI
//! (with a local-copy path for plugins that ship inside the runtime bundle).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;
use serde_yaml::Value as Yaml;
use tauri::{AppHandle, Manager};

use crate::dsh_runner::resolve_runtime;

const PRESET_ROW_ID: &str = "agent-presets";
const STATE_FILE: &str = "desktop.json";

pub fn dsh_home() -> PathBuf {
    std::env::var("DSH_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let base = std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOME"))
                .unwrap_or_default();
            PathBuf::from(base).join(".dsh")
        })
}

fn profile_web_dir(home: &Path) -> PathBuf {
    home.join("profiles").join("web")
}

fn state_path(home: &Path) -> PathBuf {
    home.join(STATE_FILE)
}

#[derive(Serialize)]
pub struct FirstRunState {
    pub initialized: bool,
    pub preset: String,
}

fn read_state(home: &Path) -> (bool, String) {
    let initialized = profile_web_dir(home).join("cordis.patch.yml").exists()
        || profile_web_dir(home).join("package.json").exists();
    let preset = fs::read_to_string(state_path(home))
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.get("preset").and_then(|p| p.as_str()).map(String::from))
        .unwrap_or_else(|| "standard".to_string());
    (initialized, preset)
}

#[tauri::command]
pub fn is_initialized() -> bool {
    read_state(&dsh_home()).0
}

#[tauri::command]
pub fn first_run_state() -> FirstRunState {
    let (initialized, preset) = read_state(&dsh_home());
    FirstRunState { initialized, preset }
}

/// Writes the chosen preset into the profile patch (replacing the
/// `agent-presets` row's `config.default`) and records it in the shell state.
#[tauri::command]
pub fn set_preset(preset: String) -> Result<(), String> {
    let home = dsh_home();
    write_state(&home, &preset)?;
    write_preset_patch(&home, &preset)?;
    Ok(())
}

fn write_state(home: &Path, preset: &str) -> Result<(), String> {
    fs::create_dir_all(home).map_err(|e| e.to_string())?;
    let state = serde_json::json!({ "preset": preset });
    fs::write(state_path(home), serde_json::to_string_pretty(&state).unwrap()).map_err(|e| e.to_string())
}

fn write_preset_patch(home: &Path, preset: &str) -> Result<(), String> {
    let patch_path = profile_web_dir(home).join("cordis.patch.yml");
    fs::create_dir_all(patch_path.parent().unwrap()).map_err(|e| e.to_string())?;

    let mut rows: Vec<Yaml> = if patch_path.exists() {
        fs::read_to_string(&patch_path)
            .ok()
            .and_then(|raw| serde_yaml::from_str(&raw).ok())
            .unwrap_or_default()
    } else {
        Vec::new()
    };

    let mut found = false;
    for row in rows.iter_mut() {
        if row.get("id").and_then(Yaml::as_str) == Some(PRESET_ROW_ID) {
            if row.get_mut("config").is_none() {
                row["config"] = Yaml::Mapping(Default::default());
            }
            row["config"]["default"] = Yaml::String(preset.to_string());
            found = true;
            break;
        }
    }
    if !found {
        rows.push(serde_yaml::from_str(&format!(
            "id: {PRESET_ROW_ID}\nconfig:\n  default: {preset}\n"
        )).map_err(|e| format!("build patch row: {e}"))?);
    }

    fs::write(&patch_path, serde_yaml::to_string(&rows).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}

/// Installs one plugin. `spec` is either a package/Git spec passed to
/// `dsh plugin --profile web add <spec>`, or `local:<name>` for a plugin that
/// ships inside the bundled runtime (`runtime/plugins/<name>`) and is copied
/// into the profile with a patch `insert` row — the dsh-win-terminal-inspector
/// path, which has no npm distribution.
#[tauri::command]
pub async fn install_plugin(app: AppHandle, spec: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || install_plugin_sync(&app, &spec))
        .await
        .map_err(|e| e.to_string())?
}

fn install_plugin_sync(app: &AppHandle, spec: &str) -> Result<String, String> {
    if let Some(name) = spec.strip_prefix("local:") {
        return install_local_plugin(app, name);
    }
    let home = dsh_home();
    let (node, dsh_bin) = resolve_runtime(app)?;
    let profile_dir = profile_web_dir(&home);
    fs::create_dir_all(&profile_dir).map_err(|e| e.to_string())?;

    let mut cmd = Command::new(&node);
    no_window(&mut cmd);
    let output = cmd
        .arg(&dsh_bin)
        .arg("plugin")
        .arg("--profile")
        .arg("web")
        .arg("add")
        .arg(spec)
        .current_dir(profile_dir)
        .output()
        .map_err(|e| format!("failed to run dsh plugin add: {e}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if !output.status.success() {
        return Err(format!("dsh plugin add failed ({})\n{stdout}\n{stderr}", output.status));
    }
    Ok(stdout)
}

fn install_local_plugin(app: &AppHandle, name: &str) -> Result<String, String> {
    let runtime_plugins = runtime_plugins_dir(app)?;
    // The runtime plugin directory keeps the repo-style `dsh-` name prefix;
    // accept both the wizard's short id and the actual directory name.
    let src = ["dsh-", ""]
        .iter()
        .map(|prefix| runtime_plugins.join(format!("{prefix}{name}")))
        .find(|p| p.exists())
        .ok_or_else(|| format!("bundled plugin {name} not found in runtime"))?;
    let home = dsh_home();
    let dest = profile_web_dir(&home).join("plugins").join(name);
    fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
    copy_dir(&src, &dest)?;

    // Append a patch `insert` row unless one with the same id already exists.
    let patch_path = profile_web_dir(&home).join("cordis.patch.yml");
    let mut rows: Vec<Yaml> = fs::read_to_string(&patch_path)
        .ok()
        .and_then(|raw| serde_yaml::from_str(&raw).ok())
        .unwrap_or_default();
    let exists = rows.iter().any(|row| {
        row.get("insert")
            .and_then(|ins| ins.as_sequence())
            .is_some_and(|seq| {
                seq.iter().any(|entry| {
                    entry.get("id").and_then(Yaml::as_str) == Some("win-terminal-inspector")
                })
            })
    });
    if !exists {
        rows.push(serde_yaml::from_str(
            "insert:\n  - id: win-terminal-inspector\n    name: ./plugins/dsh-win-terminal-inspector/index.js\n",
        ).map_err(|e| format!("build patch row: {e}"))?);
        fs::write(&patch_path, serde_yaml::to_string(&rows).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    Ok(format!("installed {name}"))
}

/// Preinstalled default plugins (plugin market + Windows terminal
/// inspector). The shell copies them from the bundled runtime into the web
/// profile on first launch — the wizard no longer installs them.
pub(crate) fn ensure_default_plugins(app: &AppHandle) -> Result<(), String> {
    let home = dsh_home();
    let profile = profile_web_dir(&home);
    let runtime_plugins = match runtime_plugins_dir(app) {
        Ok(dir) => dir,
        Err(_) => return Ok(()), // dev without bundled plugins: nothing to preinstall
    };

    let defaults: &[(&str, &str)] = &[
        ("dshmarket", "./plugins/dshmarket/node_modules/dshmarket"),
        ("dsh-win-terminal-inspector", "./plugins/dsh-win-terminal-inspector/index.js"),
    ];
    for (dir_name, patch_name) in defaults {
        let src = runtime_plugins.join(dir_name);
        if !src.exists() {
            continue;
        }
        let dest = profile.join("plugins").join(dir_name);
        if !dest.exists() {
            fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
            copy_dir(&src, &dest)?;
        }
    }

    let patch_path = profile.join("cordis.patch.yml");
    let mut rows: Vec<Yaml> = fs::read_to_string(&patch_path)
        .ok()
        .and_then(|raw| serde_yaml::from_str(&raw).ok())
        .unwrap_or_default();
    if !has_patch_row(&rows, "dshmarket") {
        rows.push(serde_yaml::from_str(
            "insert:\n  - id: dshmarket\n    name: ./plugins/dshmarket/node_modules/dshmarket/lib/index.js\n",
        ).map_err(|e| format!("build patch row: {e}"))?);
    }
    if !has_patch_row(&rows, "win-terminal-inspector") {
        rows.push(serde_yaml::from_str(
            "insert:\n  - id: win-terminal-inspector\n    name: ./plugins/dsh-win-terminal-inspector/index.js\n",
        ).map_err(|e| format!("build patch row: {e}"))?);
    }
    fs::write(&patch_path, serde_yaml::to_string(&rows).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn has_patch_row(rows: &[Yaml], id: &str) -> bool {
    rows.iter().any(|row| {
        row.get("insert")
            .and_then(|ins| ins.as_sequence())
            .is_some_and(|seq| seq.iter().any(|e| e.get("id").and_then(Yaml::as_str) == Some(id)))
    })
}

/// The app is a GUI process; spawned children would flash a console window.
fn no_window(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
}

fn runtime_plugins_dir(app: &AppHandle) -> Result<PathBuf, String> {    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|e| format!("resource dir unavailable: {e}"))?;
    let bundled = resource_dir.join("runtime").join("plugins");
    if bundled.exists() {
        return Ok(bundled);
    }
    std::env::var("DSH_DESKTOP_PLUGINS_DIR")
        .map(PathBuf::from)
        .map_err(|_| "no bundled plugins dir; set DSH_DESKTOP_PLUGINS_DIR in dev".to_string())
}

fn copy_dir(src: &Path, dest: &Path) -> Result<(), String> {
    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let target = dest.join(entry.file_name());
        if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), &target).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// Runs the full first-run flow: set the preset, then install every selected
/// plugin in order. The frontend calls this after the user confirms the
/// wizard; each step's output streams back via the individual commands.
#[tauri::command]
pub async fn apply_first_run(app: AppHandle, preset: String, plugins: Vec<String>) -> Result<(), String> {
    set_preset(preset)?;
    for spec in plugins {
        install_plugin(app.clone(), spec).await?;
    }
    Ok(())
}
