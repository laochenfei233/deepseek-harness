import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { resolvePnpm, runtimePnpmCandidate } from '../src/plugin.ts'

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
