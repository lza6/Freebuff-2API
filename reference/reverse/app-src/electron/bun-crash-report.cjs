/**
 * Read a Bun crash report out of the orchestrator's stderr tail.
 *
 * The orchestrator runs under Bun, and when Bun itself dies — a segfault, an internal assertion,
 * an illegal instruction — it is not our code on the stack and there is no JavaScript error to
 * catch. Bun prints its own report and aborts: exit code 3 on Windows (the C runtime's abort()
 * status) and 134 elsewhere. The report looks like
 *
 *     ============================================================
 *     Bun v1.3.14 (0d9b296a) Windows x64
 *     Windows v.win10_cu
 *     CPU: sse42 avx avx2
 *     Args: "C:\Users\me\AppData\Local\Programs\...\bun.exe" "...\orchestrator.js"
 *     Features: Bun.stdin(2) jsc transpiler_cache
 *     ...
 *     RSS: 0.25GB | Peak: 0.31GB | Commit: 0.43GB | Faults: 81552 | Machine: 8.54GB
 *
 *     panic(main thread): Internal assertion failure
 *     oh no: Bun has crashed. This indicates a bug in Bun, not your code.
 *
 *     To send a redacted crash report to Bun's team,
 *     please file a GitHub issue using the link below:
 *
 *      https://bun.report/1.3.14/wr1...
 *
 * A few lines of that are the whole diagnosis: which Bun on which Windows and CPU, what it
 * panicked on, and the bun.report link (Bun's own symbolicated trace, and the thing that lets
 * anyone find or file the upstream bug — when the crash handler lived long enough to print it;
 * on 2026-09-08 it did not). The `Args:` line is the user's home directory and is never kept.
 * This module keeps the signature and nothing else, so what reaches a dialog or a telemetry
 * row identifies the crash and never the machine's owner.
 */

const RUNTIME_LINE = /^Bun v\d+\.\d+\.\d+(?:-[\w.]+)? \([0-9a-f]+\)[^\n]*$/m
const OS_LINE = /^(?:Windows|macOS|Linux)[^\n]*$/m
const CPU_LINE = /^CPU: [^\n]*$/m
const PANIC_LINE = /^panic(?:\([^)\n]*\))?: [^\n]+$/m
// bun.report links are opaque tokens: no spaces, no punctuation that could close a sentence
const REPORT_URL = /https:\/\/bun\.report\/[^\s"'<>)\]]+/
const MEMORY_LINE = /^RSS: [^\n]+$/m

const MAX_LINE = 400

function clip(text) {
  const trimmed = text.trim()
  return trimmed.length > MAX_LINE ? `${trimmed.slice(0, MAX_LINE)}…` : trimmed
}

/**
 * The crash signature in `text`, or null when the text is not a Bun crash report. A report cut
 * off after the panic line (the crash handler died before the link) still counts: the panic is
 * the part that says what happened.
 */
function summarizeBunCrash(text) {
  if (typeof text !== 'string' || !text) return null
  const panic = text.match(PANIC_LINE)?.[0]
  const reportUrl = text.match(REPORT_URL)?.[0]
  if (!panic && !reportUrl) return null
  const pick = (re) => {
    const line = text.match(re)?.[0]
    return line ? clip(line) : undefined
  }
  const summary = {
    runtime: pick(RUNTIME_LINE),
    os: pick(OS_LINE),
    cpu: pick(CPU_LINE),
    panic: panic ? clip(panic) : undefined,
    reportUrl: reportUrl ? clip(reportUrl) : undefined,
    memory: pick(MEMORY_LINE),
  }
  for (const key of Object.keys(summary)) if (summary[key] === undefined) delete summary[key]
  return summary
}

module.exports = { summarizeBunCrash }
