/**
 * Ship a runtime crash from the shell, because the process that normally ships diagnostics is
 * the one that died.
 *
 * Every other Desktop failure reaches Axiom through the orchestrator's log shipper. A Bun panic
 * during startup never gets there: the orchestrator is the crashed process, the shell shows a
 * dialog and quits, and the only record is orchestrator-stderr.log on a machine we cannot see.
 * The 2026-09-08 "Internal assertion failure" on Windows x64 was found because one user pasted
 * that file; nothing said how many others hit it, on which Windows builds, or whether it began
 * with 0.0.94. So the shell posts one row itself, anonymously, to the same /api/logs endpoint the
 * orchestrator uses (it accepts unauthenticated batches, rate-limited), before the dialog.
 *
 * The row carries the crash signature from bun-crash-report.cjs and nothing else: no paths, no
 * log excerpt, no home directory. Best-effort in every direction — a failed post is swallowed,
 * and the dialog never waits more than a few seconds for it.
 */

const { createHash } = require('node:crypto')

const ORCHESTRATOR_CRASH_EVENT = 'desktop.orchestrator_crash'
const PROD_API_HOST = 'https://www.codebuff.com'
const SHIP_TIMEOUT_MS = 4_000

/** Same host rule as the orchestrator's hosts.ts: the baked/dev app URL, else production. */
function apiHost(env = process.env) {
  const configured = env.NEXT_PUBLIC_CODEBUFF_APP_URL?.trim()
  return (configured || PROD_API_HOST).replace(/\/+$/, '')
}

/**
 * What groups two of these into one crash: the panic text with its addresses generalized (a
 * segfault lands at a different address per process), plus the runtime line, which names the
 * Bun build and platform the panic belongs to.
 */
function crashFingerprint(crash) {
  const stable = `${crash.runtime ?? ''}\n${(crash.panic ?? '').replace(/0x[0-9A-Fa-f]+/g, '0x#')}`
  return createHash('sha256').update(stable).digest('hex').slice(0, 16)
}

/**
 * The analytics id the orchestrator stamps on its own rows (state.json `analyticsId`), so a
 * crash counts under the same install as the launches around it. Read-only: minting one is the
 * orchestrator's job, and a crash on a fresh install ships without one rather than write state.
 */
function readAnalyticsId(statePath, fsImpl) {
  try {
    const value = JSON.parse(fsImpl.readFileSync(statePath, 'utf8'))?.analyticsId
    return typeof value === 'string' && value.length ? value : null
  } catch {
    return null
  }
}

function crashRecord({ crash, failure, version, platform, arch, releaseFlavor, runtimeFlavor, clientSessionId }) {
  return {
    level: 'error',
    event: ORCHESTRATOR_CRASH_EVENT,
    message: crash.panic ?? crash.reportUrl ?? 'Bun crash',
    client_session_id: clientSessionId ?? null,
    fingerprint_id: crashFingerprint(crash),
    data: {
      surface: 'desktop',
      // 'exit' is a crash before readiness (the app never came up); 'runtime' is one after it
      phase: failure.kind ?? null,
      exit_code: failure.code ?? null,
      exit_signal: failure.signal ?? null,
      restart_attempt: failure.restartAttempt ?? 0,
      runtime: crash.runtime ?? null,
      os: crash.os ?? null,
      cpu: crash.cpu ?? null,
      memory: crash.memory ?? null,
      report_url: crash.reportUrl ?? null,
      platform,
      arch,
      release_flavor: releaseFlavor ?? 'standard',
      // which Bun binary actually ran: a standard install may have chosen the bundled baseline
      runtime_flavor: runtimeFlavor ?? releaseFlavor ?? 'standard',
      version: version ?? null,
    },
  }
}

async function shipCrashRecord(fetchImpl, record, { host = apiHost(), timeoutMs = SHIP_TIMEOUT_MS } = {}) {
  try {
    const response = await fetchImpl(`${host}/api/logs`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ records: [record] }),
      signal: AbortSignal.timeout(timeoutMs),
    })
    return response.ok
  } catch {
    return false
  }
}

module.exports = {
  ORCHESTRATOR_CRASH_EVENT,
  apiHost,
  crashFingerprint,
  crashRecord,
  readAnalyticsId,
  shipCrashRecord,
}
