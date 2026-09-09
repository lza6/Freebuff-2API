'use strict'

/**
 * The approval dialog, drawn by Freebuff instead of by macOS.
 *
 * The important word is WHO, not what it looks like. `/api/` authenticates nothing — the origin
 * guard admits any local caller by design so curl works — so the orchestrator cannot be the thing
 * that decides whether a connector runs. Something a local attacker cannot forge has to be, and
 * that means the MAIN process.
 *
 * So this is a window main creates and main alone talks to. It is NOT the app's renderer: separate
 * process, its own file:// origin, its own preload, `contextIsolation`, no node, and it is handed
 * no app state. That matters because the app's renderer draws untrusted content — agent output and
 * MCP tool results — and an approval sharing a DOM with those is one XSS away from approving
 * itself. Here there is nothing to cross: the transcript cannot reach this window, and every answer
 * is checked to have come from this window's own webContents before it counts.
 *
 * Shaped as `dialog.showMessageBox` so the consent bridge cannot tell the difference and stays
 * exactly as it was — same options in, same `{ response }` out. A failure to open falls back to the
 * real message box rather than to approving anything.
 *
 * No top-level `require('electron')` — everything is injected, so this unit-tests under Bun.
 */

const path = require('node:path')

const CHANNEL = 'freebuff:mcp-consent'

/** How long the page gets to say `ready` before we give up on it and use the OS dialog. */
const READY_TIMEOUT_MS = 4_000

// `dialog.showMessageBox` is the path that guarantees a human is asked when our own window cannot
// be shown, so it must not carry anything it does not document. `specText`/`explanation` exist for
// this window only; Electron ignores unknown keys today, and a version that stops doing so would
// break exactly the fallback.
const DIALOG_KEYS = [
  'message', 'type', 'buttons', 'defaultId', 'title', 'detail', 'icon', 'textWidth',
  'cancelId', 'noLink', 'normalizeAccessKeys', 'checkboxLabel', 'checkboxChecked', 'signal',
]
const forDialog = (options) =>
  Object.fromEntries(
    Object.entries(options ?? {}).filter(([key]) => DIALOG_KEYS.includes(key)),
  )

/**
 * @param BrowserWindow  Electron's
 * @param ipcMain        Electron's
 * @param fallback       `dialog.showMessageBox`, used when a window cannot be opened at all
 * @param theme          () => 'dark' | 'light'
 * @param readyTimeoutMs how long the page gets to signal `ready`; injected so tests need no wall clock
 */
function createConsentWindow({
  BrowserWindow,
  ipcMain,
  fallback,
  theme = () => 'dark',
  readyTimeoutMs = READY_TIMEOUT_MS,
}) {
  return async function ask(parent, options) {
    let win
    try {
      win = new BrowserWindow({
        width: 520,
        // A starting size only; the page measures itself and asks for the height it needs.
        height: 300,
        useContentSize: true,
        parent: parent ?? undefined,
        // Modal only when there is a live parent to be modal TO; unparented it floats free, which
        // is what makes it findable when the app has no focused window.
        modal: Boolean(parent),
        show: false,
        resizable: false,
        minimizable: false,
        maximizable: false,
        fullscreenable: false,
        // It can arrive without the user having asked for anything, so it must not open behind the
        // thing they are looking at.
        alwaysOnTop: true,
        title: 'Freebuff',
        backgroundColor: theme() === 'light' ? '#f3f3f4' : '#0c0d0f',
        webPreferences: {
          preload: path.join(__dirname, 'consent-preload.cjs'),
          contextIsolation: true,
          nodeIntegration: false,
          sandbox: true,
          // Nothing here loads anything remote, so nothing needs to reach the network.
          webSecurity: true,
        },
      })
    } catch {
      // A host that cannot open a window still has to be able to ask. Falling back to the OS dialog
      // keeps the decision with a human; falling back to "approved" would not.
      return fallback(parent, forDialog(options))
    }

    // Nothing in this page navigates, opens a window, or embeds anything, so every one of those is
    // a bug or an attack — and this window is the thing standing between a config file and code
    // execution, so it refuses rather than trusts. Belt to the CSP's braces: the CSP governs what
    // the document may fetch, these govern what the window itself may become.
    const contents = win.webContents
    contents?.on?.('will-navigate', (event) => event.preventDefault())
    contents?.on?.('will-redirect', (event) => event.preventDefault())
    contents?.on?.('will-attach-webview', (event) => event.preventDefault())
    contents?.setWindowOpenHandler?.(() => ({ action: 'deny' }))

    return new Promise((resolve) => {
      let settled = false
      let timer
      let onMessage
      const teardown = () => {
        settled = true
        clearTimeout(timer)
        ipcMain.removeHandler?.(CHANNEL)
        // This listener, not every listener on the channel: `removeAllListeners` would deafen any
        // other consent window sharing it. Nothing opens two today, but the guard that ensures
        // that lives in a different module and is not this function's to rely on.
        if (onMessage) ipcMain.removeListener?.(CHANNEL, onMessage)
        if (!win.isDestroyed()) win.destroy()
      }
      const finish = (response) => {
        if (settled) return
        teardown()
        resolve({ response })
      }

      // A page that loads but never says `ready` is never shown — `win.show()` only runs in that
      // branch — so without this the promise never settles, the caller's re-entrancy guard stays
      // held, and every later request is refused with no dialog anywhere on screen. Bounded, and
      // bounded to the OS dialog rather than to a default answer: a human still decides.
      timer = setTimeout(() => {
        if (settled) return
        teardown()
        resolve(fallback(parent, forDialog(options)))
      }, readyTimeoutMs)
      timer.unref?.()

      // Scoped to THIS window's webContents. Without the check any renderer in the app could answer
      // its own approval, which is the whole thing this window exists to prevent.
      onMessage = (event, message) => {
        if (event.sender !== win.webContents) return
        if (message?.type === 'ready') {
          clearTimeout(timer)
          event.sender.send(CHANNEL, { type: 'request', options })
          win.show()
          return
        }
        // Bounded on both ends: a page that asks for two pixels or for the whole screen is not
        // going to get either.
        if (message?.type === 'height' && Number.isFinite(message.px)) {
          const height = Math.min(720, Math.max(200, Math.ceil(message.px)))
          if (!win.isDestroyed()) win.setContentSize(520, height, false)
          return
        }
        if (message?.type === 'answer') finish(message.approved === true ? 1 : 0)
      }
      ipcMain.on(CHANNEL, onMessage)

      // Closing the window is a refusal. Anything that is not an explicit yes has to be a no.
      win.on('closed', () => finish(0))
      win.loadFile(path.join(__dirname, 'consent-window.html')).catch(() => finish(0))
    })
  }
}

module.exports = { createConsentWindow, CHANNEL }
