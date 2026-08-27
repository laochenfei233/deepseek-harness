/**
 * `dsh plugin --profile <name> <args...>` — profile plugin management as a
 * thin pnpm forwarder: initialize the profile on first use, run
 * `pnpm <args...>` in the profile directory, then reconcile the
 * `dsh.profile.bundles` layer list against the installed state (a dependency
 * resolving to a package that declares `dsh.bundle` joins the layer stack; a
 * removed or bundle-less dependency leaves it). Reconciling by installed
 * state, not by dependency diff, means `update` activates a package that
 * gained its `dsh.bundle` declaration in a newer version.
 * @module @deepseek-ai/dsh/plugin
 */

import { spawnSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import {
  DEFAULT_PROFILE_BUNDLES,
  initProfile,
  PROFILE_TEMPLATES,
  readProfileManifest,
  resolveBundleDir,
  resolveProfileDir,
  writeProfileManifest,
  type ProfileManifest,
} from '@deepseek-ai/dsh-app-boot'
import { INSTALL_ANCHOR } from './profile-boot.ts'

const NAME = 'dsh'

/** Environment variable naming the pnpm executable to use for plugin management. */
const PNPM_BINARY_ENV = 'PNPM_BINARY'

/** A resolved pnpm executable and whether spawning it needs a shell. */
export interface ResolvedPnpm {
  /** Command to run: a PATH name or an absolute shim path. */
  command: string
  /** Windows `.cmd` shims need a shell; POSIX absolute paths do not. */
  shell: boolean
}

/**
 * The desktop bundle's node distribution ships alongside the dsh closure at
 * `<runtime>/node` with a bundled pnpm in `<runtime>/node/node_modules/.bin`;
 * this CLI's `INSTALL_ANCHOR` (its package.json) sits one level under that
 * closure root in the deployed layout (`<runtime>/dsh/package.json`). Returns
 * the shim path in that layout, or undefined when the anchor layout is not the
 * deployed runtime one (a plain npm global install).
 * @param installAnchor - this CLI's package.json path (resolution anchor).
 * @returns the runtime pnpm shim path, or undefined when the layout does not match.
 */
export function runtimePnpmCandidate(installAnchor: string): string | undefined {
  const runtimeRoot = dirname(dirname(installAnchor))
  const shim = join(runtimeRoot, 'node', 'node_modules', '.bin', process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm')
  return existsSync(shim) ? shim : undefined
}

/**
 * Resolve the pnpm executable for profile plugin management. Candidate order:
 * `PNPM_BINARY` (explicit override), `pnpm` on PATH, then the desktop
 * runtime's bundled pnpm next to this CLI (`<runtime>/node/node_modules/.bin`,
 * which the desktop shell also prepends to the spawned `dsh web` PATH). A
 * candidate is accepted when `pnpm --version` runs; PATH candidates cannot be
 * stat-checked, so the probe is the single acceptance test. A broken
 * `PNPM_BINARY` falls through to the next candidate instead of failing the
 * invocation.
 * @returns the command to spawn, or undefined when no candidate works.
 */
export function resolvePnpm(): ResolvedPnpm | undefined {
  const candidates: ResolvedPnpm[] = []
  const envBinary = process.env[PNPM_BINARY_ENV]
  if (envBinary !== undefined && envBinary !== '') {
    candidates.push({ command: envBinary, shell: process.platform === 'win32' })
  }
  candidates.push({ command: 'pnpm', shell: process.platform === 'win32' })
  const runtimeShim = runtimePnpmCandidate(INSTALL_ANCHOR)
  if (runtimeShim !== undefined) {
    candidates.push({ command: runtimeShim, shell: process.platform === 'win32' })
  }
  for (const candidate of candidates) {
    // Shell candidates concatenate arguments into the command string: passing
    // args separately triggers the DEP0190 shell-injection warning and, on
    // Windows, .cmd shims ignore them anyway. Quote paths with spaces; the
    // no-shell probe spawns the path directly and must stay unquoted.
    const quoted = candidate.shell && /\s/.test(candidate.command) ? `"${candidate.command}"` : candidate.command
    const probe = candidate.shell
      ? spawnSync(`${quoted} --version`, { shell: true, stdio: 'ignore' })
      : spawnSync(candidate.command, ['--version'], { stdio: 'ignore' })
    if (probe.error === undefined && probe.status === 0) return candidate
  }
  return undefined
}

/**
 * Whether a resolved dependency exports a profile patch, i.e. is a bundle.
 * @param packageName - the dependency's package name.
 * @param profileDir - the profile directory (resolution anchor).
 * @returns true when the package manifest declares `dsh.bundle`.
 */
function exportsPatch(packageName: string, profileDir: string): boolean {
  let dir: string
  try {
    dir = resolveBundleDir(NAME, packageName, INSTALL_ANCHOR, profileDir)
  } catch {
    return false // pnpm reported success yet the package is unresolvable — treat as plain
  }
  const manifest = readProfileManifest(NAME, dir)
  return manifest.dsh?.bundle?.patch !== undefined
}

/**
 * Reconcile `dsh.profile.bundles` against the installed state: pnpm has
 * already written the real installed names (so a git/path/tarball/alias spec
 * on the command line reconciles by its true package name) and materialized
 * the packages. A dependency that resolves to a `dsh.bundle`-declaring
 * package joins the layer stack (appended in dependency order); a
 * dependency-listed name that no longer does — removed, or the installed
 * version dropped the declaration — leaves it. In-box bundles from the
 * profile template are not dependencies and are never touched. Warns once
 * per newly-added bundle-less dependency (a plain library is fine; the
 * warning is orientation).
 */
function reconcilePlugins(before: ProfileManifest, profileDir: string): void {
  const after = readProfileManifest(NAME, profileDir)
  const beforeDeps = new Set(Object.keys(before.dependencies ?? {}))
  const dependencies = Object.keys(after.dependencies ?? {})
  const plugins = after.dsh?.profile?.bundles ?? []
  let changed = false
  for (const packageName of dependencies) {
    const isBundle = exportsPatch(packageName, profileDir)
    if (isBundle && !plugins.includes(packageName)) {
      plugins.push(packageName)
      changed = true
    } else if (!isBundle && !beforeDeps.has(packageName)) {
      process.stderr.write(
        `${NAME}: warning: ${packageName} declares no dsh.bundle — installed as a plain dependency, not a profile layer `
        + '(a later update that gains one activates it automatically)\n',
      )
    }
  }
  const dependencySet = new Set(dependencies)
  for (const packageName of [...plugins]) {
    // Only dependency-managed entries are subject to removal; template
    // bundles (dsh-base and friends) are not dependencies.
    const wasDependency = beforeDeps.has(packageName) || dependencySet.has(packageName)
    const stillBundle = dependencySet.has(packageName) && exportsPatch(packageName, profileDir)
    if (wasDependency && !stillBundle) {
      plugins.splice(plugins.indexOf(packageName), 1)
      changed = true
    }
  }
  if (!changed) return
  after.dsh = { ...after.dsh, profile: { ...after.dsh?.profile, bundles: plugins } }
  writeProfileManifest(profileDir, after)
}

/**
 * Rewrite relative filesystem specs against the user's invoking directory.
 * pnpm runs with cwd = the profile directory, so a bare `.` or `../plugin`
 * (or their `file:`/`link:` forms) would silently resolve inside the profile
 * — `add .` from a plugin checkout would self-link the profile. Absolute
 * specs, registry names, and every other pnpm argument pass through
 * untouched.
 * @param argument - one pnpm argument, verbatim from argv.
 * @param cwd - the directory `dsh` was invoked from.
 * @returns the argument with a relative path spec anchored to `cwd`.
 */
function anchorPathSpec(argument: string, cwd: string): string {
  const match = /^(?<prefix>(?:file|link):)?(?<path>\.{1,2}(?:[/\\].*)?)$/.exec(argument)
  if (match?.groups?.path === undefined) return argument
  // A bare path stays bare and a prefixed spec keeps its prefix: pnpm's
  // link-vs-copy semantics differ between `file:` and a plain directory
  // path, and the anchor must not change which one the user asked for.
  const prefix = match.groups.prefix ?? ''
  return `${prefix}${resolve(cwd, match.groups.path)}`
}

/**
 * Run one `dsh plugin` invocation: init if needed, forward to pnpm, reconcile.
 * @param profile - the profile name.
 * @param args - pnpm arguments with relative path specs anchored to the invoking directory.
 * @returns the pnpm exit code.
 */
export function runPlugin(profile: string, args: readonly string[]): number {
  const dir = resolveProfileDir(profile)
  if (!existsSync(join(dir, 'package.json'))) {
    initProfile(dir, PROFILE_TEMPLATES[profile] ?? DEFAULT_PROFILE_BUNDLES)
    process.stderr.write(`${NAME}: initialized profile ${profile} at ${dir}\n`)
  }
  const before = readProfileManifest(NAME, dir)
  const pnpm = resolvePnpm()
  if (pnpm === undefined) {
    process.stderr.write(
      `${NAME}: pnpm not found — install it with \`npm install -g pnpm\` or \`corepack enable pnpm\` to manage profile plugins\n`,
    )
    return 127
  }
  // Windows resolves pnpm through its .cmd shim, which spawn() refuses
  // without a shell since the CVE-2024-27980 hardening. The shell is cmd.exe;
  // hide its console so a GUI parent (the desktop shell) never flashes one.
  // Shell mode concatenates command and args, so an absolute shim path with
  // spaces needs quoting.
  const command = pnpm.shell && /\s/.test(pnpm.command) ? `"${pnpm.command}"` : pnpm.command
  const result = spawnSync(command, args.map(argument => anchorPathSpec(argument, process.cwd())), {
    cwd: dir,
    stdio: 'inherit',
    shell: pnpm.shell,
    windowsHide: true,
  })
  if (result.error !== undefined) {
    throw result.error
  }
  const exitCode = result.status ?? 1
  if (exitCode === 0) {
    reconcilePlugins(before, dir)
  } else {
    // pnpm's own diagnostics name pnpm-workspace.yaml without saying WHICH
    // one; the profile owns it, and the commonest failure here is pnpm ≥10
    // blocking a git dependency's prepare (build) script until allowlisted.
    process.stderr.write(`${NAME}: pnpm failed in profile directory ${dir}\n`)
    if (args.some(argument => /^git\+|^github:|\.git(?:#|$)/.test(argument))) {
      process.stderr.write(
        `${NAME}: git-hosted plugins build on install via their prepare script, which pnpm blocks until allowed — `
        + `add the exact key pnpm printed above under allowBuilds in ${join(dir, 'pnpm-workspace.yaml')}, then re-run\n`,
      )
    }
  }
  return exitCode
}
