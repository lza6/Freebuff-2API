/**
 * `attachment:keepTransient` decides at drop time whether a dragged path will still exist when the
 * agent reads it: a macOS screenshot dragged from its thumbnail lives under the temp dir and is
 * deleted right after the drag, so it is copied; a file from anywhere else is the user's real file
 * and must stay where it is, or "edit this file" would edit a copy.
 */

import { afterAll, describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { basename, dirname, join } from 'node:path'

const require_ = createRequire(import.meta.url)
const { keepTransientFile, saveClipboardImage } = require_('./kept-attachments.cjs') as {
  keepTransientFile: (filePath: string, opts: { tmpdir: string; keepDir: string }) => string
  saveClipboardImage: (bytes: Uint8Array, ext: string, keepDir: string) => { path: string; name: string } | null
}

const root = mkdtempSync(join(tmpdir(), 'fb-drop-'))
const temp = join(root, 'T')
const keepDir = join(temp, 'freebuff-desktop-pastes')
const home = join(root, 'home')
mkdirSync(join(temp, 'TemporaryItems', 'NSIRD_screencaptureui_abc'), { recursive: true })
mkdirSync(home, { recursive: true })
const opts = { tmpdir: temp, keepDir }

afterAll(() => rmSync(root, { recursive: true, force: true }))

describe('keepTransientFile', () => {
  test('a screenshot dragged from its thumbnail is copied out of the temp dir under its own name', () => {
    const shot = join(temp, 'TemporaryItems', 'NSIRD_screencaptureui_abc', 'Screenshot 2026-09-04 at 10.00.00.png')
    writeFileSync(shot, 'png-bytes')
    const kept = keepTransientFile(shot, opts)
    expect(dirname(dirname(kept))).toBe(keepDir)
    expect(basename(kept)).toBe(basename(shot))
    // deleting the original, as macOS will, leaves the attachment readable
    rmSync(shot)
    expect(readFileSync(kept, 'utf8')).toBe('png-bytes')
  })

  test('the canonical /private/var spelling of the real temp dir is transient too', () => {
    // on macOS os.tmpdir() is /var/folders/… while its real path is /private/var/folders/…; a prefix check
    // against the unresolved form would silently keep the doomed path if Electron hands back the real one
    const probe = mkdtempSync(join(realpathSync(tmpdir()), 'TemporaryItems-probe-'))
    const realKeep = mkdtempSync(join(tmpdir(), 'fb-drop-keep-'))
    try {
      const shot = join(probe, 'Screenshot.png')
      writeFileSync(shot, 'png')
      const kept = keepTransientFile(shot, { tmpdir: tmpdir(), keepDir: realKeep })
      expect(kept).not.toBe(shot)
      expect(readFileSync(kept, 'utf8')).toBe('png')
    } finally {
      rmSync(probe, { recursive: true, force: true })
      rmSync(realKeep, { recursive: true, force: true })
    }
  })

  test('a file from outside the temp dir, a temp directory, and a temp file already gone all keep their path', () => {
    const notes = join(home, 'notes.md')
    writeFileSync(notes, 'hi')
    expect(keepTransientFile(notes, opts)).toBe(notes)
    const dir = join(temp, 'some-dir')
    mkdirSync(dir)
    expect(keepTransientFile(dir, opts)).toBe(dir)
    const gone = join(temp, 'TemporaryItems', 'gone.png')
    expect(keepTransientFile(gone, opts)).toBe(gone)
  })
})

describe('saveClipboardImage', () => {
  test('pasted bytes land in the kept dir under a sanitized extension', () => {
    const saved = saveClipboardImage(new Uint8Array([1, 2, 3]), 'PNG; rm -rf', keepDir)!
    expect(dirname(saved.path)).toBe(keepDir)
    expect(saved.path.endsWith('.pngrmrf')).toBe(true)
    expect(saved.name).toBe(basename(saved.path))
    expect(readFileSync(saved.path)).toEqual(Buffer.from([1, 2, 3]))
  })
})
