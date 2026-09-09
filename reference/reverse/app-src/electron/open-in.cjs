const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

const EDITORS = {
  vscode: { label: 'Visual Studio Code' },
  cursor: { label: 'Cursor' },
  zed: { label: 'Zed' },
}

function editorCandidates(platform = process.platform, env = process.env, home = os.homedir()) {
  if (platform === 'darwin') {
    return {
      vscode: [
        '/Applications/Visual Studio Code.app/Contents/Resources/app/bin/code',
        path.join(home, 'Applications/Visual Studio Code.app/Contents/Resources/app/bin/code'),
      ],
      cursor: [
        '/Applications/Cursor.app/Contents/Resources/app/bin/cursor',
        path.join(home, 'Applications/Cursor.app/Contents/Resources/app/bin/cursor'),
      ],
      zed: [
        '/Applications/Zed.app/Contents/MacOS/cli',
        path.join(home, 'Applications/Zed.app/Contents/MacOS/cli'),
      ],
    }
  }
  if (platform === 'win32') {
    const local = env.LOCALAPPDATA || ''
    const programs = local ? path.join(local, 'Programs') : ''
    const programFiles = [env.ProgramFiles, env['ProgramFiles(x86)']].filter(Boolean)
    return {
      vscode: [
        ...(programs ? [path.join(programs, 'Microsoft VS Code', 'Code.exe')] : []),
        ...programFiles.map((dir) => path.join(dir, 'Microsoft VS Code', 'Code.exe')),
      ],
      cursor: [
        ...(programs ? [path.join(programs, 'cursor', 'Cursor.exe')] : []),
        ...programFiles.map((dir) => path.join(dir, 'cursor', 'Cursor.exe')),
      ],
      zed: [
        ...(programs ? [path.join(programs, 'Zed', 'Zed.exe')] : []),
        ...programFiles.map((dir) => path.join(dir, 'Zed', 'Zed.exe')),
      ],
    }
  }
  return {}
}

function detectOpenTargets({
  platform = process.platform,
  env = process.env,
  home = os.homedir(),
  exists = fs.existsSync,
} = {}) {
  if (platform !== 'darwin' && platform !== 'win32') return []
  const candidates = editorCandidates(platform, env, home)
  const targets = []
  for (const id of Object.keys(EDITORS)) {
    const executable = candidates[id]?.find(exists)
    if (executable) targets.push({ id, label: EDITORS[id].label, kind: 'editor', executable })
  }

  if (platform === 'darwin') {
    targets.push({ id: 'terminal', label: 'Terminal', kind: 'terminal', executable: '/usr/bin/open' })
  } else {
    const wt = env.LOCALAPPDATA
      ? path.join(env.LOCALAPPDATA, 'Microsoft', 'WindowsApps', 'wt.exe')
      : ''
    if (wt && exists(wt)) {
      targets.push({
        id: 'terminal',
        label: 'Windows Terminal',
        kind: 'terminal',
        executable: wt,
        terminalMode: 'windows-terminal',
      })
    } else if (env.ComSpec && path.isAbsolute(env.ComSpec) && exists(env.ComSpec)) {
      targets.push({
        id: 'terminal',
        label: 'Command Prompt',
        kind: 'terminal',
        executable: env.ComSpec,
        terminalMode: 'cwd',
      })
    }
  }
  targets.push({
    id: 'file-manager',
    label: platform === 'darwin' ? 'Finder' : 'File Explorer',
    kind: 'system',
  })
  targets.push({ id: 'copy-path', label: 'Copy path', kind: 'system' })
  return targets
}

function nearestExisting(base, target, exists = fs.existsSync) {
  let current = target
  while (current !== base && current.startsWith(base + path.sep)) {
    if (exists(current)) return current
    current = path.dirname(current)
  }
  return exists(base) ? base : null
}

/** Resolve a renderer-selected relative file without allowing it or a symlink to leave the workspace. */
function resolveOpenTarget(root, relativePath, { exists = fs.existsSync, realpath = fs.realpathSync } = {}) {
  if (typeof root !== 'string' || !path.isAbsolute(root) || !exists(root)) return null
  let base
  try {
    base = realpath(root)
  } catch {
    return null
  }
  if (typeof relativePath !== 'string' && relativePath !== undefined) return null
  if (relativePath === '') return null
  if (relativePath && path.isAbsolute(relativePath)) return null

  const requested = relativePath ? path.resolve(base, relativePath) : base
  if (requested !== base && !requested.startsWith(base + path.sep)) return null
  const existing = nearestExisting(base, requested, exists)
  if (!existing) return null
  let opened
  try {
    opened = realpath(existing)
  } catch {
    return null
  }
  if (opened !== base && !opened.startsWith(base + path.sep)) return null
  return { requested, opened, fallback: requested !== existing }
}

function launchArgs(target, resolved, line, platform = process.platform) {
  const safeLine = Number.isInteger(line) && line > 0 && line <= 10_000_000 ? line : undefined
  if (target.kind === 'editor') {
    if (target.id === 'vscode' || target.id === 'cursor') {
      return safeLine && !resolved.fallback
        ? ['--goto', `${resolved.opened}:${safeLine}`]
        : [resolved.opened]
    }
    return safeLine && !resolved.fallback ? [`${resolved.opened}:${safeLine}`] : [resolved.opened]
  }
  const directory = fs.statSync(resolved.opened).isDirectory()
    ? resolved.opened
    : path.dirname(resolved.opened)
  if (target.id === 'terminal') {
    if (platform === 'darwin') return ['-a', 'Terminal', directory]
    return target.terminalMode === 'windows-terminal' ? ['-d', directory] : []
  }
  return null
}

function launchCwd(target, resolved) {
  if (target.kind !== 'terminal') return undefined
  return fs.statSync(resolved.opened).isDirectory()
    ? resolved.opened
    : path.dirname(resolved.opened)
}

module.exports = { detectOpenTargets, launchArgs, launchCwd, resolveOpenTarget }
