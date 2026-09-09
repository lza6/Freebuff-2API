import { describe, expect, test } from 'bun:test'
import { createRequire } from 'node:module'

const require_ = createRequire(import.meta.url)
const { summarizeBunCrash } = require_('./bun-crash-report.cjs') as {
  summarizeBunCrash: (text: unknown) => Record<string, string> | null
}

// the report a Windows user's orchestrator-stderr.log carried on 2026-09-08, home directory
// changed; note the crash handler died before the bun.report link
const WINDOWS_REPORT = `[2026-09-08T04:29:10.250Z] Starting Freebuff orchestrator
Runtime: C:\\Users\\owen\\AppData\\Local\\Programs\\@codebufffreebuff-desktop\\resources\\bun\\bun.exe

============================================================
Bun v1.3.14 (0d9b296a) Windows x64
Windows v.win10_cu
CPU: sse42 avx avx2
Args: "C:\\Users\\owen\\AppData\\Local\\Programs\\@codebufffreebuff-desktop\\resources\\bun\\bun.exe" "C:\\Users\\owen\\AppData\\Local\\Programs\\@codebufffreebuff-desktop\\resources\\orchestrator\\orchestrator.js"
Features: Bun.stderr(2) Bun.stdin(2) Bun.stdout(2) jsc transpiler_cache 
Builtins: "bun:main" "bun:sqlite" "node:fs" "node:net"
Elapsed: 1093ms | User: 1265ms | Sys: 93ms
RSS: 0.25GB | Peak: 0.31GB | Commit: 0.43GB | Faults: 81552 | Machine: 8.54GB

panic(main thread): Internal assertion failure
oh no: Bun has crashed. This indicates a bug in Bun, not your code.

To send a redacted crash report to Bun's team,
please file a GitHub issue using the link below:

`

describe('a Bun crash report', () => {
  test('keeps the signature: runtime, os, cpu, panic, memory', () => {
    expect(summarizeBunCrash(WINDOWS_REPORT)).toEqual({
      runtime: 'Bun v1.3.14 (0d9b296a) Windows x64',
      os: 'Windows v.win10_cu',
      cpu: 'CPU: sse42 avx avx2',
      panic: 'panic(main thread): Internal assertion failure',
      memory: 'RSS: 0.25GB | Peak: 0.31GB | Commit: 0.43GB | Faults: 81552 | Machine: 8.54GB',
    })
  })

  test('never carries the Args line, which is a path under the user s home', () => {
    const json = JSON.stringify(summarizeBunCrash(WINDOWS_REPORT))
    expect(json).not.toContain('owen')
    expect(json).not.toContain('Args')
    expect(json).not.toContain('Runtime:')
  })

  test('keeps the bun.report link when the crash handler lived to print it', () => {
    const withLink = `${WINDOWS_REPORT} https://bun.report/1.3.14/wr10d9b296aAgIKAABBBCCC2m_p3AqAAB\n`
    expect(summarizeBunCrash(withLink)?.reportUrl).toBe(
      'https://bun.report/1.3.14/wr10d9b296aAgIKAABBBCCC2m_p3AqAAB',
    )
  })

  test('a panic off the main thread is still a panic', () => {
    expect(summarizeBunCrash('panic(thread 4120): Segmentation fault at address 0x130\n')).toMatchObject(
      { panic: 'panic(thread 4120): Segmentation fault at address 0x130' },
    )
  })
})

describe('anything that is not one', () => {
  test.each([
    ['a JavaScript error', 'SyntaxError: unexpected token\n    at main.js:1:1\nBun v1.3.14 (Windows x64)\n'],
    ['ordinary log lines mentioning bun', '[launcher] Copying host Bun 1.3.14 from /usr/bin/bun\n'],
    ['an empty tail', ''],
    ['not a string', undefined],
  ])('%s is null', (_name, text) => {
    expect(summarizeBunCrash(text)).toBeNull()
  })
})
