# Agent Note: CLI pnpm resolution fallback

Status: implemented

English | [中文](2026-08-27-cli-pnpm-resolution-fallback.zh.md)

## Problem

`dsh plugin --profile web add dsh-plugin` failed with `pnpm not found on PATH` and exit 127 whenever the invoking shell lacked pnpm. The desktop bundle ships pnpm@11.7.0 (bundle-runtime installs the package into `runtime/node/node_modules/pnpm`) but never created executable shims for it, and the CLI resolved pnpm solely through PATH with no fallback.

## Decision

Two halves close the gap.

**Ship the bundled pnpm shims.** bundle-runtime writes `pnpm`/`pnpx` shims into `runtime/node/node_modules/.bin` after the package copy: a POSIX shim resolving the node binary relative to itself (`../../node` after the installNode flatten) and a `.cmd` shim using `%~dp0\..\..\node.exe`. Relative resolution keeps the runtime relocatable between the build host and the installed bundle. The shell already prepends that `.bin` directory to the spawned `dsh web` PATH, so the market finds pnpm too.

**Resolve pnpm in the CLI.** `resolvePnpm()` probes candidates in order — `PNPM_BINARY` override, `pnpm` on PATH, then the runtime-sibling shim `<runtime>/node/node_modules/.bin/pnpm` derived from this CLI's `INSTALL_ANCHOR` (two `dirname`s up from `runtime/dsh/package.json`) — accepting the first whose `--version` probe exits 0. When nothing resolves, the guidance names `npm install -g pnpm` or `corepack enable pnpm` before returning 127. The `$DSH_HOME/node_modules` junction is not a candidate: it points at the deploy closure's node_modules, which contains no pnpm. Shell-mode probes concatenate the flag into the command string (avoiding the DEP0190 warning) and quote paths with spaces.

## Alternatives considered

**Write the shims into the closure's `.bin` too.** Rejected: the closure has no pnpm package for a relative shim to invoke, and copying pnpm into the deploy closure duplicates the runtime copy for no reachability gain.

**Automatic `corepack pnpm` fallback.** Rejected: strict-mode corepack needs a `packageManager` field and can fetch an unpinned pnpm, conflicting with the runtime's pinned 11.7.0.

## Verification

The vitest suite covers `PNPM_BINARY` precedence, all-missing undefined, and `runtimePnpmCandidate` resolving the deployed layout while returning undefined for an npm-global layout. `tsc --noEmit` and `node --check` on the bundle script pass.

## Consequences

A desktop-runtime CLI installs plugins without a global pnpm, using the same pinned version the market uses. Plain npm-global installs without pnpm still exit 127 but with install guidance.
