/// <reference types="bun" />
//
// What this is for is the disk: a stolen laptop, a backup, another account on the machine. So what
// is pinned is that nothing readable reaches the file, and that every way the file can go wrong
// costs one sign-in rather than leaving a connector permanently broken.

import { afterEach, describe, expect, test } from 'bun:test'
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const { createSecretStore } = require('./mcp-secret-store.cjs') as {
  createSecretStore: (o: { safeStorage: unknown; file: string }) => null | {
    get(k: string): string | null
    set(k: string, v: string): void
    delete(k: string): void
  }
}

const dirs: string[] = []
const tmp = (): string => {
  const dir = mkdtempSync(join(tmpdir(), 'mcp-secrets-'))
  dirs.push(dir)
  return join(dir, 'nested', 'mcp-secrets.json')
}
afterEach(() => {
  for (const d of dirs.splice(0)) rmSync(d, { recursive: true, force: true })
})

/** Stands in for Electron's: reversible, but nothing in the ciphertext resembles the input. */
const fakeSafeStorage = (over: Partial<Record<string, unknown>> = {}) => ({
  isEncryptionAvailable: () => true,
  encryptString: (s: string) =>
    Buffer.from(`v1:${Buffer.from(s).toString('hex')}`),
  decryptString: (b: Buffer) => {
    const text = b.toString()
    if (!text.startsWith('v1:')) throw new Error('not ours')
    return Buffer.from(text.slice(3), 'hex').toString()
  },
  ...over,
})

describe('the secret store', () => {
  test('round-trips a value, and the file holds none of it', () => {
    const file = tmp()
    const store = createSecretStore({ safeStorage: fakeSafeStorage(), file })!
    store.set(
      'mcp/abc/tokens',
      JSON.stringify({ refresh_token: 'super-secret-refresh' }),
    )

    expect(store.get('mcp/abc/tokens')).toContain('super-secret-refresh')
    // the point of the whole thing: what lands on disk is not the token
    expect(readFileSync(file, 'utf8')).not.toContain('super-secret-refresh')
  })

  test('keeps other keys when one is written or deleted', () => {
    const store = createSecretStore({
      safeStorage: fakeSafeStorage(),
      file: tmp(),
    })!
    store.set('mcp/abc/tokens', 'a')
    store.set('mcp/def/tokens', 'b')
    store.delete('mcp/abc/tokens')

    expect(store.get('mcp/abc/tokens')).toBeNull()
    expect(store.get('mcp/def/tokens')).toBe('b')
  })

  test('a value written under a key this machine no longer has reads as absent', () => {
    // a restored backup, or a changed login keychain. Absent means the user signs in again;
    // throwing would leave the connector unusable with nothing to press
    const file = tmp()
    const store = createSecretStore({ safeStorage: fakeSafeStorage(), file })!
    store.set('mcp/abc/tokens', 'a')
    writeFileSync(
      file,
      JSON.stringify({
        'mcp/abc/tokens': Buffer.from('garbage').toString('base64'),
      }),
    )

    expect(store.get('mcp/abc/tokens')).toBeNull()
    // and it can be written over
    store.set('mcp/abc/tokens', 'fresh')
    expect(store.get('mcp/abc/tokens')).toBe('fresh')
  })

  test('a file someone hand-edited into nonsense starts over rather than throwing', () => {
    const file = tmp()
    const store = createSecretStore({ safeStorage: fakeSafeStorage(), file })!
    store.set('mcp/abc/tokens', 'a')
    writeFileSync(file, 'not json at all')

    expect(store.get('mcp/abc/tokens')).toBeNull()
    store.set('mcp/abc/tokens', 'fresh')
    expect(store.get('mcp/abc/tokens')).toBe('fresh')
  })

  test('a missing key reads as null rather than exploding on a first run', () => {
    const store = createSecretStore({
      safeStorage: fakeSafeStorage(),
      file: tmp(),
    })!
    expect(store.get('mcp/never/tokens')).toBeNull()
    expect(() => store.delete('mcp/never/tokens')).not.toThrow()
  })

  test('a host with no keychain stores nothing, rather than storing it in the clear', () => {
    // the fallback for a Linux box with no keyring is signing in again after a restart — never a
    // readable file of refresh tokens
    for (const safeStorage of [
      fakeSafeStorage({ isEncryptionAvailable: () => false }),
      undefined,
    ]) {
      const store = createSecretStore({ safeStorage, file: tmp() })
      expect(store.available()).toBe(false)
      expect(store.get('mcp/x/tokens')).toBeNull()
      expect(() => store.set('mcp/x/tokens', 'secret')).toThrow(/secure storage/)
      expect(() => store.delete('mcp/x/tokens')).not.toThrow()
    }
  })

  // The bug: this is constructed inside `boot()`, on the path that gates spawning the
  // orchestrator, and `isEncryptionAvailable()` reaches the OS keychain. On a runner whose login
  // keychain is not unlocked that call can BLOCK rather than return false — so the app started,
  // never finished starting, and three packaged-restart e2e tests each hung their full 60s with
  // the process alive and no error printed anywhere.
  test('constructing the store does not touch the keychain', () => {
    let probes = 0
    const store = createSecretStore({
      safeStorage: fakeSafeStorage({
        isEncryptionAvailable: () => {
          probes++
          return true
        },
      }),
      file: tmp(),
    })
    expect(probes).toBe(0)

    // first use pays for it, and only once
    store.get('mcp/x/tokens')
    expect(probes).toBe(1)
    store.get('mcp/x/tokens')
    store.available()
    expect(probes).toBe(1)
  })

  test('a keychain that throws on probe reads as unavailable, not as a crash', () => {
    const store = createSecretStore({
      safeStorage: fakeSafeStorage({
        isEncryptionAvailable: () => {
          throw new Error('keychain is locked')
        },
      }),
      file: tmp(),
    })
    expect(store.available()).toBe(false)
    expect(store.get('mcp/x/tokens')).toBeNull()
  })
})
