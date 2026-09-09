'use strict'

/**
 * CDP bridge — a tiny loopback HTTP server in the Electron MAIN process that
 * lets the orchestrator (a separate Bun child process) drive the Preview tab's
 * <webview> page over the Chrome DevTools Protocol.
 *
 * Why it exists: the preview_* agent tools run in the orchestrator, but
 * `webContents.debugger` — the only per-page CDP surface Electron offers — is a
 * main-process API. There is deliberately NO app-wide `--remote-debugging-port`
 * (that would expose every webContents, including the privileged app renderer);
 * instead this bridge scopes CDP to explicitly attached preview webviews.
 *
 * The bridge is intentionally THIN: attach/detach, a generic command relay, and
 * a raw ring buffer of the few CDP events the tools care about (console,
 * network, page load). All interpretation — a11y snapshots, uid maps, log
 * formatting — lives orchestrator-side in preview-inspector.ts. CDP events only
 * arrive on the main-process debugger object, and buffering them raw for the
 * orchestrator to pull (`GET /events?since=`) avoids inventing a main→
 * orchestrator push channel.
 *
 * Access control mirrors freebuff-mcp-server.ts: loopback bind + loopback Host
 * check (DNS-rebinding pages) + a per-boot random bearer token that main hands
 * ONLY to its spawned orchestrator via env. `/attach` additionally refuses any
 * webContents that is not a loopback-URL <webview>, so the bridge can never
 * touch the app's own renderer.
 *
 * No top-level `require('electron')` — the electron objects are injected
 * (`getWebContents`, `isLoopbackHttpUrl`) so the module unit-tests under Bun.
 */

const http = require('node:http')
const crypto = require('node:crypto')

/** Max buffered events per attached webContents; oldest dropped first. */
const RING_CAP = 2000
/** Max serialized size of one event's params — a chatty page (huge console
 *  payloads, big response headers) must not balloon main-process memory. */
const PARAMS_CAP_BYTES = 8 * 1024
/** Max accepted request body. Commands are small; anything bigger is abuse. */
const BODY_CAP_BYTES = 256 * 1024

/** The only CDP events worth keeping. Everything else (DOM mutations, frame
 *  trees, …) is request/response via /command and never buffered. */
const BUFFERED_EVENTS = new Set([
  'Runtime.consoleAPICalled',
  'Runtime.exceptionThrown',
  'Log.entryAdded',
  'Network.requestWillBeSent',
  'Network.responseReceived',
  'Network.loadingFailed',
  'Page.loadEventFired',
  'Page.frameStoppedLoading',
  'Page.frameNavigated',
  // A React/Next route change is history.pushState, which commits no document
  // and so fires no frameNavigated — this is the only signal that the page the
  // agent's uids were minted against has been replaced under it.
  'Page.navigatedWithinDocument',
])

/** CDP domains enabled on attach so events flow from page load onward. */
const ENABLE_COMMANDS = [
  'Runtime.enable',
  'Page.enable',
  'Network.enable',
  'Log.enable',
  'DOM.enable',
  'Accessibility.enable',
]

/** Loopback-only Host check (same rationale as freebuff-mcp-server.ts): a
 *  non-loopback Host means the request reached us via something other than a
 *  direct localhost connection, e.g. a DNS-rebinding webpage. */
function isLoopbackHost(host) {
  if (!host) return false
  const match = /^(\[[^\]]+\]|[^:]+)(?::(\d+))?$/.exec(host)
  if (!match || (match[2] !== undefined && Number(match[2]) > 65_535)) return false
  const name = match[1].toLowerCase()
  return name === '127.0.0.1' || name === 'localhost' || name === '[::1]'
}

/** Keep an event's params under PARAMS_CAP_BYTES. Small params pass through
 *  untouched; oversized ones are replaced by a truncation marker carrying a
 *  prefix of the JSON so the orchestrator can still show *something*. */
function capParams(params) {
  let json
  try {
    json = JSON.stringify(params ?? {})
  } catch {
    return { truncated: true, preview: '[unserializable params]' }
  }
  if (json.length <= PARAMS_CAP_BYTES) return params ?? {}
  return { truncated: true, preview: json.slice(0, PARAMS_CAP_BYTES) }
}

/**
 * Start the bridge.
 *
 * @param {object} opts
 * @param {(id: number) => import('electron').WebContents | null | undefined} opts.getWebContents
 * @param {(url: string) => boolean} opts.isLoopbackHttpUrl
 * @returns {Promise<{ port: number, token: string, close: () => Promise<void> }>}
 */
function createCdpBridge({ getWebContents, isLoopbackHttpUrl }) {
  const token = crypto.randomUUID()

  /** webContentsId → { contents, events, nextSeq, dropped, cleanup } */
  const sessions = new Map()

  function dropSession(id) {
    const session = sessions.get(id)
    if (!session) return
    sessions.delete(id)
    session.cleanup()
    try {
      if (session.contents.debugger.isAttached()) session.contents.debugger.detach()
    } catch {
      /* already detached / destroyed */
    }
  }

  async function attach(id) {
    if (sessions.has(id)) return // idempotent — capture keeps running
    const contents = getWebContents(id)
    if (!contents || contents.isDestroyed()) {
      throw bridgeError(404, 'no_webcontents', `no webContents with id ${id}`)
    }
    // The privilege boundary: only preview webviews (preload-free, loopback-
    // pinned by the will-attach-webview guard) may ever be driven over CDP.
    if (contents.getType() !== 'webview') {
      throw bridgeError(403, 'not_webview', `webContents ${id} is not a <webview>`)
    }
    if (!isLoopbackHttpUrl(contents.getURL())) {
      throw bridgeError(403, 'not_webview', `webContents ${id} is not showing a loopback page`)
    }

    if (!contents.debugger.isAttached()) contents.debugger.attach('1.3')

    const session = {
      contents,
      events: [],
      nextSeq: 1,
      dropped: 0,
      cleanup: () => {
        contents.debugger.removeListener('message', onMessage)
        contents.debugger.removeListener('detach', onDetach)
        contents.removeListener('destroyed', onDestroyed)
      },
    }
    const onMessage = (_event, method, params) => {
      if (!BUFFERED_EVENTS.has(method)) return
      session.events.push({ seq: session.nextSeq++, method, params: capParams(params), ts: Date.now() })
      if (session.events.length > RING_CAP) {
        session.events.splice(0, session.events.length - RING_CAP)
        session.dropped++
      }
    }
    const onDetach = () => dropSession(id)
    const onDestroyed = () => dropSession(id)
    contents.debugger.on('message', onMessage)
    contents.debugger.on('detach', onDetach)
    contents.once('destroyed', onDestroyed)
    sessions.set(id, session)

    try {
      for (const method of ENABLE_COMMANDS) await contents.debugger.sendCommand(method)
      // Leave Chromium's background throttling alone. Hidden preview entries
      // stay composited through opacity (see .preview-live-hidden), so capture
      // still works while Blink is free to idle their timers and rAF. Disabling
      // throttling here kept every mounted preview running at full rate while
      // the whole app was backgrounded — exactly the kind of invisible work
      // that makes a laptop hot after the user has left the Preview tab.
      // Electron 33/macOS verification: a minimized guest settled at zero rAF
      // callbacks while capturePage still completed, including at opacity 0.
    } catch (err) {
      dropSession(id)
      throw bridgeError(502, 'cdp_error', `attach failed: ${err?.message ?? err}`)
    }
  }

  function bridgeError(status, kind, message) {
    const err = new Error(message)
    err.status = status
    err.kind = kind
    return err
  }

  function readBody(req) {
    return new Promise((resolve, reject) => {
      const chunks = []
      let size = 0
      req.on('data', (chunk) => {
        size += chunk.length
        if (size > BODY_CAP_BYTES) {
          reject(bridgeError(400, 'bad_request', 'request body too large'))
          req.destroy()
          return
        }
        chunks.push(chunk)
      })
      req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')))
      req.on('error', reject)
    })
  }

  const server = http.createServer(async (req, res) => {
    const respond = (status, payload) => {
      res.writeHead(status, { 'content-type': 'application/json' })
      res.end(JSON.stringify(payload))
    }
    try {
      if (!isLoopbackHost(req.headers.host)) {
        return respond(403, { error: { kind: 'bad_request', message: 'forbidden host' } })
      }
      if (req.headers.authorization !== `Bearer ${token}`) {
        return respond(401, { error: { kind: 'bad_request', message: 'missing or invalid token' } })
      }

      const url = new URL(req.url, 'http://127.0.0.1')

      if (req.method === 'GET' && url.pathname === '/healthz') {
        return respond(200, { ok: true })
      }

      if (req.method === 'GET' && url.pathname === '/events') {
        const id = Number(url.searchParams.get('webContentsId'))
        const since = Number(url.searchParams.get('since') ?? 0)
        if (!Number.isInteger(id)) throw bridgeError(400, 'bad_request', 'webContentsId must be an integer')
        const session = sessions.get(id)
        if (!session) throw bridgeError(409, 'not_attached', `webContents ${id} is not attached`)
        const events = session.events.filter((e) => e.seq > since)
        return respond(200, { events, nextSeq: session.nextSeq, dropped: session.dropped })
      }

      if (req.method !== 'POST') throw bridgeError(400, 'bad_request', 'unsupported method')

      let body = {}
      const raw = await readBody(req)
      if (raw) {
        try {
          body = JSON.parse(raw)
        } catch {
          throw bridgeError(400, 'bad_request', 'invalid JSON body')
        }
      }
      const id = body.webContentsId
      if (!Number.isInteger(id)) throw bridgeError(400, 'bad_request', 'webContentsId must be an integer')

      switch (url.pathname) {
        case '/attach': {
          await attach(id)
          return respond(200, { ok: true })
        }
        case '/detach': {
          dropSession(id) // idempotent
          return respond(200, { ok: true })
        }
        case '/command': {
          const session = sessions.get(id)
          if (!session) throw bridgeError(409, 'not_attached', `webContents ${id} is not attached`)
          if (typeof body.method !== 'string' || !body.method) {
            throw bridgeError(400, 'bad_request', 'method must be a string')
          }
          // Pseudo-command, not relayed to CDP: grab the LATEST already-
          // composited frame via webContents.capturePage(). This is the DEFAULT
          // screenshot path (see preview-inspector.ts), because it reads the
          // compositor with no renderer round-trip: ~10ms whatever the page is
          // doing, where CDP's Page.captureScreenshot force-redraws and waits
          // for a fresh-frame ack that rAF-driven and very tall pages never
          // give. It captures the viewport only, and — like every other pixel
          // path — hangs if the guest isn't being composited at all, so callers
          // must bound it. Same attach gating as real commands, so it can only
          // ever see preview webviews.
          if (body.method === 'Freebuff.capturePage') {
            try {
              const image = await session.contents.capturePage()
              const png = image.toPNG()
              if (png.length === 0) {
                throw new Error('capturePage returned an empty image')
              }
              return respond(200, { result: { data: png.toString('base64') } })
            } catch (err) {
              throw bridgeError(502, 'cdp_error', err?.message ?? String(err))
            }
          }
          try {
            // If the client aborts mid-command (its deadline, or the bounded
            // screenshot attempt), this await stays pending until the command
            // settles or the session drops — debugger.sendCommand has no
            // cancellation. Accepted cost: one small stranded closure per
            // abandoned command (freed on detach/destroy), with any late
            // respond() going to an already-dead socket.
            const result = await session.contents.debugger.sendCommand(body.method, body.params ?? {})
            return respond(200, { result: result ?? {} })
          } catch (err) {
            throw bridgeError(502, 'cdp_error', err?.message ?? String(err))
          }
        }
        default:
          throw bridgeError(400, 'bad_request', `unknown route ${url.pathname}`)
      }
    } catch (err) {
      const status = err?.status ?? 500
      const kind = err?.kind ?? 'bad_request'
      respond(status, { error: { kind, message: err?.message ?? String(err) } })
    }
  })

  return new Promise((resolve, reject) => {
    server.once('error', reject)
    server.listen(0, '127.0.0.1', () => {
      const port = server.address().port
      resolve({
        port,
        token,
        close: () =>
          new Promise((done) => {
            for (const id of [...sessions.keys()]) dropSession(id)
            server.close(() => done())
          }),
      })
    })
  })
}

module.exports = { createCdpBridge, isLoopbackHost }
