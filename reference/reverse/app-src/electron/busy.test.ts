/**
 * The whole point of this module is that "the renderer has gone quiet" is not an answer,
 * and that its two consumers need opposite defaults when it happens: the updater must not
 * restart the app on silence, and the quit guard must not hold a quit open on silence.
 * Both are one-liners over `fresh`, so what is worth pinning is that silence and staleness
 * really do fall out of BOTH sides rather than just the one that was written first.
 *
 * Time is a parameter here rather than a clock, so the stale cases are exact instead of
 * slept for.
 */

import { describe, expect, test } from 'bun:test'
import { createRequire } from 'node:module'

const require_ = createRequire(import.meta.url)
const { createBusySignal, BUSY_STALE_MS } = require_('./busy.cjs') as {
  createBusySignal: (staleMs?: number) => {
    record(busy: unknown, now: number): void
    fresh(now: number): boolean
    busy(now: number): boolean
    idle(now: number): boolean
  }
  BUSY_STALE_MS: number
}

const T0 = 1_000_000

describe('before the renderer has said anything', () => {
  test('is neither busy nor idle', () => {
    const signal = createBusySignal()
    // the launch window: a renderer that has not mounted yet cannot authorise a restart,
    // and must not make a quit ask about work nobody has reported
    expect(signal.fresh(T0)).toBe(false)
    expect(signal.busy(T0)).toBe(false)
    expect(signal.idle(T0)).toBe(false)
  })
})

describe('a fresh report', () => {
  test('busy is busy and not idle', () => {
    const signal = createBusySignal()
    signal.record(true, T0)
    expect(signal.busy(T0 + 1000)).toBe(true)
    expect(signal.idle(T0 + 1000)).toBe(false)
  })

  test('idle is idle and not busy', () => {
    const signal = createBusySignal()
    signal.record(false, T0)
    expect(signal.idle(T0 + 1000)).toBe(true)
    expect(signal.busy(T0 + 1000)).toBe(false)
  })

  test('the latest word wins', () => {
    const signal = createBusySignal()
    signal.record(true, T0)
    signal.record(false, T0 + 1)
    expect(signal.idle(T0 + 2)).toBe(true)
    expect(signal.busy(T0 + 2)).toBe(false)
  })

  test('anything truthy is busy, anything falsy is idle', () => {
    // it arrives over IPC from the renderer, so the value is whatever was sent
    const signal = createBusySignal()
    signal.record('yes', T0)
    expect(signal.busy(T0)).toBe(true)
    signal.record(undefined, T0)
    expect(signal.idle(T0)).toBe(true)
  })
})

describe('a stale report', () => {
  // a window reloading, or a renderer that crashed: it knows nothing about now
  test('cannot say busy — a dead renderer must not hold a quit behind a dialog', () => {
    const signal = createBusySignal()
    signal.record(true, T0)
    expect(signal.busy(T0 + BUSY_STALE_MS + 1)).toBe(false)
  })

  test('cannot say idle — silence is not permission to restart the app', () => {
    const signal = createBusySignal()
    signal.record(false, T0)
    expect(signal.idle(T0 + BUSY_STALE_MS + 1)).toBe(false)
  })

  test('the last moment of freshness still counts', () => {
    const signal = createBusySignal()
    signal.record(true, T0)
    expect(signal.fresh(T0 + BUSY_STALE_MS)).toBe(true)
    expect(signal.busy(T0 + BUSY_STALE_MS)).toBe(true)
    expect(signal.fresh(T0 + BUSY_STALE_MS + 1)).toBe(false)
  })

  test('a later report un-stales it', () => {
    const signal = createBusySignal()
    signal.record(true, T0)
    signal.record(true, T0 + BUSY_STALE_MS + 1)
    expect(signal.busy(T0 + BUSY_STALE_MS + 2)).toBe(true)
  })
})

describe('busy and idle are never both true', () => {
  test('across every combination of report and elapsed time', () => {
    const staleMs = 100
    for (const reported of [true, false]) {
      for (const elapsed of [0, 1, staleMs - 1, staleMs, staleMs + 1, staleMs * 10]) {
        const signal = createBusySignal(staleMs)
        signal.record(reported, T0)
        const now = T0 + elapsed
        expect(signal.busy(now) && signal.idle(now)).toBe(false)
        // and exactly one of them holds while the report is still fresh
        expect(signal.busy(now) || signal.idle(now)).toBe(signal.fresh(now))
      }
    }
  })
})

describe('the heartbeat keeps up with the staleness window', () => {
  test('a renderer reporting on schedule is never stale', () => {
    // src/ui/chrome/UpdateCard.tsx heartbeats every BUSY_HEARTBEAT_MS on top of reporting
    // every change; if that interval ever exceeded this window, a live app would look dead
    const heartbeat = readHeartbeatMs()
    expect(heartbeat).toBeLessThan(BUSY_STALE_MS)
  })
})

function readHeartbeatMs(): number {
  const source = require_('node:fs').readFileSync(
    require_('node:path').join(import.meta.dir, '../src/ui/chrome/UpdateCard.tsx'),
    'utf8',
  ) as string
  const match = /BUSY_HEARTBEAT_MS = ([\d_]+)/.exec(source)
  if (!match) throw new Error('BUSY_HEARTBEAT_MS not found in UpdateCard.tsx')
  return Number(match[1].replaceAll('_', ''))
}
