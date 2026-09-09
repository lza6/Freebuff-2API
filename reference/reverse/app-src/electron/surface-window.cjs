/** Bringing the app back to the user after they finished something in their browser.
 *
 * Split out of main.cjs so the window-choosing rule is testable without Electron: it is small, but
 * every clause in it is a case someone hit — a destroyed window still sitting in the map, a main
 * window closed on macOS while the app runs on, a minimized window that `focus()` alone leaves
 * minimized. `deps` is passed in rather than required so a test can hand it fakes.
 */

/**
 * @param {{
 *   mainWindow: unknown,
 *   threadWindows: Map<string, unknown>,
 *   focusApp: () => void,
 *   platform?: string,
 * }} deps
 * @returns {unknown|null} the window brought forward, or null when there was nothing to surface
 */
function surfaceWindow({ mainWindow, threadWindows, focusApp, platform = process.platform }) {
  const live = (w) => Boolean(w) && typeof w.isDestroyed === 'function' && !w.isDestroyed()
  // The main window by preference: sign-in is app-level — the screens offering it live in the tab
  // strip — and every open window sees the event that triggers this, so targeting whichever one
  // reported it would let a background pop-out decide what the user is looking at.
  // A pop-out is a fallback rather than a dead end: on macOS the main window can be closed while
  // the app keeps running, and the sign-in may have started from a thread window's notice card.
  const win = live(mainWindow) ? mainWindow : [...threadWindows.values()].find(live)
  // Deliberately does NOT create a window: this arrives from a background state change, and a user
  // who closed every window did not ask for a new one.
  if (!win) return null

  // `focus()` on a minimized window leaves it minimized on Windows and Linux, which is the state
  // this exists to get out of.
  if (win.isMinimized()) win.restore()
  win.show()
  win.focus()
  // On macOS, focusing a window does not bring the APP forward while another app owns the
  // foreground — which is exactly the situation here, since the browser does. `app.focus({steal})`
  // is the documented way across that line, and this is the moment it is meant for: the user's last
  // gesture was "sign me in", and it finished somewhere else on their behalf.
  if (platform === 'darwin') focusApp()
  return win
}

module.exports = { surfaceWindow }
