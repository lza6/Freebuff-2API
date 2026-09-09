import { afterAll, describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { realpathSync } from 'node:fs'

const require_ = createRequire(import.meta.url)
const { detectOpenTargets, launchArgs, resolveOpenTarget } = require_('./open-in.cjs') as {
  detectOpenTargets: (options: Record<string, unknown>) => { id: string; label: string; kind: string; executable?: string }[]
  launchArgs: (target: { id: string; kind: string; terminalMode?: string }, resolved: { opened: string; fallback: boolean }, line?: number, platform?: string) => string[] | null
  resolveOpenTarget: (root: string, relativePath?: string) => { requested: string; opened: string; fallback: boolean } | null
}

const parent = mkdtempSync(join(tmpdir(), 'fb-open-in-'))
const root = join(parent, 'repo')
const outside = join(parent, 'outside')
mkdirSync(join(root, 'src'), { recursive: true })
mkdirSync(outside)
writeFileSync(join(root, 'src', 'app.ts'), 'x')
writeFileSync(join(outside, 'secret'), 'x')
symlinkSync(outside, join(root, 'escape'))
const realRoot = realpathSync(root)

afterAll(() => rmSync(parent, { recursive: true, force: true }))

describe('open target discovery', () => {
  test('returns only installed editors and the fixed macOS system actions', () => {
    const targets = detectOpenTargets({
      platform: 'darwin',
      home: '/Users/test',
      exists: (value: string) => value.includes('Cursor.app'),
    })
    expect(targets.map(({ id, label }) => ({ id, label }))).toEqual([
      { id: 'cursor', label: 'Cursor' },
      { id: 'terminal', label: 'Terminal' },
      { id: 'file-manager', label: 'Finder' },
      { id: 'copy-path', label: 'Copy path' },
    ])
  })

  test('does not advertise unsupported Linux behavior', () => {
    expect(detectOpenTargets({ platform: 'linux' })).toEqual([])
  })

  test('falls back to Command Prompt without constructing a shell command', () => {
    const targets = detectOpenTargets({
      platform: 'win32',
      env: { ComSpec: '/Windows/System32/cmd.exe' },
      exists: (value: string) => value.endsWith('cmd.exe'),
    })
    expect(targets[0]).toMatchObject({ id: 'terminal', label: 'Command Prompt' })
    expect(launchArgs({ id: 'terminal', kind: 'terminal', terminalMode: 'cwd' }, { opened: root, fallback: false }, undefined, 'win32')).toEqual([])
  })
})

describe('workspace path boundary', () => {
  test('resolves a file and safely falls back for a deleted file', () => {
    expect(resolveOpenTarget(root, 'src/app.ts')).toEqual({
      requested: join(realRoot, 'src', 'app.ts'),
      opened: join(realRoot, 'src', 'app.ts'),
      fallback: false,
    })
    expect(resolveOpenTarget(root, 'src/deleted.ts')).toEqual({
      requested: join(realRoot, 'src', 'deleted.ts'),
      opened: join(realRoot, 'src'),
      fallback: true,
    })
  })

  test('rejects traversal, absolute paths, and symlink escapes', () => {
    expect(resolveOpenTarget(root, '../outside/secret')).toBeNull()
    expect(resolveOpenTarget(root, join(root, 'src', 'app.ts'))).toBeNull()
    expect(resolveOpenTarget(root, 'escape/secret')).toBeNull()
  })
})

describe('launch arguments', () => {
  test('passes a selected line as one argv value without shell interpolation', () => {
    const opened = join(root, 'src', 'app.ts')
    expect(launchArgs({ id: 'vscode', kind: 'editor' }, { opened, fallback: false }, 42)).toEqual([
      '--goto',
      `${opened}:42`,
    ])
  })

  test('opens the fallback folder without a stale line', () => {
    expect(launchArgs({ id: 'zed', kind: 'editor' }, { opened: join(root, 'src'), fallback: true }, 42)).toEqual([
      join(root, 'src'),
    ])
  })
})
