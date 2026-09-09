/**
 * Which packaged Bun binary runs the orchestrator.
 *
 * Bun's standard x64 build is compiled for AVX2, and on a CPU without it the binary dies before
 * it runs a line of JavaScript: `panic: Illegal instruction`, exit 0xC000001D, even for
 * `bun.exe --version`. Every native Windows startup crash on the public tracker apart from one
 * class (#881, #976, #983; CLI #752, #765, #810, #822, #930, #1134, #1288) was that, on Sandy and
 * Ivy Bridge-era CPUs, and the only remedy was a separate installer built on Bun's pre-AVX2
 * "baseline" binary that users had to know to look for.
 *
 * So the standard Windows x64 package now ships both binaries (scripts/fetch-bun.ts), and this
 * module decides between them once per launch:
 *
 *  - one binary present (the baseline installer, arm64, unix): that one, no questions;
 *  - both present: the standard binary unless a remembered verdict for THIS bun.exe (by size
 *    and mtime, so an update re-asks) says it cannot run here, else a probe — run the standard
 *    binary with `--version`, which is exactly the instruction-set test that matters, and takes
 *    a few hundred milliseconds — and a failed probe picks the baseline and remembers it.
 *
 * Nothing here reads CPUID: the question is not "does this CPU advertise AVX2" but "does this
 * exact binary run on this machine", and the binary answers that itself. The shell also treats a
 * pre-ready Bun panic as a second chance to try the baseline (main.cjs), for the case where
 * `--version` runs and the real workload does not.
 *
 * Two flavors matter and must not be conflated: the INSTALLER flavor (baseline installer or
 * standard) decides the update channel, so a standard install that chose the baseline binary
 * keeps updating from the standard channel, which now also carries both binaries; the RUNTIME
 * flavor is which binary actually ran, which the failure dialog and crash telemetry report.
 */

const path = require('node:path')
const { summarizeBunCrash } = require('./bun-crash-report.cjs')

const PROBE_TIMEOUT_MS = 5_000

// STATUS_ILLEGAL_INSTRUCTION (0xC000001D), as Node reports it on Windows: unsigned, and the
// signed reading some shells show. A process killed this way prints no crash report at all.
const ILLEGAL_INSTRUCTION_EXITS = new Set([0xc000001d, 0xc000001d - 2 ** 32])

function bunRuntimeCandidates(resourcesPath, platform, exists) {
  const exe = platform === 'win32' ? '.exe' : ''
  const dir = path.join(resourcesPath, 'bun')
  const standard = path.join(dir, `bun${exe}`)
  const baseline = path.join(dir, `bun-baseline${exe}`)
  return {
    standard: exists(standard) ? standard : null,
    baseline: exists(baseline) ? baseline : null,
  }
}

/** Which installer this is: the update channel follows it, never the binary chosen at launch. */
function installerFlavor(candidates) {
  return candidates.baseline && !candidates.standard ? 'baseline' : 'standard'
}

/** Whether `exe` runs on this machine at all: `--version` exits 0 and prints a version. */
function probeBunRuns(exe, execFileSync) {
  try {
    const out = execFileSync(exe, ['--version'], {
      timeout: PROBE_TIMEOUT_MS,
      windowsHide: true,
      encoding: 'utf8',
      stdio: ['ignore', 'pipe', 'ignore'],
    })
    return /^\d+\.\d+\.\d+/.test(String(out).trim())
  } catch {
    return false
  }
}

/** The identity a verdict is remembered under: a replaced bun.exe (an update) gets re-asked. */
function binaryIdentity(exe, statSync) {
  try {
    const stat = statSync(exe)
    return `${stat.size}:${Math.floor(stat.mtimeMs)}`
  } catch {
    return null
  }
}

function createRuntimeMemory({ file, fs }) {
  return {
    read(identity) {
      try {
        const saved = JSON.parse(fs.readFileSync(file, 'utf8'))
        return saved &&
          saved.identity === identity &&
          saved.verdict === 'baseline'
          ? 'baseline'
          : null
      } catch {
        return null
      }
    },
    write(identity, verdict) {
      try {
        fs.writeFileSync(
          file,
          JSON.stringify({ identity, verdict, at: new Date().toISOString() }),
        )
      } catch {
        /* a memory that cannot be written costs one probe per launch, nothing more */
      }
    },
    forget() {
      try {
        fs.rmSync(file, { force: true })
      } catch {
        /* same */
      }
    },
  }
}

/**
 * @returns {{ bun: string, runtimeFlavor: 'standard'|'baseline', installerFlavor: 'standard'|'baseline', reason: string }}
 */
function chooseBunRuntime({ candidates, probe, memory, identity }) {
  const installer = installerFlavor(candidates)
  if (!candidates.baseline) {
    return {
      bun: candidates.standard,
      runtimeFlavor: 'standard',
      installerFlavor: installer,
      reason: 'only binary',
    }
  }
  if (!candidates.standard) {
    return {
      bun: candidates.baseline,
      runtimeFlavor: 'baseline',
      installerFlavor: installer,
      reason: 'only binary',
    }
  }
  if (identity && memory?.read(identity) === 'baseline') {
    return {
      bun: candidates.baseline,
      runtimeFlavor: 'baseline',
      installerFlavor: installer,
      reason: 'remembered',
    }
  }
  if (probe(candidates.standard)) {
    return {
      bun: candidates.standard,
      runtimeFlavor: 'standard',
      installerFlavor: installer,
      reason: 'probe ran',
    }
  }
  if (identity) memory?.write(identity, 'baseline')
  return {
    bun: candidates.baseline,
    runtimeFlavor: 'baseline',
    installerFlavor: installer,
    reason: 'probe failed',
  }
}

/**
 * Whether a failed startup is worth one more attempt on the baseline binary: the standard
 * binary ran (not the baseline, and the baseline is bundled to switch to), the process did not
 * merely time out or find the profile busy, and what killed it was the runtime — a Bun crash
 * report on stderr, or an illegal-instruction exit, which leaves none.
 */
function retryOnBaseline(failure, choice) {
  if (!choice?.baselineBundled || choice.runtimeFlavor !== 'standard')
    return false
  if (!failure || failure.kind === 'timeout' || failure.code === 75)
    return false
  if (summarizeBunCrash(failure.stderrTail)?.panic) return true
  return ILLEGAL_INSTRUCTION_EXITS.has(failure.code)
}

module.exports = {
  ILLEGAL_INSTRUCTION_EXITS,
  PROBE_TIMEOUT_MS,
  retryOnBaseline,
  binaryIdentity,
  bunRuntimeCandidates,
  chooseBunRuntime,
  createRuntimeMemory,
  installerFlavor,
  probeBunRuns,
}
