/**
 * Login-shell PATH recovery.
 *
 * A GUI app launched from Finder/Dock does NOT inherit the shell's environment:
 * launchd hands it a minimal PATH (`/usr/bin:/bin:/usr/sbin:/sbin`), with no
 * Homebrew, no `~/.bun/bin`, no `~/.local/bin`, no nvm/mise/asdf shims. Every
 * child we spawn inherits that PATH, so the orchestrator — and through it every
 * agent harness — reports `bun: command not found`, `codex: command not found`,
 * and startup scripts fail. Launched from a terminal (dev) the PATH is already
 * fine, which is exactly why this only breaks in the packaged/Finder path.
 *
 * Fix: ask the user's real login shell what PATH it produces (it sources
 * .zprofile/.zshrc/.bash_profile, so it sees every version manager), then UNION
 * it onto the current PATH. Union, not replace: we only ever add directories, so
 * a working environment can't be made worse by a misbehaving shell rc.
 */

const { execFileSync } = require('node:child_process')
const fs = require('node:fs')
const os = require('node:os')
const path = require('node:path')

/** Marker so we can find PATH in shell output even when rc files print banners. */
const MARKER = '__FREEBUFF_PATH__'

/**
 * Directories worth having even if the login shell probe fails entirely.
 *
 * Includes the FIXED shim dirs of the common Node version managers (asdf, mise,
 * volta). Those managers put `codex` on a stable, probe-independent path, so a
 * user who installed `@openai/codex` globally under one of them stays reachable
 * even when the login-shell probe fails (broken/slow rc, timeout, or a passwd
 * shell that never sources the rc file where the manager is set up). nvm and fnm
 * have NO fixed shim dir — their bins live under a per-active-version directory
 * only known after sourcing the shell — so those still depend on the probe.
 */
function fallbackDirs(home = os.homedir()) {
  return [
    path.join(home, '.bun', 'bin'),
    path.join(home, '.local', 'bin'),
    path.join(home, '.cargo', 'bin'),
    path.join(home, '.volta', 'bin'), // volta shims
    path.join(home, '.asdf', 'shims'), // asdf shims
    path.join(home, '.local', 'share', 'mise', 'shims'), // mise shims
    '/opt/homebrew/bin',
    '/usr/local/bin',
  ]
}

/**
 * Resolve the user's login shell.
 *
 * `$SHELL` is present when the app is launched from a terminal (dev), but a
 * Finder/Dock-launched GUI app on macOS usually inherits NO `$SHELL`. Without a
 * fallback the PATH probe would be skipped in exactly the packaged case it
 * exists to fix, so we fall back to the passwd-DB login shell (`getpwuid`, via
 * `os.userInfo().shell`), which is populated regardless of how the app started.
 * That keeps `codex`/`bun` reachable for users whose global bin lives on an
 * nvm/fnm/mise/asdf node shim dir (not one of {@link fallbackDirs}).
 *
 * `shell`/`shellEnv`/`userInfo` are injectable so this is testable without the
 * ambient environment.
 */
function resolveLoginShell(opts = {}) {
  const { shell, userInfo = os.userInfo } = opts
  // `in` checks, not destructuring defaults: an explicit `shellEnv: undefined`
  // (a test forcing "no $SHELL") must override, not re-trigger the default.
  const shellEnv = 'shellEnv' in opts ? opts.shellEnv : process.env.SHELL
  // An explicitly-passed shell (even empty) is authoritative — no env fallback.
  if (shell !== undefined) return { shell: shell || null, source: 'explicit' }
  if (shellEnv) return { shell: shellEnv, source: '$SHELL' }
  try {
    const passwd = userInfo().shell || null
    return { shell: passwd, source: passwd ? 'passwd' : 'none' }
  } catch {
    // os.userInfo() can throw if the uid has no passwd entry (rare).
    return { shell: null, source: 'none' }
  }
}

/**
 * Ask the login shell for its PATH. Returns null if it can't be determined.
 * Interactive + login (`-ilc`) so both .zprofile and .zshrc run — version
 * managers land in either one depending on the user's setup.
 *
 * fish needs its own expression: there `$PATH` is a LIST, so `"$PATH"` would
 * come back SPACE-separated and we'd merge one garbage entry instead of N dirs.
 */
function probeLoginShellPath(opts = {}) {
  const { exec = execFileSync, platform = process.platform } = opts
  if (platform === 'win32') return null
  const { shell } = resolveLoginShell(opts)
  if (!shell) return null

  const isFish = path.basename(shell) === 'fish'
  const script = isFish
    ? `printf '%s%s' ${MARKER} (string join : $PATH)`
    : `printf '%s%s' ${MARKER} "$PATH"`
  try {
    const out = exec(shell, ['-ilc', script], {
      encoding: 'utf8',
      timeout: 5000,
      stdio: ['ignore', 'pipe', 'ignore'],
    })
    // Take the text AFTER the marker: rc files love to print banners, and a
    // login shell will happily emit them on stdout ahead of our value.
    const at = String(out).lastIndexOf(MARKER)
    if (at === -1) return null
    const value = String(out).slice(at + MARKER.length).trim()
    return value || null
  } catch {
    // A broken or slow rc file must never stop the app from booting.
    return null
  }
}

/**
 * Merge newly-discovered dirs onto `current`, preserving order and dropping
 * duplicates, empties, and anything not absolute. Purely additive: entries
 * already on PATH keep their position and precedence, so a misbehaving rc file
 * can only ever fail to help — it can't shadow a binary we already resolve.
 *
 * `extra` entries are only ever appended, never prepended, for that reason.
 */
function mergePath(current, extra) {
  const seen = new Set()
  const merged = []
  for (const part of [...current.split(path.delimiter), ...extra]) {
    const dir = part.trim()
    // Relative entries are a security smell on PATH and a sign of mangled shell
    // output (e.g. a list-valued $PATH that came back space-separated).
    if (!dir || !path.isAbsolute(dir) || seen.has(dir)) continue
    seen.add(dir)
    merged.push(dir)
  }
  return merged.join(path.delimiter)
}

/** First directory on `pathStr` that holds an executable named `bin`, or null.
 *  Used only for the boot breadcrumb — a cheap "did we actually make `codex`/
 *  `bun` reachable?" check so the next PATH regression is a log read, not a
 *  repro hunt. `access` is injectable for tests. */
function findOnPath(bin, pathStr, access = fs.accessSync) {
  for (const dir of pathStr.split(path.delimiter)) {
    if (!dir) continue
    const p = path.join(dir, bin)
    try {
      access(p, fs.constants.X_OK)
      return p
    } catch {
      // not here / not executable — keep scanning
    }
  }
  return null
}

/**
 * Compute the repaired PATH. Pure — takes the environment, returns the string —
 * so it's testable without touching the real process.
 *
 * `log` (optional) receives a one-line diagnostic of what the probe did and
 * whether `codex`/`bun` ended up reachable. This is the SECOND "works in dev,
 * breaks in the packaged/Finder build" PATH bug for Codex, and both prior
 * failures were silent — so we leave a breadcrumb to make the next one a log
 * read instead of a guess.
 */
function resolvePath(opts = {}) {
  const {
    current = process.env.PATH ?? '',
    home = os.homedir(),
    platform = process.platform,
    log,
    access,
  } = opts
  // Windows GUI apps inherit the full user PATH from the registry, so there is
  // nothing to repair — and the POSIX fallbacks below would be nonsense there.
  if (platform === 'win32') return current

  const { shell, source } = resolveLoginShell(opts)
  const shellPath = probeLoginShellPath({ ...opts, platform })
  const extra = [
    ...(shellPath ? shellPath.split(path.delimiter) : []),
    ...fallbackDirs(home),
  ]
  const next = mergePath(current, extra)

  if (typeof log === 'function') {
    const probeDirs = shellPath ? shellPath.split(path.delimiter).length : 0
    log(
      `[path-repair] shell=${shell ?? '(none)'} (${source}) ` +
        `probe=${shellPath ? `ok:${probeDirs}dirs` : 'failed'} ` +
        `dirs:${current.split(path.delimiter).filter(Boolean).length}->${next.split(path.delimiter).filter(Boolean).length} ` +
        `bun=${findOnPath('bun', next, access) ?? 'MISSING'} ` +
        `codex=${findOnPath('codex', next, access) ?? 'MISSING'}`,
    )
  }
  return next
}

/**
 * Repair `process.env.PATH` in place. Call ONCE at main-process startup, before
 * anything is spawned — children snapshot the env at spawn time.
 */
function fixPath(opts = {}) {
  const next = resolvePath(opts)
  process.env.PATH = next
  return next
}

module.exports = {
  fixPath,
  resolvePath,
  mergePath,
  probeLoginShellPath,
  fallbackDirs,
  findOnPath,
}
