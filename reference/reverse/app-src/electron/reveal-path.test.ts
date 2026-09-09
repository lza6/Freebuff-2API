/**
 * `shell:revealChange` hands this a path the RENDERER chose and then opens the
 * result in Finder, so the containment check is a trust boundary: anything that
 * escapes the thread worktree is a main-process file disclosure driven by web
 * content. The reject cases below are the escapes that a naive check misses —
 * absolute paths, `..`, and the sibling-prefix trick (`/w/repo-secrets` starts
 * with `/w/repo`) that a plain `startsWith(base)` would wave through.
 *
 * The accept cases cover the other half: a deleted file must still reveal the
 * directory that contained it, walking up only as far as the worktree root.
 */

import { afterAll, describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const require_ = createRequire(import.meta.url)
const { resolveRevealTarget } = require_('./reveal-path.cjs') as {
  resolveRevealTarget: (
    root: string | null | undefined,
    relativePath: string | null | undefined,
  ) => { path: string; directoryOnly: boolean } | null
}

// one real worktree: existsSync decisions are the whole point, so nothing here is faked
const parent = mkdtempSync(join(tmpdir(), 'fb-reveal-'))
const root = join(parent, 'repo')
mkdirSync(join(root, 'src', 'deep'), { recursive: true })
writeFileSync(join(root, 'src', 'app.ts'), 'x')
mkdirSync(join(root, 'src', 'deep', 'nested'), { recursive: true })
// a sibling whose name has the worktree root as a string prefix
mkdirSync(join(parent, 'repo-secrets'), { recursive: true })
writeFileSync(join(parent, 'repo-secrets', 'creds.env'), 'TOKEN=1')

afterAll(() => rmSync(parent, { recursive: true, force: true }))

describe('containment', () => {
  test('rejects an absolute path even when it points inside the worktree', () => {
    // the renderer only ever sends repo-relative paths; an absolute one means the
    // caller is not the diff list, so it is refused rather than sanitised
    expect(resolveRevealTarget(root, join(root, 'src', 'app.ts'))).toBeNull()
    expect(resolveRevealTarget(root, '/etc/passwd')).toBeNull()
  })

  test('rejects a traversal out of the worktree', () => {
    expect(resolveRevealTarget(root, '../repo-secrets/creds.env')).toBeNull()
    expect(resolveRevealTarget(root, 'src/../../repo-secrets/creds.env')).toBeNull()
    expect(resolveRevealTarget(root, '../../../../etc/passwd')).toBeNull()
  })

  test('rejects a sibling directory that merely starts with the root string', () => {
    // `/…/repo-secrets`.startsWith(`/…/repo`) is true — the separator in the guard is
    // the only thing between this and revealing the neighbouring worktree
    expect(resolveRevealTarget(root, '../repo-secrets')).toBeNull()
  })

  test('rejects when either side is missing', () => {
    expect(resolveRevealTarget('', 'src/app.ts')).toBeNull()
    expect(resolveRevealTarget(null, 'src/app.ts')).toBeNull()
    expect(resolveRevealTarget(root, '')).toBeNull()
    expect(resolveRevealTarget(root, null)).toBeNull()
  })

  test('rejects everything when the worktree root itself is gone', () => {
    // a reaped worktree must not fall back to revealing whatever `path.resolve` produced
    const dead = join(parent, 'reaped')
    expect(resolveRevealTarget(dead, 'src/app.ts')).toBeNull()
  })
})

describe('resolution', () => {
  test('reveals an existing file itself, not just its folder', () => {
    expect(resolveRevealTarget(root, 'src/app.ts')).toEqual({
      path: join(root, 'src', 'app.ts'),
      directoryOnly: false,
    })
  })

  test('normalises redundant segments inside the worktree', () => {
    expect(resolveRevealTarget(root, './src/deep/../app.ts')).toEqual({
      path: join(root, 'src', 'app.ts'),
      directoryOnly: false,
    })
  })

  test('a deleted file falls back to the directory that contained it', () => {
    // the reveal button stays useful for the delete half of a diff
    expect(resolveRevealTarget(root, 'src/gone.ts')).toEqual({
      path: join(root, 'src'),
      directoryOnly: true,
    })
  })

  test('walks up past directories the deletion also removed', () => {
    expect(resolveRevealTarget(root, 'src/deep/nested/vanished/gone.ts')).toEqual({
      path: join(root, 'src', 'deep', 'nested'),
      directoryOnly: true,
    })
  })

  test('stops walking up at the worktree root', () => {
    // the loop condition is the second containment check: without it a deleted
    // top-level file would climb into the worktrees' shared parent
    expect(resolveRevealTarget(root, 'gone.ts')).toEqual({ path: root, directoryOnly: true })
    expect(resolveRevealTarget(root, 'a/b/c/d.ts')).toEqual({ path: root, directoryOnly: true })
  })
})
