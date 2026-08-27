# Agent Note: Desktop hub port resilience

Status: implemented

English | [中文](2026-08-27-desktop-hub-port-resilience.zh.md)

## Problem

The shell and the bundled `dsh web` hard-coded port 3081 in three places: the spawn argument, the iframe address, and the CSP. An unrelated process holding 3081 made the hub fail to bind and restart-loop into the "dsh crashed repeatedly" banner, and an unrelated HTTP service on 3081 was treated as "ready" because the readiness probe only checks TCP connectivity.

## Decision

Port selection moves into the supervisor and runs on every spawn: 3081 when it accepts a bind, otherwise the first bindable port in the 50-port range above it. `spawn_child` returns the chosen port; the supervisor threads it through `wait_ready`, the port-takeover check, and the `dsh://ready` / `dsh://restarted-by-plugin` event payloads. The frontend stops hard-coding the origin: it probes `dsh_status` for an already-running hub and otherwise loads the iframe from the ready-event port, keeping a plain 3081 fallback only for browser previews without the Tauri bridge. The build-time CSP widens to loopback port wildcards (`http://127.0.0.1:*`, `ws://127.0.0.1:*`) because a compiled-in CSP cannot change at runtime. A restart returns to 3081 as soon as the occupier releases it; nothing is persisted.

The probe is a bind, dropped before the child binds; the small race is absorbed by the existing not-ready restart path. No HTTP probe distinguishes a dsh from another occupier — the shell picks a free port whenever 3081 is busy and logs the choice in `desktop-startup.log`.

## Alternatives considered

**Probe whether the occupier is dsh.** Rejected: distinguishing responses is fragile, and the double-instance case is diagnosable from the startup log.

**Keep the CSP at exactly 3081 and rewrite it at runtime.** Rejected: Tauri embeds the CSP into the binary at build time.

**Persist the chosen port.** Rejected: re-evaluating per spawn naturally returns to 3081 when free.

## Verification

`cargo test` covers `select_port` returning the default when free and skipping occupied ports; `cargo check`, `node --check` on the shell page and bundle script, and the vitest plugin suite pass. Desktop installs validate the end-to-end flow.

## Consequences

The hub survives an occupied default port and the shell always loads the UI at the port the hub actually serves. An occupied 3081 serving an unrelated service no longer renders that service in the window. The scan range is bounded to 50 ports; an exhausted range fails loud through the startup log and the error banner.
