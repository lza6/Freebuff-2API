import { describe, expect, test } from 'bun:test'
import { createRequire } from 'node:module'

const require = createRequire(import.meta.url)
const { postMutation } = require('./orchestrator-request.cjs') as {
  postMutation: (
    fetchImpl: typeof fetch,
    url: URL,
    launchId: string | null,
    body: unknown,
  ) => Promise<unknown>
}

describe('postMutation', () => {
  test('authenticates a main-process mutation with the orchestrator launch id', async () => {
    let seen: { url: string; init?: RequestInit } | undefined
    const request = ((url: URL, init?: RequestInit) => {
      seen = { url: url.toString(), init }
      return Promise.resolve({})
    }) as typeof fetch

    await postMutation(
      request,
      new URL('http://127.0.0.1:8787/api/thread/t1/close'),
      'secret',
      {},
    )

    expect(seen).toEqual({
      url: 'http://127.0.0.1:8787/api/thread/t1/close',
      init: {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          'x-freebuff-launch-id': 'secret',
        },
        body: '{}',
      },
    })
  })

  test('still works in dev when the orchestrator has no launch id', async () => {
    let headers: HeadersInit | undefined
    const request = ((_url: URL, init?: RequestInit) => {
      headers = init?.headers
      return Promise.resolve({})
    }) as typeof fetch

    await postMutation(request, new URL('http://127.0.0.1:8787/api/test'), null, {})

    expect(headers).toEqual({ 'content-type': 'application/json' })
  })
})
