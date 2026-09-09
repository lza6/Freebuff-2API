/**
 * Whether the app is in the middle of something, as last reported by the renderer.
 *
 * The renderer owns this answer: it already knows about every running turn and queued
 * item across all tabs (isAppBusy in src/ui/chrome/UpdateCard.tsx), and heartbeats it
 * over `update:busy`. Main would otherwise have to poll the orchestrator for it.
 *
 * Two consumers, and the interesting part is that they need OPPOSITE defaults when the
 * renderer has gone quiet — a window reloading, a renderer that crashed:
 *
 *   - the updater asks `idle()` before restarting the app under the user. No fresh word
 *     is not the same as "idle", so silence must not authorise a restart.
 *   - the quit guard asks `busy()` before interrupting a quit. No fresh word is not the
 *     same as "busy" either: a dead renderer must not hold the app open with a dialog
 *     about work it can no longer see.
 *
 * Both fall out of one freshness rule, which is why they live together here.
 */

// past this a report says nothing about now
const BUSY_STALE_MS = 90 * 1000

function createBusySignal(staleMs = BUSY_STALE_MS) {
  /** the renderer's last word, with the time it spoke; null until it first speaks */
  let last = null
  const fresh = (now) => last !== null && now - last.at <= staleMs
  return {
    record: (busy, now) => {
      last = { busy: !!busy, at: now }
    },
    fresh,
    busy: (now) => fresh(now) && last.busy,
    idle: (now) => fresh(now) && !last.busy,
  }
}

module.exports = { createBusySignal, BUSY_STALE_MS }
