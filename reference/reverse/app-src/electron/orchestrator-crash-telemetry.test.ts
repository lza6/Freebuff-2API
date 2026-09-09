import { describe, expect, test } from 'bun:test'
import { createRequire } from 'node:module'

const require_ = createRequire(import.meta.url)
const telemetry = require_('./orchestrator-crash-telemetry.cjs') as {
  ORCHESTRATOR_CRASH_EVENT: string
  apiHost: (env?: Record<string, string | undefined>) => string
  crashFingerprint: (crash: Record<string, string>) => string
  crashRecord: (input: Record<string, unknown>) => Record<string, any>
  readAnalyticsId: (path: string, fs: { readFileSync: (p: string, e: string) => string }) => string | null
  shipCrashRecord: (
    fetchImpl: (url: string, init: any) => Promise<{ ok: boolean }>,
    record: unknown,
    options?: { host?: string; timeoutMs?: number },
  ) => Promise<boolean>
}

const crash = {
  runtime: 'Bun v1.3.14 (0d9b296a) Windows x64',
  os: 'Windows v.win10_cu',
  cpu: 'CPU: sse42 avx avx2',
  panic: 'panic(main thread): Internal assertion failure',
  memory: 'RSS: 0.25GB | Peak: 0.31GB | Commit: 0.43GB | Faults: 81552 | Machine: 8.54GB',
}

describe('the crash row', () => {
  const record = telemetry.crashRecord({
    crash,
    failure: { kind: 'exit', code: 3, signal: null },
    version: '0.0.94',
    platform: 'win32',
    arch: 'x64',
    releaseFlavor: 'standard',
    clientSessionId: 'anon_test',
  })

  test('is a valid /api/logs record naming the crash and the install', () => {
    expect(record.level).toBe('error')
    expect(record.event).toBe(telemetry.ORCHESTRATOR_CRASH_EVENT)
    expect(record.message).toBe('panic(main thread): Internal assertion failure')
    expect(record.client_session_id).toBe('anon_test')
    expect(record.fingerprint_id).toMatch(/^[0-9a-f]{16}$/)
    expect(record.data).toMatchObject({
      surface: 'desktop',
      phase: 'exit',
      exit_code: 3,
      runtime: crash.runtime,
      os: crash.os,
      cpu: crash.cpu,
      platform: 'win32',
      arch: 'x64',
      release_flavor: 'standard',
      version: '0.0.94',
    })
  })

  test('carries the signature and nothing from the machine', () => {
    // no path, no home directory, no log excerpt can be in here: only the fields above exist
    const json = JSON.stringify(record)
    expect(json).not.toMatch(/[A-Za-z]:\\|\/Users\/|\/home\//)
    expect(Object.keys(record.data).sort()).toEqual(
      [
        'arch',
        'cpu',
        'exit_code',
        'exit_signal',
        'memory',
        'os',
        'phase',
        'platform',
        'release_flavor',
        'report_url',
        'runtime_flavor',
        'restart_attempt',
        'runtime',
        'surface',
        'version',
      ].sort(),
    )
  })

  test('a fresh install with no analytics id still ships, unattributed', () => {
    const fresh = telemetry.crashRecord({ crash, failure: { kind: 'exit', code: 3 }, platform: 'win32', arch: 'x64' })
    expect(fresh.client_session_id).toBeNull()
  })
})

describe('the fingerprint', () => {
  test('groups the same panic across processes: addresses are generalized', () => {
    const a = telemetry.crashFingerprint({ ...crash, panic: 'panic(main thread): Segmentation fault at address 0x26B33641AA2' })
    const b = telemetry.crashFingerprint({ ...crash, panic: 'panic(main thread): Segmentation fault at address 0x7FF6BF66A000' })
    expect(a).toBe(b)
  })

  test('separates the same panic on different Bun builds', () => {
    const a = telemetry.crashFingerprint(crash)
    const b = telemetry.crashFingerprint({ ...crash, runtime: 'Bun v1.3.13 (aaaaaaaa) Windows x64' })
    expect(a).not.toBe(b)
  })
})

describe('the analytics id', () => {
  const fsWith = (content: string | null) => ({
    readFileSync: () => {
      if (content === null) throw new Error('ENOENT')
      return content
    },
  })

  test('is the orchestrator s own id when state.json has one', () => {
    expect(telemetry.readAnalyticsId('state.json', fsWith('{"analyticsId":"anon_abc"}'))).toBe('anon_abc')
  })

  test.each([
    ['no state file', null],
    ['no id yet', '{"workspace":{}}'],
    ['a corrupt file', '{not json'],
  ])('is null with %s, and never minted here', (_name, content) => {
    expect(telemetry.readAnalyticsId('state.json', fsWith(content))).toBeNull()
  })
})

describe('shipping', () => {
  test('posts one anonymous batch to /api/logs on the API host', async () => {
    const calls: { url: string; init: any }[] = []
    const ok = await telemetry.shipCrashRecord(
      async (url, init) => {
        calls.push({ url, init })
        return { ok: true }
      },
      { event: 'x' },
      { host: 'https://api.example' },
    )
    expect(ok).toBe(true)
    expect(calls).toHaveLength(1)
    expect(calls[0].url).toBe('https://api.example/api/logs')
    expect(calls[0].init.method).toBe('POST')
    expect(calls[0].init.headers.authorization).toBeUndefined()
    expect(JSON.parse(calls[0].init.body)).toEqual({ records: [{ event: 'x' }] })
  })

  test('a failed post is swallowed: the dialog is the priority, not the row', async () => {
    const ok = await telemetry.shipCrashRecord(
      async () => {
        throw new Error('offline')
      },
      { event: 'x' },
      { host: 'https://api.example' },
    )
    expect(ok).toBe(false)
  })

  test('defaults to production, or the configured app url', () => {
    expect(telemetry.apiHost({})).toBe('https://www.codebuff.com')
    expect(telemetry.apiHost({ NEXT_PUBLIC_CODEBUFF_APP_URL: 'http://localhost:3000/' })).toBe('http://localhost:3000')
  })
})
