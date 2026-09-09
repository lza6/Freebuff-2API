const flashing = new WeakSet()

/** Ask the OS for attention without raising or focusing the window. */
function requestWindowAttention(win, appFocused = false) {
  if (!win || win.isDestroyed() || appFocused || win.isFocused() || flashing.has(win))
    return false

  flashing.add(win)
  win.flashFrame(true)
  win.once('focus', () => clearWindowAttention(win))
  return true
}

function clearWindowAttention(win) {
  if (!win || !flashing.delete(win)) return false
  if (!win.isDestroyed()) win.flashFrame(false)
  return true
}

module.exports = { clearWindowAttention, requestWindowAttention }
