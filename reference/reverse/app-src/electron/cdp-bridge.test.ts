/**
 * The bridge is the ONLY path from the orchestrator (a child process) into
 * `webContents.debugger`, i.e. into full CDP on a page. Everything it refuses is
 * a page it must never be able to drive: the app's own privileged renderer, a
 * webview that has navigated off loopback, or a caller who does not hold this
 * boot's token. So the accept/reject matrix is enumerated rather than reasoned
 * about, and every reject case also asserts that no side effect ran — "refused"
 * and "refused after attaching" look identical from the status code alone.
 *
 * The electron objects are already injected (`getWebContents`,
 * `isLoopbackHttpUrl`), so the fakes below are plain EventEmitters and the
 * server under test is the real one, listening on a real loopback port.
 */

import { afterEach, describe, expect, test } from 'bun:test'
import { EventEmitter } from 'node:events'
import { networkInterfaces } from 'node:os'

interface Bridge {
  port: number
  token: string
  close: () => Promise<void>
}

const { createCdpBridge, isLoopbackHost } = require('./cdp-bridge.cjs') as {
  createCdpBridge: (opts: {
    getWebContents: (id: number) => unknown
    isLoopbackHttpUrl: (url: string) => boolean
  }) => Promise<Bridge>
  isLoopbackHost: (host: string | undefined) => boolean
}

class FakeDebugger extends EventEmitter {
  attached = false
  attachedProtocol: string | null = null
  detachCalls = 0
  commands: { method: string; params: unknown }[] = []
  /** per-method override; anything not listed resolves to `undefined` */
  impl: Record<string, (params: unknown) => unknown> = {}

  attach(protocol: string) {
    this.attached = true
    this.attachedProtocol = protocol
  }
  isAttached() {
    return this.attached
  }
  detach() {
    this.detachCalls++
    this.attached = false
  }
  async sendCommand(method: string, params: unknown) {
    this.commands.push({ method, params })
    return this.impl[method]?.(params)
  }
  /** the user opening real devtools kicks us off the page */
  stolen() {
    this.attached = false
    this.emit('detach')
  }
  event(method: string, params: unknown = {}) {
    this.emit('message', {}, method, params)
  }
}

class FakeContents extends EventEmitter {
  debugger = new FakeDebugger()
  type = 'webview'
  url = 'http://127.0.0.1:5173/preview'
  destroyed = false
  throttling: boolean[] = []
  png: Buffer | (() => Buffer) = Buffer.from('\x89PNG-frame')

  constructor(init: Partial<Pick<FakeContents, 'type' | 'url' | 'destroyed' | 'png'>> = {}) {
    super()
    Object.assign(this, init)
  }
  getType() {
    return this.type
  }
  getURL() {
    return this.url
  }
  isDestroyed() {
    return this.destroyed
  }
  setBackgroundThrottling(value: boolean) {
    this.throttling.push(value)
  }
  async capturePage() {
    const png = typeof this.png === 'function' ? this.png() : this.png
    return { toPNG: () => png }
  }
}

const open: Bridge[] = []

afterEach(async () => {
  await Promise.all(open.splice(0).map((b) => b.close().catch(() => {})))
})

interface CallInit {
  method?: string
  body?: unknown
  /** `undefined` = this bridge's token, `null` = send no Authorization header */
  token?: string | null
  authorization?: string
  host?: string
}

interface Harness {
  bridge: Bridge
  /** every id `getWebContents` was asked about — empty means the guards ran first */
  lookups: number[]
  page: (id: number, init?: ConstructorParameters<typeof FakeContents>[0]) => FakeContents
  call: (path: string, init?: CallInit) => Promise<{ status: number; body: any }>
}

async function harness(): Promise<Harness> {
  const pages = new Map<number, FakeContents>()
  const lookups: number[] = []
  const bridge = await createCdpBridge({
    getWebContents: (id) => {
      lookups.push(id)
      return pages.get(id) ?? null
    },
    // stands in for main.cjs's predicate: the preview always lives on the dev/orchestrator origin
    isLoopbackHttpUrl: (url) => /^http:\/\/(127\.0\.0\.1|localhost|\[::1\])(:\d+)?(\/|$)/.test(url),
  })
  open.push(bridge)

  const call = async (path: string, init: CallInit = {}) => {
    const headers: Record<string, string> = {}
    const token = init.token === undefined ? bridge.token : init.token
    if (token !== null) headers.authorization = `Bearer ${token}`
    if (init.authorization !== undefined) headers.authorization = init.authorization
    if (init.host) headers.host = init.host
    const method = init.method ?? 'POST'
    const res = await fetch(`http://127.0.0.1:${bridge.port}${path}`, {
      method,
      headers,
      body:
        method === 'GET' || init.body === undefined
          ? undefined
          : typeof init.body === 'string'
            ? init.body
            : JSON.stringify(init.body),
    })
    return { status: res.status, body: await res.json() }
  }

  return {
    bridge,
    lookups,
    page: (id, init) => {
      const contents = new FakeContents(init)
      pages.set(id, contents)
      return contents
    },
    call,
  }
}

/** attach id 1 and hand back the page it is bound to */
async function attached(h: Harness, id = 1): Promise<FakeContents> {
  const contents = h.page(id)
  expect((await h.call('/attach', { body: { webContentsId: id } })).status).toBe(200)
  return contents
}

function firstExternalIPv4(): string | undefined {
  for (const addrs of Object.values(networkInterfaces())) {
    for (const addr of addrs ?? []) {
      if (addr.family === 'IPv4' && !addr.internal) return addr.address
    }
  }
  return undefined
}

/** a machine with no external interface cannot make the bind assertion at all — skip visibly */
const EXTERNAL_IPV4 = firstExternalIPv4()

describe('isLoopbackHost', () => {
  // an IPv6 Host is always bracketed, which is what lets the trailing ":port" be stripped without
  // eating the address's own colons
  test('accepts the loopback spellings a Host header actually carries', () => {
    for (const host of ['127.0.0.1', '127.0.0.1:8080', 'localhost', 'LocalHost:1', '[::1]', '[::1]:65535']) {
      expect(isLoopbackHost(host)).toBe(true)
    }
  })

  // the whole point of the check: a rebinding page reaches us on a name of its own choosing, and a
  // substring/prefix match would wave every one of these through
  test('rejects hostnames that merely contain or extend a loopback name', () => {
    for (const host of [
      'localhost.evil.com',
      '127.0.0.1.nip.io',
      'notlocalhost',
      'evil.com',
      '0.0.0.0',
      '127.0.0.2',
      '192.168.1.10',
      '[::1',
      '::1]',
      'localhost]',
      '[localhost',
      'localhost:65536',
      '',
    ]) {
      expect(isLoopbackHost(host)).toBe(false)
    }
  })

  // node omits Host on HTTP/1.0 requests; absent must mean deny, not "skip the check"
  test('rejects a missing Host', () => {
    expect(isLoopbackHost(undefined)).toBe(false)
  })
})

describe('binding', () => {
  test.skipIf(!EXTERNAL_IPV4)('listens on loopback only', async () => {
    const h = await harness()
    // a 0.0.0.0 bind would put full CDP on every preview page onto the local network. (a host
    // firewall could also make this unreachable — that direction only weakens the test, it cannot
    // turn a loopback bind into a failure)
    const reached = await fetch(`http://${EXTERNAL_IPV4}:${h.bridge.port}/healthz`, {
      signal: AbortSignal.timeout(2000),
    }).then(
      () => 'reachable',
      () => 'unreachable',
    )
    expect(reached).toBe('unreachable')
  })

  test('a non-loopback Host is rejected even with the right token, before anything is looked up', async () => {
    const h = await harness()
    h.page(1)
    for (const host of ['attacker.example', 'localhost.evil.com', '127.0.0.1.evil.com']) {
      const res = await h.call('/attach', { body: { webContentsId: 1 }, host })
      expect(res.status).toBe(403)
    }
    expect(h.lookups).toEqual([])
  })

  test('loopback aliases are accepted', async () => {
    const h = await harness()
    for (const host of ['localhost:1', '127.0.0.1:65535', '[::1]:1']) {
      expect(await h.call('/healthz', { method: 'GET', host })).toEqual({
        status: 200,
        body: { ok: true },
      })
    }
  })
})

describe('token', () => {
  test('a missing, wrong or malformed token is refused before the webContents is even resolved', async () => {
    const h = await harness()
    const contents = h.page(1)
    const attempts: CallInit[] = [
      { token: null }, // no header at all
      { token: 'not-the-token' },
      { token: '' }, // "Bearer " with nothing after it
      { authorization: h.bridge.token }, // right token, no scheme
      { authorization: `bearer ${h.bridge.token}` }, // scheme is case-sensitive here
      { authorization: `Basic ${h.bridge.token}` },
      { authorization: `Bearer ${h.bridge.token}x` },
    ]
    for (const init of attempts) {
      const res = await h.call('/attach', { body: { webContentsId: 1 }, ...init })
      expect(res.status).toBe(401)
    }
    expect(h.lookups).toEqual([])
    expect(contents.debugger.attached).toBe(false)
  })

  test('the token cannot be smuggled in the query string', async () => {
    const h = await harness()
    const res = await h.call(`/healthz?token=${h.bridge.token}`, { method: 'GET', token: null })
    expect(res.status).toBe(401)
  })

  test("another boot's token does not open this bridge", async () => {
    // the token is minted per createCdpBridge; a stale orchestrator from a previous app launch must
    // not be able to drive the new one's previews
    const a = await harness()
    const b = await harness()
    expect(a.bridge.token).not.toBe(b.bridge.token)
    expect((await b.call('/healthz', { method: 'GET', token: a.bridge.token })).status).toBe(401)
  })
})

describe('/attach', () => {
  test("refuses the app's own renderer", async () => {
    // this is the privilege boundary — a BrowserWindow's webContents runs the preload bridge, so CDP
    // on it would mean Runtime.evaluate against the user's session
    const h = await harness()
    const contents = h.page(1, { type: 'window', url: 'http://127.0.0.1:5173/' })
    const res = await h.call('/attach', { body: { webContentsId: 1 } })
    expect(res.status).toBe(403)
    expect(res.body.error.kind).toBe('not_webview')
    expect(contents.debugger.attached).toBe(false)
  })

  test('refuses a webview that has navigated off loopback', async () => {
    // the preview webview can be driven to any URL by the page itself; once it is on a remote origin
    // it is a stranger's page and CDP on it would read that origin's cookies and DOM
    const h = await harness()
    const contents = h.page(1, { url: 'https://evil.com/' })
    const res = await h.call('/attach', { body: { webContentsId: 1 } })
    expect(res.status).toBe(403)
    expect(res.body.error.kind).toBe('not_webview')
    expect(contents.debugger.attached).toBe(false)
  })

  test('an unknown or already-destroyed webContents is a 404, not a crash', async () => {
    // ids are recycled by electron and the orchestrator's id can be one tab-close stale
    const h = await harness()
    h.page(2, { destroyed: true })
    expect((await h.call('/attach', { body: { webContentsId: 99 } })).status).toBe(404)
    const res = await h.call('/attach', { body: { webContentsId: 2 } })
    expect(res.status).toBe(404)
    expect(res.body.error.kind).toBe('no_webcontents')
  })

  test('attaching enables the event domains without disabling background throttling', async () => {
    const h = await harness()
    const contents = await attached(h)
    expect(contents.debugger.attachedProtocol).toBe('1.3')
    expect(contents.debugger.commands.map((c) => c.method)).toEqual([
      'Runtime.enable',
      'Page.enable',
      'Network.enable',
      'Log.enable',
      'DOM.enable',
      'Accessibility.enable',
    ])
    // Inactive previews stay composited through opacity, so screenshots remain
    // available without forcing their timers/rAF to run while the app is in the
    // background. A preview page can otherwise heat a laptop indefinitely.
    expect(contents.throttling).toEqual([])
  })

  test('re-attaching is idempotent and keeps the events captured so far', async () => {
    // the orchestrator attaches at the start of every preview tool call; re-enabling the domains
    // would be harmless, but resetting the buffer would drop the console errors it is there to read
    const h = await harness()
    const contents = await attached(h)
    contents.debugger.event('Page.loadEventFired')

    expect((await h.call('/attach', { body: { webContentsId: 1 } })).status).toBe(200)

    expect(contents.debugger.commands).toHaveLength(6)
    expect(contents.throttling).toEqual([])
    const events = await h.call('/events?webContentsId=1', { method: 'GET' })
    expect(events.body.events.map((e: any) => e.method)).toEqual(['Page.loadEventFired'])
  })

  test('a failed enable leaves no half-attached session behind', async () => {
    const h = await harness()
    const contents = h.page(1)
    contents.debugger.impl['Network.enable'] = () => {
      throw new Error('target closed')
    }

    const res = await h.call('/attach', { body: { webContentsId: 1 } })
    expect(res.status).toBe(502)
    expect(res.body.error).toEqual({ kind: 'cdp_error', message: 'attach failed: target closed' })

    // the debugger is released and the listeners are gone, so a page that keeps talking cannot grow
    // an orphaned buffer forever
    expect(contents.debugger.detachCalls).toBe(1)
    expect(contents.debugger.listenerCount('message')).toBe(0)
    expect(contents.listenerCount('destroyed')).toBe(0)
    expect((await h.call('/command', { body: { webContentsId: 1, method: 'DOM.getDocument' } })).status).toBe(409)
  })
})

describe('/events', () => {
  test('buffers only the events the preview tools read, and ignores the firehose', async () => {
    // Runtime.enable + Network.enable + DOM.enable put every DOM mutation and script parse on this
    // listener; buffering them would cost main-process memory for data nobody reads
    const h = await harness()
    const contents = await attached(h)
    contents.debugger.event('Runtime.consoleAPICalled', { type: 'error' })
    contents.debugger.event('DOM.childNodeInserted', { nodeId: 4 })
    contents.debugger.event('Debugger.scriptParsed', { scriptId: '7' })
    contents.debugger.event('Network.responseReceived', { status: 500 })
    // an SPA route change commits no document, so this is the only thing that
    // tells the inspector the agent's uids no longer address anything
    contents.debugger.event('Page.navigatedWithinDocument', { url: 'http://127.0.0.1:1/about' })
    // CDP sends some events with no params at all; `undefined` would serialize the key away and
    // the orchestrator reads `event.params.…` unconditionally
    contents.debugger.emit('message', {}, 'Page.loadEventFired', undefined)

    const res = await h.call('/events?webContentsId=1', { method: 'GET' })
    expect(res.body.events.map((e: any) => e.method)).toEqual([
      'Runtime.consoleAPICalled',
      'Network.responseReceived',
      'Page.navigatedWithinDocument',
      'Page.loadEventFired',
    ])
    expect(res.body.events[0].params).toEqual({ type: 'error' })
    expect(res.body.events[3].params).toEqual({})
    expect(res.body.nextSeq).toBe(5)
    expect(res.body.dropped).toBe(0)
  })

  test('`since` returns only what the caller has not seen, and nextSeq is what to ask for next', async () => {
    // the orchestrator polls this in a loop; re-serving old events would make every tool call report
    // the same stale console error
    const h = await harness()
    const contents = await attached(h)
    contents.debugger.event('Log.entryAdded', { text: 'a' })
    contents.debugger.event('Log.entryAdded', { text: 'b' })

    const first = await h.call('/events?webContentsId=1&since=0', { method: 'GET' })
    expect(first.body.events.map((e: any) => e.seq)).toEqual([1, 2])

    const caughtUp = await h.call(`/events?webContentsId=1&since=${first.body.nextSeq - 1}`, {
      method: 'GET',
    })
    expect(caughtUp.body.events).toEqual([])

    contents.debugger.event('Log.entryAdded', { text: 'c' })
    const next = await h.call('/events?webContentsId=1&since=2', { method: 'GET' })
    expect(next.body.events.map((e: any) => e.params.text)).toEqual(['c'])
  })

  test('a chatty page drops the oldest events instead of growing without bound', async () => {
    const h = await harness()
    const contents = await attached(h)
    for (let i = 0; i < 2100; i++) contents.debugger.event('Log.entryAdded', { i })

    const res = await h.call('/events?webContentsId=1', { method: 'GET' })
    expect(res.body.events).toHaveLength(2000)
    // seq keeps counting through the drop, so a poller cannot be handed a stale window
    expect(res.body.events[0].seq).toBe(101)
    expect(res.body.nextSeq).toBe(2101)
    expect(res.body.dropped).toBeGreaterThan(0)
  })

  test('an oversized event params is replaced by a bounded preview', async () => {
    // one 5 MB console.log of a data URL would otherwise sit in main-process memory until detach
    const h = await harness()
    const contents = await attached(h)
    contents.debugger.event('Runtime.consoleAPICalled', { text: 'x'.repeat(100_000) })

    const [event] = (await h.call('/events?webContentsId=1', { method: 'GET' })).body.events
    expect(event.params.truncated).toBe(true)
    expect(event.params.preview).toHaveLength(8 * 1024)
    expect(event.params.preview.startsWith('{"text":"xxx')).toBe(true)
  })

  test('params that cannot be serialized do not take the main process down', async () => {
    // this runs inside an electron event listener: an uncaught throw here is not a failed request,
    // it is an unhandled exception in main
    const h = await harness()
    const contents = await attached(h)
    const circular: Record<string, unknown> = { name: 'loop' }
    circular.self = circular
    contents.debugger.event('Runtime.exceptionThrown', circular)

    const [event] = (await h.call('/events?webContentsId=1', { method: 'GET' })).body.events
    expect(event.params).toEqual({ truncated: true, preview: '[unserializable params]' })
  })

  test('polling a webContents that is not attached is a 409, not an empty page of events', async () => {
    // "no events" and "your session is gone" must not look the same, or the orchestrator would sit
    // in a polling loop against a dead page forever
    const h = await harness()
    h.page(1)
    const res = await h.call('/events?webContentsId=1', { method: 'GET' })
    expect(res.status).toBe(409)
    expect(res.body.error.kind).toBe('not_attached')
  })

  test('a non-numeric webContentsId is rejected', async () => {
    const h = await harness()
    const res = await h.call('/events?webContentsId=abc', { method: 'GET' })
    expect(res.status).toBe(400)
  })
})

describe('/command', () => {
  test('relays the method and params and returns the CDP result', async () => {
    const h = await harness()
    const contents = await attached(h)
    contents.debugger.impl['DOM.getDocument'] = () => ({ root: { nodeId: 1 } })

    const res = await h.call('/command', {
      body: { webContentsId: 1, method: 'DOM.getDocument', params: { depth: -1 } },
    })
    expect(res.body).toEqual({ result: { root: { nodeId: 1 } } })
    expect(contents.debugger.commands.at(-1)).toEqual({
      method: 'DOM.getDocument',
      params: { depth: -1 },
    })
  })

  test('a command with no params sends {} rather than undefined', async () => {
    // electron rejects `sendCommand(method, undefined)` for domains that expect an object
    const h = await harness()
    const contents = await attached(h)
    await h.call('/command', { body: { webContentsId: 1, method: 'Page.reload' } })
    expect(contents.debugger.commands.at(-1)).toEqual({ method: 'Page.reload', params: {} })
  })

  test('a command that resolves with nothing still answers with a result object', async () => {
    // the orchestrator reads `.result`; undefined would serialize away and read as a failure
    const h = await harness()
    await attached(h)
    const res = await h.call('/command', { body: { webContentsId: 1, method: 'Runtime.enable' } })
    expect(res.body).toEqual({ result: {} })
  })

  test('an unattached webContents cannot be driven', async () => {
    // the attach gate is the only thing that checked this page is a loopback webview, so a command
    // path that resolved the id itself would bypass the privilege boundary entirely
    const h = await harness()
    const contents = h.page(1, { type: 'window', url: 'https://evil.com/' })
    const res = await h.call('/command', { body: { webContentsId: 1, method: 'Runtime.evaluate' } })
    expect(res.status).toBe(409)
    expect(res.body.error.kind).toBe('not_attached')
    expect(contents.debugger.commands).toEqual([])
  })

  test('a rejected CDP command is reported as cdp_error with the page’s message', async () => {
    const h = await harness()
    const contents = await attached(h)
    contents.debugger.impl['Runtime.evaluate'] = () => {
      throw new Error('Cannot find context with specified id')
    }
    const res = await h.call('/command', { body: { webContentsId: 1, method: 'Runtime.evaluate' } })
    expect(res.status).toBe(502)
    expect(res.body.error).toEqual({
      kind: 'cdp_error',
      message: 'Cannot find context with specified id',
    })
  })

  test('a missing or non-string method is rejected before it reaches CDP', async () => {
    const h = await harness()
    const contents = await attached(h)
    for (const method of [undefined, '', 42, { method: 'x' }]) {
      const res = await h.call('/command', { body: { webContentsId: 1, method } })
      expect(res.status).toBe(400)
    }
    expect(contents.debugger.commands).toHaveLength(6) // the enables, nothing more
  })

  describe('Freebuff.capturePage', () => {
    test('reads the composited frame instead of relaying a CDP screenshot', async () => {
      // Page.captureScreenshot waits for the renderer to ack a fresh frame, a handshake that never
      // completes on a continuously painting page — the pseudo-command exists to avoid it
      const h = await harness()
      const contents = await attached(h)
      const res = await h.call('/command', {
        body: { webContentsId: 1, method: 'Freebuff.capturePage' },
      })
      expect(res.body.result.data).toBe(Buffer.from('\x89PNG-frame').toString('base64'))
      expect(contents.debugger.commands.some((c) => c.method.startsWith('Freebuff.'))).toBe(false)
    })

    test('it is gated by attach exactly like a real command', async () => {
      const h = await harness()
      h.page(1, { type: 'window' })
      const res = await h.call('/command', {
        body: { webContentsId: 1, method: 'Freebuff.capturePage' },
      })
      expect(res.status).toBe(409)
    })

    test('an empty frame is an error, not an empty screenshot', async () => {
      // a zero-byte PNG round-trips as valid base64; handed to the model it reads as a blank page
      const h = await harness()
      const contents = await attached(h)
      contents.png = Buffer.alloc(0)
      const res = await h.call('/command', {
        body: { webContentsId: 1, method: 'Freebuff.capturePage' },
      })
      expect(res.status).toBe(502)
      expect(res.body.error).toEqual({
        kind: 'cdp_error',
        message: 'capturePage returned an empty image',
      })
    })

    test('a capturePage that throws is reported rather than left hanging', async () => {
      const h = await harness()
      const contents = await attached(h)
      contents.png = () => {
        throw new Error('window is destroyed')
      }
      const res = await h.call('/command', {
        body: { webContentsId: 1, method: 'Freebuff.capturePage' },
      })
      expect(res.status).toBe(502)
      expect(res.body.error.message).toBe('window is destroyed')
    })
  })
})

describe('session lifecycle', () => {
  test('detach releases the debugger and its listeners, and is idempotent', async () => {
    const h = await harness()
    const contents = await attached(h)

    expect((await h.call('/detach', { body: { webContentsId: 1 } })).status).toBe(200)
    expect(contents.debugger.detachCalls).toBe(1)
    // a leaked listener per attach/detach cycle is a slow main-process leak, and the preview tools
    // attach on every single tool call
    expect(contents.debugger.listenerCount('message')).toBe(0)
    expect(contents.debugger.listenerCount('detach')).toBe(0)
    expect(contents.listenerCount('destroyed')).toBe(0)

    // detaching twice (or a page that was never attached) must not throw
    expect((await h.call('/detach', { body: { webContentsId: 1 } })).status).toBe(200)
    expect((await h.call('/detach', { body: { webContentsId: 77 } })).status).toBe(200)
    expect(contents.debugger.detachCalls).toBe(1)
  })

  test('the user opening devtools drops the session instead of leaving it wedged', async () => {
    // only one debugger client per page: when the user attaches devtools we are kicked off, and a
    // session that stayed in the map would answer /command with a permanent CDP error
    const h = await harness()
    const contents = await attached(h)
    contents.debugger.stolen()

    expect((await h.call('/events?webContentsId=1', { method: 'GET' })).status).toBe(409)
    expect(contents.debugger.listenerCount('message')).toBe(0)
  })

  test('a destroyed webContents drops its session', async () => {
    // closing the preview tab destroys the webContents; holding the session would keep the whole
    // event buffer (and the contents object) alive
    const h = await harness()
    const contents = await attached(h)
    contents.destroyed = true
    contents.emit('destroyed')

    expect((await h.call('/command', { body: { webContentsId: 1, method: 'Page.reload' } })).status).toBe(409)
    expect(contents.debugger.listenerCount('message')).toBe(0)
  })

  test('close detaches every live session and stops serving', async () => {
    // app quit: leaving a debugger attached blocks the renderer from shutting down cleanly
    const h = await harness()
    const one = await attached(h, 1)
    const two = await attached(h, 2)

    await h.bridge.close()
    expect(one.debugger.attached).toBe(false)
    expect(two.debugger.attached).toBe(false)

    const reached = await fetch(`http://127.0.0.1:${h.bridge.port}/healthz`, {
      headers: { authorization: `Bearer ${h.bridge.token}` },
      signal: AbortSignal.timeout(2000),
    }).then(
      () => 'reachable',
      () => 'unreachable',
    )
    expect(reached).toBe('unreachable')
  })
})

describe('request validation', () => {
  test('an unparseable body is a 400 rather than a 500', async () => {
    const h = await harness()
    const res = await h.call('/attach', { body: '{"webContentsId":' })
    expect(res.status).toBe(400)
    expect(res.body.error).toEqual({ kind: 'bad_request', message: 'invalid JSON body' })
  })

  test('a webContentsId that is not an integer never reaches the session map', async () => {
    // Number('1') === 1, so a string id would otherwise attach a session the caller can never look
    // up again by the same key
    const h = await harness()
    h.page(1)
    for (const webContentsId of [undefined, '1', 1.5, null, true, '']) {
      const res = await h.call('/attach', { body: { webContentsId } })
      expect(res.status).toBe(400)
    }
    expect(h.lookups).toEqual([])
  })

  test('an unknown route is refused', async () => {
    const h = await harness()
    const res = await h.call('/evaluate', { body: { webContentsId: 1 } })
    expect(res.status).toBe(400)
    expect(res.body.error.message).toBe('unknown route /evaluate')
  })

  test('a GET to anything but /healthz or /events is refused as a method', async () => {
    const h = await harness()
    const res = await h.call('/attach', { method: 'GET' })
    expect(res.status).toBe(400)
    expect(res.body.error.message).toBe('unsupported method')
  })

  test('an oversized body is dropped without attaching, and the bridge keeps serving', async () => {
    // the body is read into memory before it is parsed, so an unbounded read is a main-process OOM
    const h = await harness()
    h.page(1)
    await h
      .call('/attach', {
        body: JSON.stringify({ webContentsId: 1, pad: 'x'.repeat(300 * 1024) }),
      })
      .catch(() => {
        /* the server destroys the request mid-upload, so the client may see a reset */
      })

    expect(h.lookups).toEqual([])
    expect((await h.call('/healthz', { method: 'GET' })).status).toBe(200)
  })
})
