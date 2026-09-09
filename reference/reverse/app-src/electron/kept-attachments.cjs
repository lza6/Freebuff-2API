const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

// Attachments that would not survive until the agent reads them: pasted bytes with no path, and
// dropped files the OS is about to delete. main.cjs only wires the IPC channels to these two.
const KEPT_DIR = path.join(os.tmpdir(), 'freebuff-desktop-pastes')

// A pasted screenshot has no path, so the renderer hands over the bytes. `ext` is sanitized to a
// short alnum token. Returns { path, name } or null.
function saveClipboardImage(bytes, ext, keepDir = KEPT_DIR) {
  try {
    const safeExt = String(ext || 'png').toLowerCase().replace(/[^a-z0-9]/g, '').slice(0, 8) || 'png'
    fs.mkdirSync(keepDir, { recursive: true })
    const file = path.join(keepDir, `paste-${Date.now()}-${process.pid}.${safeExt}`)
    fs.writeFileSync(file, Buffer.from(bytes))
    return { path: file, name: path.basename(file) }
  } catch {
    return null
  }
}

// realpath, because macOS spells the temp dir both /var/folders/… and /private/var/folders/…
const canonical = (p) => {
  try {
    return fs.realpathSync(p)
  } catch {
    return path.resolve(p)
  }
}

// A file dragged from the macOS screenshot thumbnail (or any other file-promise drag) lands under
// the temp dir and is deleted moments after the drag ends, long before the agent reads it, so it is
// copied into `keepDir` at drop time, keeping its basename. Anything else is the user's real file
// and keeps its path. On any failure the original path is returned: at least it is honest.
function keepTransientFile(filePath, { tmpdir = os.tmpdir(), keepDir = KEPT_DIR } = {}) {
  const original = path.resolve(String(filePath || ''))
  try {
    const target = canonical(original)
    const temp = canonical(tmpdir)
    if (!target.startsWith(temp + path.sep) || !fs.statSync(target).isFile()) return original
    const dir = path.join(keepDir, `drop-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`)
    fs.mkdirSync(dir, { recursive: true })
    const copy = path.join(dir, path.basename(target))
    fs.copyFileSync(target, copy)
    return copy
  } catch {
    return original
  }
}

module.exports = { keepTransientFile, saveClipboardImage }
