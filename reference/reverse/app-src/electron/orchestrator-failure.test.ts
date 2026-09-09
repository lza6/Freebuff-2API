/**
 * This dialog is the last thing between a broken launch and a user who cannot start the app, so
 * what it says is the behaviour under test — not an implementation detail of it.
 */

import { describe, expect, test } from 'bun:test'
import { createRequire } from 'node:module'

const require_ = createRequire(import.meta.url)
const { describeFailure, COMPATIBILITY_BUILD_URL, PROFILE_BUSY_EXIT } = require_('./orchestrator-failure.cjs') as {
  describeFailure: (failure: {
    kind: string
    code?: number
    signal?: string | null
    platform: string
    arch?: string
    releaseFlavor?: string
    runtimeFlavor?: string
    baselineBundled?: boolean
    errorMessage?: string
    stderrTail?: string
  }) => { title: string; type: string; message: string; detail: string }
  COMPATIBILITY_BUILD_URL: string
  PROFILE_BUSY_EXIT: number
}

// what the reported launch actually put on screen: bun's minified frames, ending in the throw
const MINIFIED_STACK = `139989 |     const code = error46 && typeof error46 === "object" && "code" in error46 ?
139990 |     throw new OrchestratorAlreadyRunningError(path23);
                     ^
OrchestratorAlreadyRunningError: Another Freebuff orchestrator is already using this Desktop state profile.`

describe('a profile another orchestrator already owns', () => {
  const busy = (platform: string) =>
    describeFailure({ kind: 'exit', code: PROFILE_BUSY_EXIT, platform, stderrTail: MINIFIED_STACK })

  test('reads as a sentence about Freebuff, not as a crash', () => {
    const { title, type, message, detail } = busy('win32')

    expect(message).toBe('Freebuff is already open')
    // the heading that used to sit above a stack trace claimed something had gone wrong
    expect(message).not.toMatch(/failed|unexpected/i)
    // and neither the title bar nor the icon may contradict it — both used to say "failed"
    expect(title).not.toMatch(/failed/i)
    expect(type).not.toBe('error')
    // nothing a person has to decode: no frames, no class names, no paths
    expect(detail).not.toContain('OrchestratorAlreadyRunningError')
    expect(detail).not.toContain(MINIFIED_STACK)
    expect(detail).not.toMatch(/\bat \w+ \(|\.sqlite|\bcode \d/)
  })

  test('names the place the reader would go, in their own platform s words', () => {
    expect(busy('win32').detail).toContain('Task Manager')
    expect(busy('darwin').detail).toContain('Activity Monitor')
    expect(busy('linux').detail).toContain('system monitor')
    // and the thing to look for there, which is not called "Freebuff"
    expect(busy('win32').detail).toContain('"bun"')
  })

  test('answers both halves: use the window you have, or clear the one you cannot see', () => {
    const { detail } = busy('darwin')

    expect(detail).toMatch(/if you can see a Freebuff window/i)
    expect(detail).toMatch(/leftover background process/i)
  })
})

describe('every other way a launch dies', () => {
  test('still reports the fault, and still hands over the log', () => {
    const { title, type, message, detail } = describeFailure({
      kind: 'exit',
      code: 1,
      signal: null,
      platform: 'darwin',
      stderrTail: 'SyntaxError: unexpected token\n',
    })

    expect(message).toBe('The Freebuff orchestrator failed to start or stopped unexpectedly.')
    expect(title).toBe('Freebuff failed to start')
    expect(type).toBe('error')
    expect(detail).toContain('Process ended (code 1, signal none).')
    expect(detail).toContain('SyntaxError: unexpected token')
  })

  test('a spawn failure keeps the reason it was given', () => {
    const { detail } = describeFailure({
      kind: 'spawn',
      platform: 'darwin',
      errorMessage: 'Failed to start the orchestrator with "bun": ENOENT.',
    })

    expect(detail).toContain('ENOENT')
  })

  test('a readiness timeout says so, and carries no exit code to confuse it with one', () => {
    const { message, detail } = describeFailure({ kind: 'timeout', platform: 'win32' })

    expect(message).toBe('The Freebuff orchestrator failed to start or stopped unexpectedly.')
    expect(detail).toContain('did not become ready in time')
  })
})

describe('the runtime itself crashing', () => {
  // the report a Windows user's orchestrator-stderr.log carried on 2026-09-08 (home renamed); Bun's
  // crash handler died before the bun.report link, so the panic line is all there is
  const BUN_CRASH = `============================================================
Bun v1.3.14 (0d9b296a) Windows x64
Windows v.win10_cu
CPU: sse42 avx avx2
Args: "C:\\Users\\owen\\AppData\\Local\\Programs\\@codebufffreebuff-desktop\\resources\\bun\\bun.exe" "C:\\Users\\owen\\AppData\\Local\\Programs\\@codebufffreebuff-desktop\\resources\\orchestrator\\orchestrator.js"
Elapsed: 1093ms | User: 1265ms | Sys: 93ms

panic(main thread): Internal assertion failure
oh no: Bun has crashed. This indicates a bug in Bun, not your code.

To send a redacted crash report to Bun's team,
please file a GitHub issue using the link below:

`
  const crashed = (overrides: Record<string, unknown> = {}) =>
    describeFailure({
      kind: 'exit',
      code: 3,
      signal: null,
      platform: 'win32',
      arch: 'x64',
      stderrTail: BUN_CRASH,
      ...overrides,
    } as never) as ReturnType<typeof describeFailure> & {
      compatibilityBuildUrl?: string
      bunCrash?: { panic: string }
    }

  test('says whose crash it is and quotes the line that identifies it', () => {
    const { message, detail, type } = crashed()
    expect(type).toBe('error')
    expect(message).toBe("Freebuff's runtime crashed")
    expect(detail).toContain('bug inside Bun')
    expect(detail).toContain('panic(main thread): Internal assertion failure')
    expect(detail).toContain('Bun v1.3.14 (0d9b296a) Windows x64')
  })

  test('keeps the user s path out of the dialog even though the log has it', () => {
    expect(crashed().detail).not.toContain('owen')
    expect(crashed().detail).not.toContain('Args:')
  })

  test('offers the compatibility build on Windows x64, where one exists', () => {
    const { detail, compatibilityBuildUrl } = crashed()
    expect(compatibilityBuildUrl).toBe(COMPATIBILITY_BUILD_URL)
    expect(detail).toContain(COMPATIBILITY_BUILD_URL)
  })

  test.each([
    ['the baseline build itself', { releaseFlavor: 'baseline' }],
    ['a standard install that already ran the bundled baseline binary', { runtimeFlavor: 'baseline' }],
    ['a standard install that carries the baseline binary (the shell tries it itself)', { baselineBundled: true }],
    ['Windows on ARM', { arch: 'arm64' }],
    ['macOS', { platform: 'darwin', code: 134 }],
  ])('does not offer it on %s, which has no other build to try', (_name, overrides) => {
    const { detail, compatibilityBuildUrl } = crashed(overrides)
    expect(compatibilityBuildUrl).toBeUndefined()
    expect(detail).not.toContain('compatibility build')
  })

  test('exposes the crash signature so the shell can report it', () => {
    expect(crashed().bunCrash?.panic).toBe('panic(main thread): Internal assertion failure')
  })

  test('a JavaScript failure that merely mentions Bun is not a runtime crash', () => {
    const { message } = describeFailure({
      kind: 'exit',
      code: 1,
      signal: null,
      platform: 'win32',
      stderrTail: 'error: Invalid environment configuration\nBun v1.3.14 (Windows x64)\n',
    })
    expect(message).toBe('The Freebuff orchestrator failed to start or stopped unexpectedly.')
  })
})
