---
feature: cli-pnpm-resolution
status: designed
updated: 2026-08-27
branch: feat/desktop-port-pnpm
commits: <base-sha>..<head-sha> # filled at delivery
---

# CLI pnpm Resolution

## Report

## [S1] Problem

`dsh plugin --profile web add dsh-plugin` fails with `pnpm not found on PATH — install pnpm to manage profile plugins` and exit 127 on machines whose PATH has no pnpm (the reporter's Intel Mac). The desktop runtime does bundle `pnpm@11.7.0` (bundle-runtime installs the package into `runtime/node/node_modules/pnpm`), but:

- bundle-runtime never creates the `.bin` shims, so nothing on the desktop PATH chain can execute the bundled pnpm;
- `apps/cli/src/plugin.ts:runPlugin` resolves pnpm solely through `spawnSync('pnpm', ...)` against PATH, with no fallback and only a bare error line.

The profile plugin manager therefore works only on developer machines with a global pnpm, and the desktop market's plugin installs are similarly at risk.

## [S2] Design

### Ship the bundled pnpm shims (`apps/desktop/scripts/bundle-runtime.mjs`)

After the `cpSync` of the pnpm package into `runtime/node/node_modules/pnpm`, write executable shims into `runtime/node/node_modules/.bin`:

- `pnpm` (POSIX): a standard npm-style shim resolving `../pnpm/bin/pnpm.cjs` via the shim's own directory, so the runtime stays relocatable;
- `pnpm.cmd` (Windows): `@node "%~dp0\..\pnpm\bin\pnpm.cjs" %*`;
- `pnpx` / `pnpx.cmd` shims for the companion binary.

The shell already prepends `runtime/node/node_modules/.bin` to the spawned `dsh web` PATH, so the market finds pnpm; the CLI finds it through the `$DSH_HOME` fallback below.

### Resolve pnpm in the CLI (`apps/cli/src/plugin.ts`)

New `resolvePnpm()` returns the command to run or `undefined`. Candidate order:

1. `PNPM_BINARY` environment variable (explicit override; run as given, no shell on POSIX);
2. `pnpm` on PATH (current behaviour);
3. `$DSH_HOME/node_modules/.bin/pnpm` (`.cmd` on Windows) — the desktop runtime junction created by `ensure_home_node_modules` exposes the shims above.

A candidate is accepted when a probe `spawnSync(command, ['--version'], { shell: platform === 'win32', stdio: 'ignore' })` does not error and exits 0; PATH candidates cannot be `existsSync`-checked, so the probe is the single acceptance test. `$DSH_HOME` resolves through `@deepseek-ai/dsh-home-paths` (`resolveDshHome`), already a dependency of the app (`profile-boot.ts` imports it).

`runPlugin` uses the resolved command (with `shell` set for Windows `.cmd` shims, as today) and, when nothing resolves, prints actionable guidance — `npm install -g pnpm` or `corepack enable pnpm` — before returning 127.

### Error behaviour

- All candidates missing: exit 127 with the guidance line (unchanged code, better message).
- `PNPM_BINARY` set but broken (probe fails): fall through to the next candidate, do not fail hard.
- A selected candidate that later fails (non-zero exit) keeps today's diagnostics (profile dir + git allowBuilds hint).

## [S3] Out of Scope

- No automatic `corepack pnpm` fallback: strict-mode corepack needs a `packageManager` field and can download an unpinned pnpm, which conflicts with the runtime's pinned 11.7.0. Guidance text names corepack instead.
- No change to the market's own pnpm invocation inside the dsh web process: it inherits the improved PATH from the shell, which now contains real shims.
- pnpm version alignment stays bundle-runtime's `pnpmVersion` constant; the CLI does not pin or verify a version.

## Tasks

- [ ] T1: bundle-runtime shims — acceptance: bundle-runtime.mjs writes pnpm/pnpx shims (POSIX + .cmd) after the pnpm package copy; `node --check bundle-runtime.mjs` clean (covers: S2 Ship the bundled pnpm shims)
- [ ] T2: CLI resolvePnpm candidate chain — acceptance: plugin.ts probes PNPM_BINARY → PATH → `$DSH_HOME/node_modules/.bin/pnpm`, uses the first that works, and prints actionable guidance when none does (covers: S2 Resolve pnpm in the CLI)
- [ ] T3: plugin resolution tests — acceptance: apps/cli/tests/plugin.spec.ts covers PNPM_BINARY precedence, DSH_HOME fallback, all-missing guidance, and probe rejection of a broken candidate; `pnpm --filter @deepseek-ai/dsh test` passes (covers: S2)
