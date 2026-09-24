/** Explicit CLI exemptions use the profile lock and never become pnpm arguments. */
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, onTestFinished, vi } from 'vitest'
import { getDshRuntimeVersion, readProfileVersionExemptions } from '@deepseek-ai/dsh-app-boot'
import { runPluginCommand } from '@deepseek-ai/dsh-plugin-manager/operations'
import { resolvePnpm, runPlugin, runtimePnpmCandidate } from '../src/plugin.ts'

vi.mock('../src/profile-boot.ts', () => ({ INSTALL_ANCHOR: '/installation/package.json' }))
vi.mock('@deepseek-ai/dsh-plugin-manager/operations', async importOriginal => ({
  ...await importOriginal<typeof import('@deepseek-ai/dsh-plugin-manager/operations')>(),
  runPluginCommand: vi.fn(),
}))

function fixture() {
  const home = mkdtempSync(join(tmpdir(), 'cli-version-exemptions-'))
  vi.stubEnv('DSH_HOME', home)
  // CLI now probes pnpm before runPluginCommand; node accepts `--version` so the probe always succeeds.
  vi.stubEnv('PNPM_BINARY', process.execPath)
  const stdout = vi.spyOn(process.stdout, 'write').mockImplementation(() => true)
  const stderr = vi.spyOn(process.stderr, 'write').mockImplementation(() => true)
  onTestFinished(() => {
    vi.restoreAllMocks()
    vi.unstubAllEnvs()
    vi.mocked(runPluginCommand).mockReset()
    rmSync(home, { recursive: true, force: true })
  })
  return { dir: join(home, 'profiles', 'test'), stdout, stderr }
}

const shimName = (): string => (process.platform === 'win32' ? 'pnpm.cmd' : 'pnpm')

/** A fake pnpm shim that passes the `--version` probe. */
function writeFakeShim(binDir: string): string {
  mkdirSync(binDir, { recursive: true })
  const file = join(binDir, shimName())
  if (process.platform === 'win32') {
    writeFileSync(file, '@ECHO off\r\nexit /b 0\r\n')
  } else {
    writeFileSync(file, '#!/bin/sh\nexit 0\n', { mode: 0o755 })
  }
  return file
}

describe('runtimePnpmCandidate', () => {
  it('resolves the runtime sibling layout (deployed desktop bundle)', () => {
    const runtime = mkdtempSync(join(tmpdir(), 'dsh-runtime-'))
    try {
      const binDir = join(runtime, 'node', 'node_modules', '.bin')
      writeFakeShim(binDir)
      const anchor = join(runtime, 'dsh', 'package.json')
      expect(runtimePnpmCandidate(anchor)).toBe(join(binDir, shimName()))
    } finally {
      rmSync(runtime, { recursive: true, force: true })
    }
  })

  it('returns undefined for a non-runtime layout (npm global install)', () => {
    const nodeModules = mkdtempSync(join(tmpdir(), 'dsh-npm-'))
    try {
      const anchor = join(nodeModules, '@deepseek-ai', 'dsh', 'package.json')
      expect(runtimePnpmCandidate(anchor)).toBeUndefined()
    } finally {
      rmSync(nodeModules, { recursive: true, force: true })
    }
  })
})

describe('resolvePnpm', () => {
  let home: string

  beforeEach(() => {
    home = mkdtempSync(join(tmpdir(), 'dsh-plugin-test-'))
    vi.stubEnv('DSH_HOME', home)
    vi.stubEnv('PATH', '')
    vi.stubEnv('PNPM_BINARY', '')
  })

  afterEach(() => {
    vi.unstubAllEnvs()
    rmSync(home, { recursive: true, force: true })
  })

  it('prefers PNPM_BINARY over every other candidate', () => {
    vi.stubEnv('PNPM_BINARY', process.execPath) // node --version exits 0
    expect(resolvePnpm()?.command).toBe(process.execPath)
  })

  it('returns undefined when no candidate resolves', () => {
    expect(resolvePnpm()).toBeUndefined()
  })
})

it('requires explicit acknowledgement, grants only the exact pair, lists and revokes it without pnpm', async () => {
  const { dir, stdout, stderr } = fixture()
  const runtime = getDshRuntimeVersion()
  expect(await runPlugin('test', ['allow-version', '@example/plugin@1.2.3', '--dsh-version', runtime, '--accept-risk'])).toBe(0)
  expect(stderr.mock.calls.map(call => call[0]).join('')).toMatchInlineSnapshot(`
    "dsh: warning: allowing incompatible plugin versions can break the application or corrupt data. Approval applies only to the exact package and DSH versions.
    "
  `)
  expect(stderr.mock.invocationCallOrder[0]).toBeLessThan(stdout.mock.invocationCallOrder[0]!)
  expect(readProfileVersionExemptions(dir)).toEqual({ '@example/plugin@1.2.3': [runtime] })
  expect(JSON.parse(readFileSync(join(dir, 'compatibility.json'), 'utf8'))).toEqual({ '@example/plugin@1.2.3': [runtime] })
  expect(await runPlugin('test', ['version-exemptions'])).toBe(0)
  expect(stdout).toHaveBeenLastCalledWith(JSON.stringify({ '@example/plugin@1.2.3': [runtime] }, undefined, 2) + '\n')
  expect(await runPlugin('test', ['revoke-version', '@example/plugin@1.2.3', `--dsh-version=${runtime}`])).toBe(0)
  expect(readProfileVersionExemptions(dir)).toEqual({})
  expect(runPluginCommand).not.toHaveBeenCalled()
})

it.each([
  ['allow-version', 'plugin@1.2.3', '--dsh-version', 'CURRENT'],
  ['allow-version', 'plugin@1.2.3', '--accept-risk'],
  ['allow-version', 'plugin@^1.2.3', '--dsh-version', 'CURRENT', '--accept-risk'],
  ['allow-version', 'plugin@1.2.3', '--dsh-version', '999.0.0', '--accept-risk'],
  ['allow-version', 'plugin@1.2.3', '--dsh-version', 'CURRENT', '--accept-risk=false'],
  ['allow-version', 'plugin@1.2.3', '--dsh-version', 'CURRENT', '--accept-risk', '--accept-risk'],
  ['revoke-version', 'plugin@1.2.3'],
  ['version-exemptions', 'extra'],
])('rejects malformed or unacknowledged exemption command %j', async (...arguments_) => {
  const { dir, stderr } = fixture()
  const args = arguments_.map(value => value === 'CURRENT' ? getDshRuntimeVersion() : value)
  expect(await runPlugin('test', args)).toBe(1)
  expect(stderr).toHaveBeenCalled()
  expect(existsSync(join(dir, 'compatibility.json'))).toBe(false)
  expect(runPluginCommand).not.toHaveBeenCalled()
})

it('uses shipped profile defaults when listing a missing profile', async () => {
  fixture()
  expect(await runPlugin('web', ['version-exemptions'])).toBe(0)
  expect(runPluginCommand).not.toHaveBeenCalled()
})

it.each([0, 1])('forwards ordinary pnpm output and exit status %s', async (exitCode) => {
  const { stdout, stderr } = fixture()
  vi.mocked(runPluginCommand).mockImplementation(async (_context, _args, options) => {
    options.onOutput?.('pnpm output', 'stdout')
    return { exitCode, output: '', truncated: false, logPath: '/profile/log' }
  })
  expect(await runPlugin('test', ['list'])).toBe(exitCode)
  expect(stdout).toHaveBeenCalledWith('pnpm output')
  if (exitCode === 0) expect(stderr).not.toHaveBeenCalled()
  else expect(stderr).toHaveBeenCalledWith('dsh: plugin command failed; diagnostics: /profile/log\n')
})

it('names the exact grant command for each package a compatibility check refused', async () => {
  const { stderr } = fixture()
  vi.mocked(runPluginCommand).mockResolvedValue({
    exitCode: 1, output: '', truncated: false, logPath: '/profile/log',
    incompatible: [{ name: '@example/plugin', version: '1.2.3', runtimeVersion: '0.1.0', peers: { '@deepseek-ai/dsh': '^9.0.0' } }],
  })
  expect(await runPlugin('test', ['add', '@example/plugin'])).toBe(1)
  expect(stderr.mock.calls.map(call => call[0])).toEqual([
    'dsh: to accept the risk, run: dsh plugin --profile test allow-version @example/plugin@1.2.3 --dsh-version 0.1.0 --accept-risk\n',
    'dsh: plugin command failed; diagnostics: /profile/log\n',
  ])
})

it('forwards all other commands unchanged and retains package diagnostics', async () => {
  const { stderr } = fixture()
  vi.mocked(runPluginCommand).mockResolvedValue({ exitCode: 127, output: '', truncated: false, logPath: '/profile/log' })
  const args = ['add', 'github:example/plugin']
  expect(await runPlugin('test', args)).toBe(127)
  expect(runPluginCommand).toHaveBeenCalledWith(expect.objectContaining({ profile: 'test' }), args, expect.objectContaining({ execution: 'cli' }))
  expect(stderr.mock.calls.map(call => call[0]).join('')).toContain('pnpm was not found')
})

it('passes the resolved pnpm command (and shell flag on Windows) into runPluginCommand', async () => {
  fixture()
  vi.stubEnv('PNPM_BINARY', process.execPath)
  vi.mocked(runPluginCommand).mockResolvedValue({ exitCode: 0, output: '', truncated: false, logPath: '/profile/log' })
  expect(await runPlugin('test', ['list'])).toBe(0)
  expect(runPluginCommand).toHaveBeenCalledWith(
    expect.objectContaining({ profile: 'test' }),
    ['list'],
    expect.objectContaining({
      execution: 'cli',
      command: process.execPath,
      shell: process.platform === 'win32',
      windowsHide: true,
    }),
  )
})
