/**
 * How this copy of the app is signed, probed once at boot and reported on the
 * launch analytics event. The interesting failure: an ad-hoc signature's
 * designated requirement is a bare content hash, and macOS keys every privacy
 * grant (Documents, Desktop, …) to that requirement — so each update of an
 * ad-hoc build silently resets the user's grants. This field makes a bad
 * release observable within hours instead of via user complaints.
 *
 * Never throws and never outlasts timeoutMs: a diagnostic must not be able to
 * delay or break startup. Any failure degrades to 'unknown'.
 */

const { execFile } = require('node:child_process')

// values are stable — Axiom/PostHog queries match on them
const STATES = {
  developerId: 'developer-id', // certificate-anchored: grants survive updates
  adhoc: 'adhoc', // cdhash-pinned: every update resets privacy grants
  unsigned: 'unsigned',
  dev: 'dev-unpackaged', // source run — the signature would be dev Electron's
  notApplicable: 'not-applicable',
  unknown: 'unknown',
}

// Classify the designated requirement from `codesign -d -r-` output. The
// requirement (not the mere presence of a signature) is what macOS stores and
// re-evaluates for TCC, so it's what separates durable grants from doomed ones.
function classify(output) {
  if (/code object is not signed at all/i.test(output)) return { state: STATES.unsigned }
  // ad-hoc's line is prefixed `# ` because its requirement is implicit
  const match = /^#?\s*designated =>\s*(.+)$/m.exec(output)
  if (!match) return { state: STATES.unknown }
  const requirement = match[1].trim()
  if (/\bcertificate\b/.test(requirement) && /\banchor apple\b/.test(requirement)) {
    const team = /leaf\[subject\.OU\]\s*=\s*"?([A-Z0-9]+)"?/.exec(requirement)
    return { state: STATES.developerId, ...(team ? { team: team[1] } : {}) }
  }
  // a universal ad-hoc binary has one cdhash PER ARCH joined by `or` — still starts with cdhash
  if (/^cdhash\b/.test(requirement)) return { state: STATES.adhoc }
  return { state: STATES.unknown }
}

// ~30ms in practice (reads the signature blob, no bundle re-hash); the 1s cap
// is a hard ceiling on how long a diagnostic may hold up boot.
// `exec` is injectable for one reason: the backstop below cannot be tested against real codesign.
// Both halves of the race are bounded by the same timeoutMs, so driving it with a tiny timeout only
// asks which of two timers fires first — and under load both are delayed while codesign quietly
// finishes its real ~30ms of work, so the probe answers 'adhoc' and the cut-off never happens. A
// runner that never calls back is the case the backstop exists for, and it is the only way to state it.
function detectSigning({ platform, isPackaged, execPath, timeoutMs = 1_000, exec = execFile }) {
  if (!isPackaged) return Promise.resolve({ state: STATES.dev })
  if (platform !== 'darwin') return Promise.resolve({ state: STATES.notApplicable })
  const unknown = { state: STATES.unknown }
  const probe = new Promise((resolve) => {
    // codesign resolves the executable to its bundle, so execPath needs no surgery
    exec('codesign', ['-d', '-r-', execPath], { timeout: timeoutMs }, (_err, stdout, stderr) =>
      resolve(classify(`${stdout ?? ''}\n${stderr ?? ''}`)),
    )
  }).catch(() => unknown)
  // backstop for a runner that never calls back — boot awaits this promise
  let timer
  const cap = new Promise((resolve) => {
    timer = setTimeout(() => resolve(unknown), timeoutMs)
    timer.unref?.()
  })
  return Promise.race([probe, cap]).finally(() => clearTimeout(timer))
}

module.exports = { detectSigning, classify }
