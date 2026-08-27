---
feature: desktop-port-resilience
status: designed
updated: 2026-08-27
branch: feat/desktop-port-pnpm
commits: <base-sha>..<head-sha> # filled at delivery
---

# Desktop Port Resilience

## Report

## [S1] Problem

The shell and the bundled `dsh web` process hard-code port `3081` (`INTERNAL_PORT` in `src-tauri/src/lib.rs`, `DSH_ORIGIN` in `ui/dist/app.js`, CSP in `tauri.conf.json`). When `3081` is occupied by an unrelated process:

- the spawned `dsh web` fails to bind (`EADDRINUSE`), exits during startup, and the supervisor restart loop ends in the "dsh crashed repeatedly" banner — the window never shows the UI;
- if an unrelated HTTP service happens to hold `3081`, `port_open` treats it as "ready", the shell emits `dsh://ready`, and the iframe renders whatever that service serves.

There is no mechanism to pick another free port and load it. The user explicitly requested: when the port is taken, the shell should find a free port and load the UI there.

## [S2] Design

Port selection happens per spawn inside the supervisor loop; the frontend learns the actual port from the `dsh://ready` event payload instead of a compile-time constant.

### Port selection (Rust, `dsh_runner.rs`)

- `fn select_port() -> Result<u16, String>`: when `INTERNAL_PORT` (3081) accepts a bind, use it; otherwise scan 50 ports starting at 3082 (3082..=3131) and return the first port whose bind succeeds (probe via `std::net::TcpListener::bind`, dropped before spawning). If none is free, return `Err` with a message naming the occupied range.
- `spawn_child` returns `(Child, u16)` — the child and the port it was told to listen on (`--port <port>` argument). Selection runs on every spawn, so after an external occupier releases 3081 a restart naturally returns to the default port.
- The supervisor (`supervise`) threads the port through: `wait_ready(port, ...)`, the readiness event `app.emit("dsh://ready", port)`, the port-takeover check (`port_open(port)` → `wait_port_closed(port)`), and the `dsh://restarted-by-plugin` event (payload = port).
- `DshHandle` gains `port: Arc<AtomicU16>` (initialised to `INTERNAL_PORT`, stored after each successful spawn); `port()` returns the current value, so `dsh_status` reports the live port and `running` probes it.
- The `dsh://failed` payload may include the attempted port for diagnostics.

### CSP (`tauri.conf.json`)

The CSP is compiled into the binary at build time and cannot change at runtime, so it must allow any loopback port instead of exactly 3081:

```
default-src 'self'; frame-src http://127.0.0.1:*; connect-src http://127.0.0.1:* ws://127.0.0.1:*; img-src 'self' http://127.0.0.1:* data: blob:; style-src 'self' 'unsafe-inline'; script-src 'self'
```

`host:*` port wildcards are valid CSP3 host-source syntax, supported by WebView2 (Chromium) and WKWebView (WebKit). Scope is loopback-only; no remote origin gains access.

### Frontend (`ui/dist/app.js`)

- `loadUi(port = 3081)` sets `frame.src = 'http://127.0.0.1:' + port`.
- In the Tauri environment the iframe is not loaded eagerly. The shell first asks `dsh_status` (which reports the live port and whether the hub answers) and loads immediately when the hub is already up; otherwise `dsh://ready` (payload = port) and `dsh://restarted-by-plugin` (payload = port) both call `loadUi(port)`. The iframe stays `about:blank` until the hub answers, which is the few hundred milliseconds the hub needs to start.
- The plain-browser fallback (Tauri bridge unavailable) keeps loading `http://127.0.0.1:3081` directly — the browser preview has no supervisor and no dynamic port.
- `dsh://failed` behavior is unchanged (error banner).

### Not changed

- The wizard flow (`wizard.html`/`wizard.js`) and `first_run_state` are untouched; a fresh install still goes through the wizard before any hub connection matters.
- The port-takeover semantics for the dsh-plugin market's own server restart (do not spawn a rival while the port stays occupied) are preserved, parameterised by the current port.
- CLI users running their own `dsh web` still default to 3080.

## [S3] Out of Scope

- No HTTP probe distinguishes "occupied by dsh" from "occupied by something else" at startup: a non-3081 port is selected whenever 3081 is busy, and the occupied 3081 is logged in `desktop-startup.log`. A double-instance scenario (the shell's new hub plus an external one) is diagnosable from that log and out of scope to arbitrate automatically.
- The external port occupier is never probed or probed for identity; `dsh_status` reports only the shell's own port.
- No port is persisted across restarts: selection is re-evaluated per spawn (3081 returns as soon as it is free).

## Tasks

- [ ] T1: Rust port selection + threaded supervisor events — acceptance: `select_port` unit test passes; `cargo check` and `cargo test` clean; ready/restarted-by-plugin events carry the chosen port (covers: S2 Port selection)
- [ ] T2: CSP port wildcard — acceptance: `tauri.conf.json` csp uses `http://127.0.0.1:*` / `ws://127.0.0.1:*`; `cargo check` clean (covers: S2 CSP)
- [ ] T3: Frontend dynamic port loading — acceptance: app.js loads the iframe from the event payload port, probes `dsh_status` for an already-running hub, and keeps the 3081 fallback for plain browsers; `node --check app.js` clean (covers: S2 Frontend)
- [ ] T4: README port contract update — acceptance: apps/desktop/README.md describes default-3081-with-fallback and event-driven port delivery (covers: S2)
