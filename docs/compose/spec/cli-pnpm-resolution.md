---
feature: cli-pnpm-resolution
status: delivered
updated: 2026-08-27
branch: feat/desktop-port-pnpm
commits: bdabcc741..03012546d
---

# CLI pnpm Resolution

## Report

**What was built** — bundle-runtime writes `pnpm`/`pnpx` shims into `runtime/node/node_modules/.bin` after the pnpm package copy (POSIX shims resolving the node binary relatively, `.cmd` shims via `%~dp0`), so the market (through the shell's PATH prefix) and the CLI can execute the bundled pnpm. The CLI's `resolvePnpm()` probes candidates in order — `PNPM_BINARY` override, `pnpm` on PATH, then the runtime-sibling shim `<runtime>/node/node_modules/.bin/pnpm` derived from `INSTALL_ANCHOR` — accepting the first whose `--version` probe exits 0, and prints `npm install -g pnpm` / `corepack enable pnpm` guidance when none works. Review caught the dead `$DSH_HOME` junction candidate (the junction targets the deploy closure, which has no pnpm) and replaced it with the runtime-sibling shim.

**Verification** — `vitest run apps/cli/tests/plugin.spec.ts`: 4/4 pass; `tsc -p apps/cli/tsconfig.json --noEmit`: clean; `node --check apps/desktop/scripts/bundle-runtime.mjs`: clean; review re-check (general-4) confirmed the deployed layout math, bundle PATH, and test coverage.

**Journey log** — The `$DSH_HOME/node_modules` junction points at the deploy closure, not the node distribution — the shipped mechanism claim was wrong until review C1; the fix (runtime-sibling shim from `INSTALL_ANCHOR`) also removed the now-unused `dsh-home-paths` import. The probe's shell concatenation initially lost the `shell: true` flag and failed on Windows; quoting for spaced paths was added in the same pass.

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

The shell already prepends `runtime/node/node_modules/.bin` to the spawned `dsh web` PATH, so the market finds pnpm; the CLI finds the same shim through the runtime-sibling fallback below.

### Resolve pnpm in the CLI (`apps/cli/src/plugin.ts`)

New `resolvePnpm()` returns the command to run or `undefined`. Candidate order:

1. `PNPM_BINARY` environment variable (explicit override; shell on Windows, none on POSIX);
2. `pnpm` on PATH (current behaviour);
3. The runtime-sibling shim `<runtime>/node/node_modules/.bin/pnpm` (`.cmd` on Windows): this CLI's `INSTALL_ANCHOR` (its package.json) sits one level under the deployed closure root (`<runtime>/dsh/package.json`), so two `dirname`s reach `<runtime>`; the candidate is stat-checked and only added when the shim exists.

A candidate is accepted when a probe `pnpm --version` does not error and exits 0 (shell candidates concatenate the flag into the command string to avoid the DEP0190 shell-injection warning; paths with spaces are quoted); PATH candidates cannot be `existsSync`-checked, so the probe is the single acceptance test. The `$DSH_HOME/node_modules` junction is deliberately not a candidate: it targets the deploy closure's node_modules, which has no pnpm.

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

- [x] T1: bundle-runtime shims — acceptance: bundle-runtime.mjs writes pnpm/pnpx shims (POSIX + .cmd) after the pnpm package copy; `node --check bundle-runtime.mjs` clean (covers: S2 Ship the bundled pnpm shims)
- [x] T2: CLI resolvePnpm candidate chain — acceptance: plugin.ts probes PNPM_BINARY → PATH → runtime-sibling shim, uses the first that works, and prints actionable guidance when none does (covers: S2 Resolve pnpm in the CLI)
- [x] T3: plugin resolution tests — acceptance: apps/cli/tests/plugin.spec.ts covers PNPM_BINARY precedence, runtimePnpmCandidate layout resolution (deployed vs npm-global), and all-missing undefined; `vitest run apps/cli/tests/plugin.spec.ts` passes (covers: S2)
