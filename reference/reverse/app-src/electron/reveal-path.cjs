const fs = require('node:fs')
const path = require('node:path')

/** Resolve a renderer-supplied changed-file path inside its thread worktree.
 * Returns the nearest existing parent for deleted files so Finder can still
 * open the directory that contained them. */
function resolveRevealTarget(root, relativePath) {
  if (!root || !relativePath || path.isAbsolute(relativePath)) return null
  const base = path.resolve(root)
  const target = path.resolve(base, relativePath)
  if (target !== base && !target.startsWith(base + path.sep)) return null

  if (fs.existsSync(target)) return { path: target, directoryOnly: false }
  let parent = path.dirname(target)
  while (parent !== base && parent.startsWith(base + path.sep)) {
    if (fs.existsSync(parent)) return { path: parent, directoryOnly: true }
    parent = path.dirname(parent)
  }
  return fs.existsSync(base) ? { path: base, directoryOnly: true } : null
}

module.exports = { resolveRevealTarget }
