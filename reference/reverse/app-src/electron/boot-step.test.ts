/// <reference types="bun" />
//
// The failure this exists to prevent: an optional boot step that HANGS. A step that throws is
// recoverable — a broken feature gets reported. A hang leaves the process alive, healthy by every
// external signal, and doing nothing, and it cost three packaged e2e tests their full 60s timeout
// with no error printed anywhere before anyone looked at the boot path.

import { describe, expect, test } from 'bun:test'

const { withBoundedBoot } = require('./boot-step.cjs') as {
  withBoundedBoot: <T>(
    name: string,
    start: () => T | Promise<T>,
    opts?: { timeoutMs?: number; onError?: (...args: unknown[]) => void },
  ) => Promise<T | null>
}

const quiet = () => {}

describe('an optional boot step', () => {
  test('returns what it started when it starts', async () => {
    expect(await withBoundedBoot('ok', () => ({ port: 1 }), { onError: quiet })).toEqual({ port: 1 })
  })

  test('a step that never settles gives up instead of holding boot forever', async () => {
    const started = Date.now()
    const result = await withBoundedBoot('hangs', () => new Promise(() => {}), {
      timeoutMs: 40,
      onError: quiet,
    })
    expect(result).toBeNull()
    expect(Date.now() - started).toBeLessThan(2_000)
  })

  test('a rejected promise is caught, not propagated into boot', async () => {
    expect(
      await withBoundedBoot('rejects', async () => {
        throw new Error('no keychain')
      }, { onError: quiet }),
    ).toBeNull()
  })

  // The keychain probe threw synchronously on some hosts. This holds for either shape of the call
  // (the try/catch covers it), so it pins the behaviour rather than discriminating an implementation.
  test('a SYNCHRONOUS throw is caught too', async () => {
    expect(
      await withBoundedBoot('throws sync', () => {
        throw new Error('boom')
      }, { onError: quiet }),
    ).toBeNull()
  })

  test('the failure is reported, naming the step, so it is not silent', async () => {
    const said: unknown[][] = []
    await withBoundedBoot('mcp-consent-bridge', () => { throw new Error('locked') }, {
      onError: (...args) => said.push(args),
    })
    expect(said).toHaveLength(1)
    expect(String(said[0]![0])).toContain('mcp-consent-bridge')
    expect(String(said[0]![1])).toContain('locked')
  })

  test('a timeout message names the step and the bound', async () => {
    const said: unknown[][] = []
    await withBoundedBoot('slow-thing', () => new Promise(() => {}), {
      timeoutMs: 20,
      onError: (...args) => said.push(args),
    })
    expect(String(said[0]![1])).toMatch(/slow-thing did not start within 20ms/)
  })
})
