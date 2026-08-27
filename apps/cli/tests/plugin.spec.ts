import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { resolvePnpm } from '../src/plugin.ts'

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

  it('falls through a broken PNPM_BINARY to the DSH_HOME shim', () => {
    vi.stubEnv('PNPM_BINARY', join(home, 'missing-pnpm'))
    const shim = writeFakeShim(join(home, 'node_modules', '.bin'))
    expect(resolvePnpm()?.command).toBe(shim)
  })

  it('uses the DSH_HOME shim when PATH has no pnpm', () => {
    const shim = writeFakeShim(join(home, 'node_modules', '.bin'))
    expect(resolvePnpm()?.command).toBe(shim)
  })

  it('returns undefined when no candidate resolves', () => {
    expect(resolvePnpm()).toBeUndefined()
  })
})
