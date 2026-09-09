/**
 * Freebuff Desktop updater.
 *
 * electron-updater owns update discovery, integrity verification, download,
 * and platform-specific installation. This module supplies Freebuff's UX:
 * taskbar progress, macOS translocation detection, and the in-app recovery
 * states for any check or install failure.
 *
 * Nothing asks at launch any more. A launch check starts downloading the new
 * version straight away and installs it when it lands; the only prompt is a
 * compact card in the corner of the app with a Cancel button, for a user who
 * would rather stay on the version they have. A release found later in the
 * session gets a branded sheet offering Install / Install when idle, once per
 * version. Nothing lets the user refuse a version durably: every dismissal
 * lasts at most until the next launch, which installs on its own.
 *
 * The pushed surface IS the state machine. Main owns it; the renderer only
 * mirrors it, and a window that is still loading (or reloading) asks for it on
 * mount rather than missing the one push it would ever get. Everything else
 * reads off it too — `armed` means an update is downloaded and waiting for a
 * quiet moment, `installing` means the app is on its way out — so there are no
 * parallel flags to keep in step with what the user can see.
 */

// The next check is derived from the wall clock rather than trusted to a timer:
// a laptop that slept through the interval fires it late or not at all, which
// is exactly how an app left open for days never notices a release.
const CHECK_INTERVAL_MS = 2 * 60 * 60 * 1000
const RETRY_AFTER_FAILURE_MS = 10 * 60 * 1000
// A timer, not a straight call, purely so the check lands after boot() returns
// rather than in the middle of it.
const FIRST_CHECK_DELAY_MS = 0
const TICK_MS = 60 * 1000
// A check that never settles (a half-open socket after a network change) would
// otherwise leave `checking` set and kill every later check in the session.
const CHECK_TIMEOUT_MS = 2 * 60 * 1000
// How long the app has to stay idle before a deferred install fires. Long
// enough that it never feels like the app restarted out of nowhere.
const IDLE_SETTLE_MS = 60 * 1000
// Release notes ride the update metadata (`releaseNotes` in latest.yml, cut from
// freebuff-desktop/CHANGELOG.md by the release workflow) and are pushed to the
// renderer verbatim, so a runaway entry is capped here rather than trusted.
const MAX_RELEASE_NOTES_CHARS = 4000

/**
 * The notes electron-updater found for a version, as one string, or undefined.
 * electron-builder writes a string; the type also allows a per-version array
 * (the GitHub provider's shape), so both are accepted and anything else — a
 * number, an object, whitespace — is treated as "no notes" rather than shown.
 */
function releaseNotesOf(updateInfo) {
  const raw = updateInfo?.releaseNotes
  const text =
    typeof raw === 'string'
      ? raw
      : Array.isArray(raw)
        ? raw
            .map((entry) => (typeof entry?.note === 'string' ? entry.note : ''))
            .filter(Boolean)
            .join('\n')
        : ''
  const trimmed = text.trim()
  if (!trimmed) return undefined
  return trimmed.length > MAX_RELEASE_NOTES_CHARS
    ? trimmed.slice(0, MAX_RELEASE_NOTES_CHARS)
    : trimmed
}

const { createBusySignal } = require('./busy.cjs')

let singleton = null

function macAppBundlePath(execPath) {
  const marker = '/Contents/MacOS/'
  const i = String(execPath).indexOf(marker)
  return i === -1 ? null : execPath.slice(0, i)
}

/** A DMG or translocated bundle cannot self-update. */
function isMacBundleSelfUpdatable(bundle) {
  if (!bundle) return false
  const value = String(bundle)
  return !value.startsWith('/Volumes/') && !value.includes('/AppTranslocation/')
}

function manualDownloadUrl(platform, arch, releaseFlavor = 'standard') {
  const base = 'https://freebuff.com/api/desktop/download'
  if (platform === 'darwin') return `${base}/${arch === 'arm64' ? 'mac-arm64' : 'mac-intel'}`
  if (platform === 'win32' && arch === 'arm64') return `${base}/windows-arm64`
  if (platform === 'win32')
    return `${base}/${releaseFlavor === 'baseline' ? 'windows-baseline' : 'windows'}`
  if (platform === 'linux')
    return `${base}/${arch === 'arm64' ? 'linux-arm64' : 'linux'}`
  return 'https://freebuff.com/desktop'
}

function loadState(app, fs, path) {
  try {
    const {
      skippedVersion: _retired,
      updateSchedule: _retiredSchedule,
      lastScheduledCheckAt: _retiredLastCheck,
      ...state
    } = JSON.parse(
      fs.readFileSync(path.join(app.getPath('userData'), 'update-state.json'), 'utf8'),
    )
    return state
  } catch {
    return {}
  }
}

function saveState(app, fs, path, state) {
  try {
    fs.writeFileSync(path.join(app.getPath('userData'), 'update-state.json'), JSON.stringify(state))
    return true
  } catch {
    // Prompt bookkeeping is best effort; durable preferences check the result.
    return false
  }
}

/** `busy`: the renderer's heartbeat (busy.cjs). Injected because main.cjs's quit guard reads
 *  the same signal — the updater listens for it here only because this is where the IPC has
 *  always been wired; it does not own the answer. */
function init({ currentVersion, isPackaged, releaseFlavor = 'standard', getWindow, busy = createBusySignal() }) {
  if (singleton) return singleton

  const path = require('node:path')
  const fs = require('node:fs')
  const { app, ipcMain, powerMonitor, shell } = require('electron')
  // CancellationToken comes from the same module so a cancelled download really
  // stops; scripts/electron-updater-entry.ts has to keep re-exporting it.
  const { autoUpdater, CancellationToken } = require(
    isPackaged ? path.join(process.resourcesPath, 'electron-updater.cjs') : 'electron-updater',
  )

  const downloadUrl = manualDownloadUrl(process.platform, process.arch, releaseFlavor)
  const autoChecksEnabled = isPackaged && !process.env.FREEBUFF_DISABLE_UPDATE_CHECK
  let checking = false
  // when the next scheduled check is due, in wall-clock ms
  let nextCheckAt = Number.POSITIVE_INFINITY
  // what the renderer is showing, and the whole update state with it
  let card = null
  // the download in flight: { version, token, cancelled }
  let active = null
  // what the last successful check found: { version, notes } — every card for
  // that version carries its notes, whichever path built it
  let latest = null
  // when the current uninterrupted idle stretch began
  let idleSince = 0
  // versions the user cancelled this session — not persisted, so the next launch
  // offers to update again
  const declined = new Set()

  // Freebuff drives the download itself so it can be cancelled and so a plain
  // quit never applies an update the user said no to.
  autoUpdater.autoDownload = false
  autoUpdater.autoInstallOnAppQuit = false
  autoUpdater.autoRunAppAfterInstall = true
  autoUpdater.disableWebInstaller = true
  // Tells the feed which build is ASKING, which is the only way to see what is
  // INSTALLED rather than what is being downloaded. `parseUpdaterUserAgent` in
  // web's api/desktop/updates/[target]/[file] reads it back, and its docstring
  // explains why this exact shape cannot change once a build ships.
  autoUpdater.requestHeaders = {
    'User-Agent': `Freebuff/${currentVersion} (${process.platform}; ${process.arch}; ${releaseFlavor})`,
  }
  autoUpdater.logger = {
    debug: (...args) => console.debug('[updater]', ...args),
    info: (...args) => console.info('[updater]', ...args),
    warn: (...args) => console.warn('[updater]', ...args),
    error: (...args) => console.error('[updater]', ...args),
  }
  /**
   * Start something and stop caring. Nothing in here is worth taking the app
   * down for, and an unhandled rejection in the main process is exactly that —
   * a failed browser handoff should be logged rather than becoming a rejection
   * that takes down the main process.
   */
  function detach(promise) {
    void Promise.resolve(promise).catch((error) =>
      console.error('[updater]', error?.message || error),
    )
  }

  function setProgress(fraction) {
    try {
      getWindow()?.setProgressBar(fraction)
    } catch {
      // The window may have closed during installation.
    }
  }

  const liveWindow = () => {
    const win = getWindow()
    return win && !win.isDestroyed?.() && win.webContents ? win : null
  }

  function setCard(next) {
    card = next
    try {
      liveWindow()?.webContents.send('update-card', next)
    } catch {
      // The window can go away between the check and the send.
    }
  }

  const offerOf = (version) => ({
    version,
    currentVersion,
    // only ever present as a non-empty string: the renderer keys the "what's
    // new" list off its existence
    ...(latest?.version === version && latest.notes
      ? { notes: latest.notes }
      : {}),
  })

  /** A download or an install is under way; a second one would collide with it. */
  const updateInFlight = () => active !== null || card?.phase === 'installing'

  /** Whether restarting right now would interrupt something the user cares about.
   *  Unknown is busy: a renderer that has not mounted yet (or stopped heartbeating)
   *  cannot prove that an automatic restart is safe. */
  const appIsBusy = () => !busy.idle(Date.now())

  function canSelfUpdate() {
    if (process.platform !== 'darwin') return true
    return isMacBundleSelfUpdatable(macAppBundlePath(process.execPath))
  }

  /**
   * Download it, then do `when` with it: 'now' for a user who pressed Install,
   * 'idle' for one who asked to be left alone until the app is quiet, and
   * 'auto' for the launch path — which nobody asked for, so it installs at once
   * unless that would land on a running turn.
   */
  async function update(version, when) {
    if (updateInFlight()) return
    if (!canSelfUpdate()) {
      // Downloading a package this bundle can never apply just burns bandwidth.
      setCard({ phase: 'translocated', ...offerOf(version) })
      return
    }

    const token = CancellationToken ? new CancellationToken() : undefined
    const download = { version, token, cancelled: false }
    active = download
    setCard({ phase: 'downloading', fraction: 0, ...offerOf(version) })

    const onProgress = ({ percent }) => {
      if (download.cancelled) return
      const fraction = Number.isFinite(percent)
        ? Math.min(1, Math.max(0, percent / 100))
        : 0
      setProgress(fraction)
      setCard({ phase: 'downloading', fraction, ...offerOf(version) })
    }
    autoUpdater.on('download-progress', onProgress)

    let failure = null
    try {
      await autoUpdater.downloadUpdate(token)
    } catch (error) {
      failure = error
    } finally {
      // Release download resources before publishing its result; a late progress
      // event must not redraw a download the user has already been given back.
      autoUpdater.off('download-progress', onProgress)
      setProgress(-1)
      if (active === download) active = null
    }

    if (download.cancelled) return
    if (failure) {
      console.error(
        '[updater] update download failed:',
        failure?.message || failure,
      )
      // Leave the update on the table and keep both recovery paths in Freebuff.
      setCard({ phase: 'failed', ...offerOf(version) })
      return
    }
    if (when === 'now' || (when === 'auto' && !appIsBusy())) {
      installDownloaded(version)
      return
    }
    // A turn is running (a restart auto-resumes one, so this is common at
    // launch) or the user asked to wait. idleSince is left as it is: they may
    // already be partway through the settle time.
    setCard({ phase: 'armed', ...offerOf(version) })
    maybeIdleInstall()
  }

  /** Hand off to the installer and restart. Only ever called post-download. */
  function installDownloaded(version) {
    if (card?.phase === 'installing') return
    setCard({ phase: 'installing', ...offerOf(version) })

    const failed = (error) => {
      console.error('[updater] update install failed:', error?.message || error)
      // Not a dead end: retry and manual-download actions stay available.
      setCard({ phase: 'failed', ...offerOf(version) })
    }
    // macOS hands the verified ZIP to Squirrel here; failures in that final
    // native step arrive as an event, not a rejected promise, so keep the
    // manual fallback available until the app actually exits.
    autoUpdater.once('error', failed)
    try {
      // NSIS's stock progress window clashes with the in-app update card and
      // makes the final step look like a different application. Windows can do
      // this handoff silently; macOS keeps its normal visible Squirrel flow.
      autoUpdater.quitAndInstall(process.platform === 'win32', true)
    } catch (error) {
      autoUpdater.off('error', failed)
      failed(error)
    }
  }

  function cancelDownload() {
    if (!active) return
    active.cancelled = true
    declined.add(active.version)
    try {
      active.token?.cancel()
    } catch (error) {
      // Without a token the bytes keep coming, but nothing will be installed.
      console.warn('[updater] could not cancel download:', error?.message || error)
    }
    setProgress(-1)
    setCard(null)
  }

  /** Reject rather than hang forever on a socket that will never answer. */
  function withTimeout(promise) {
    let timer
    const timeout = new Promise((_resolve, reject) => {
      timer = setTimeout(() => reject(new Error('update check timed out')), CHECK_TIMEOUT_MS)
      timer.unref?.()
    })
    return Promise.race([promise, timeout]).finally(() => clearTimeout(timer))
  }

  /**
   * `launch` downloads without asking; `session` offers the sheet once per
   * version; `interactive` is the menu item, which answers in-app because
   * someone is waiting on it.
   */
  async function checkNow(mode = 'session') {
    const interactive = mode === 'interactive'
    if (updateInFlight() || checking) return
    if (!isPackaged) {
      if (interactive) setCard({ phase: 'unavailable', currentVersion })
      return
    }

    if (interactive) setCard({ phase: 'checking', currentVersion })
    checking = true
    if (!interactive) nextCheckAt = Date.now() + CHECK_INTERVAL_MS
    let result
    try {
      result = await withTimeout(autoUpdater.checkForUpdates())
    } catch (error) {
      console.error('[updater] check failed:', error?.message || error)
      // Offline now shouldn't mean no updates for the whole two-hour interval.
      nextCheckAt = Date.now() + RETRY_AFTER_FAILURE_MS
      if (interactive) setCard({ phase: 'check-failed', currentVersion })
      return
    } finally {
      checking = false
    }

    if (!result?.isUpdateAvailable) {
      if (interactive) setCard({ phase: 'current', currentVersion })
      return
    }

    // "available" with nothing to name is not something to download.
    const version = result.updateInfo?.version
    if (!version) {
      console.error('[updater] update reported with no version')
      if (interactive) setCard({ phase: 'check-failed', currentVersion })
      return
    }
    latest = { version, notes: releaseNotesOf(result.updateInfo) }
    if (!interactive && declined.has(version)) return

    if (interactive) {
      setCard({ phase: 'offered', ...offerOf(version) })
      return
    }

    if (mode === 'launch') {
      await update(version, 'auto')
      return
    }

    // Mid-session: one card per version, and it waits for an answer rather than
    // interrupting whatever is on screen.
    const state = loadState(app, fs, path)
    if (state.promptedVersion === version) return
    // The card is the update state, so replacing an armed one would throw away a
    // restart that is already waiting for the app to go quiet. An unanswered
    // offer is fair game: a newer version supersedes it.
    if (card && card.phase !== 'offered') return
    if (!liveWindow()) return // nobody to ask; the next tick tries again
    setCard({ phase: 'offered', ...offerOf(version) })
    saveState(app, fs, path, { ...state, promptedVersion: version })
  }

  /** Install a downloaded update once the app has been quiet long enough. */
  function maybeIdleInstall() {
    if (card?.phase !== 'armed') return
    // No fresh word from the renderer is not the same as "idle" (busy.cjs).
    if (!busy.idle(Date.now())) return
    if (!idleSince || Date.now() - idleSince < IDLE_SETTLE_MS) return
    installDownloaded(card.version)
  }

  function tick() {
    maybeIdleInstall()
    if (Date.now() < nextCheckAt) return
    detach(checkNow('session'))
  }

  // The renderer owns the busy signal (busy.cjs): it already knows about every running
  // turn and queued item across all tabs, and main would otherwise have to poll it. The
  // listener lives here, but the state it feeds is shared — main.cjs's quit guard reads it.
  ipcMain.on('update:busy', (_event, value) => {
    const isBusy = !!value
    const now = Date.now()
    const lapsed = !busy.fresh(now)
    if (isBusy) idleSince = 0
    else if (!idleSince || lapsed) idleSince = now
    busy.record(isBusy, now)
    maybeIdleInstall()
  })

  // A renderer that mounts (or reloads) mid-download would otherwise have no way
  // to know there is anything to show — including the Cancel button.
  ipcMain.handle('update:pending', () => card)

  ipcMain.handle('update:action', (_event, action) => {
    if (!card) return 'ignored'
    const { phase, version } = card
    if (action === 'install') {
      // Already downloaded and waiting for a quiet moment: this IS that moment.
      if (phase === 'armed') installDownloaded(version)
      else if (phase === 'offered' || phase === 'failed')
        detach(update(version, 'now'))
      else return 'ignored'
      return 'installing'
    }
    if (action === 'install-when-idle') {
      if (phase !== 'offered') return 'ignored'
      // Never count idle time from before the user asked, so choosing this while
      // already idle still leaves them a beat before the app restarts.
      idleSince = busy.idle(Date.now()) ? Date.now() : 0
      // Download now; when the quiet moment comes the install is instant.
      detach(update(version, 'idle'))
      return 'armed'
    }
    if (action === 'cancel') {
      // The same button in both places, and it means the same thing: stop. What
      // there is to stop differs — a download, or a restart that is waiting.
      if (phase === 'downloading') cancelDownload()
      else if (phase === 'armed')
        setCard({ phase: 'offered', ...offerOf(version) })
      else return 'ignored'
      return 'cancelled'
    }
    if (action === 'check-again') {
      if (phase !== 'check-failed') return 'ignored'
      detach(checkNow('interactive'))
      return 'checking'
    }
    if (action === 'manual-download') {
      if (
        phase !== 'failed' &&
        phase !== 'translocated' &&
        phase !== 'check-failed'
      ) {
        return 'ignored'
      }
      const recoveryCard = card
      setCard(null)
      detach(
        Promise.resolve()
          .then(() => shell.openExternal(downloadUrl))
          .catch((error) => {
            // The system browser is the last recovery path. If the handoff
            // fails, put the actionable sheet back unless another state won.
            if (card === null) setCard(recoveryCard)
            throw error
          }),
      )
      return 'opened-download'
    }
    if (action === 'later') {
      if (
        phase === 'downloading' ||
        phase === 'armed' ||
        phase === 'installing'
      ) {
        return 'ignored'
      }
      // The card goes away for this version; the next launch updates anyway.
      setCard(null)
      return 'dismissed'
    }
    // Anything else is a renderer this build does not know about. Dismissing the
    // card is a state change, so it is not a safe thing to fall through to.
    return 'ignored'
  })

  if (autoChecksEnabled) {
    nextCheckAt = Date.now()
    setTimeout(() => detach(checkNow('launch')), FIRST_CHECK_DELAY_MS)
    setInterval(tick, TICK_MS).unref?.()
    try {
      // Timers do not run while the machine sleeps, which is most of the time
      // between one workday and the next.
      powerMonitor?.on?.('resume', tick)
    } catch (error) {
      console.warn('[updater] powerMonitor unavailable:', error?.message || error)
    }
  }

  singleton = { checkNow }
  return singleton
}

module.exports = { init }
