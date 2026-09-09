'use strict'

/**
 * Running an OPTIONAL boot step so that neither a throw nor a hang can stop the app starting.
 *
 * Everything on the path that gates spawning the orchestrator runs for every user, whether or not
 * they use the feature it belongs to. A step that fails loudly there is recoverable — a broken
 * feature gets reported. A step that HANGS is not: the process is alive, healthy by every external
 * signal, and doing nothing. That is the least diagnosable failure available, and it is not
 * hypothetical — a keychain probe on the MCP consent bridge blocked boot on any machine whose login
 * keychain was locked, and three packaged e2e tests each burned their full 60s timeout with the
 * process up and no error printed anywhere.
 *
 * So: anything on that path which is not itself required to spawn the orchestrator goes through
 * here, and the app comes up without it.
 *
 * No `require('electron')`: the clock and the logger are injected, so this unit-tests under Bun.
 */

/** How long an optional boot step gets before the app starts without it. */
const BOOT_STEP_TIMEOUT_MS = 8_000

async function withBoundedBoot(
  name,
  start,
  { timeoutMs = BOOT_STEP_TIMEOUT_MS, onError = console.error } = {},
) {
  let timer
  try {
    return await Promise.race([
      // Called inside the try, so a SYNCHRONOUS throw is caught as well as a rejection — the
      // keychain probe threw synchronously on some hosts. (`Promise.resolve().then` is belt to
      // that braces; the try alone would do it. Kept because it also normalises a `start` that
      // returns a plain value.)
      Promise.resolve().then(start),
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(new Error(`${name} did not start within ${timeoutMs}ms`)),
          timeoutMs,
        )
        timer.unref?.()
      }),
    ])
  } catch (error) {
    onError(
      `[boot] ${name} unavailable, continuing without it:`,
      error?.message ?? error,
    )
    return null
  } finally {
    clearTimeout(timer)
  }
}

module.exports = { withBoundedBoot, BOOT_STEP_TIMEOUT_MS }
