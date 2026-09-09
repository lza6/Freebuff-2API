/// <reference types="bun" />
//
// The window is only worth anything if it cannot be answered by anything except itself. So what
// these pin is who may reply, and that every way of NOT answering counts as a refusal — a dialog
// that can be dismissed into an approval is worse than no dialog.

import { describe, expect, test } from 'bun:test'

const { createConsentWindow, CHANNEL } = require('./consent-window.cjs') as {
  createConsentWindow: (
    deps: Record<string, unknown>,
  ) => (
    parent: unknown,
    options: Record<string, unknown>,
  ) => Promise<{ response: number }>
  CHANNEL: string
}

const OPTIONS = {
  title: 'Run this connector?',
  message: 'Run "linear" on this computer?',
  detail:
    'Command:  npx\n\nThis runs a program with the same permissions as you.',
  buttons: ['Cancel', 'Run it'],
}

/** Just enough Electron to drive the thing, with a handle on the wiring under test. */
function harness(over: { failToOpen?: boolean; readyTimeoutMs?: number } = {}) {
  const listeners: ((event: unknown, message: unknown) => void)[] = []
  const sent: unknown[] = []
  const state = {
    destroyed: false,
    shown: false,
    loaded: null as string | null,
  }
  const guards = new Map<string, (event: { preventDefault: () => void }) => void>()
  let windowOpen: (() => { action: string }) | null = null
  const webContents = {
    send: (_c: string, m: unknown) => sent.push(m),
    on: (event: string, cb: (e: { preventDefault: () => void }) => void) =>
      guards.set(event, cb),
    setWindowOpenHandler: (cb: () => { action: string }) => {
      windowOpen = cb
    },
  }
  const sized: [number, number][] = []
  const win = {
    webContents,
    setContentSize: (w: number, h: number) => sized.push([w, h]),
    isDestroyed: () => state.destroyed,
    destroy: () => {
      state.destroyed = true
    },
    show: () => {
      state.shown = true
    },
    on: (_event: string, cb: () => void) => {
      win.closed = cb
    },
    loadFile: (p: string) => {
      state.loaded = p
      return Promise.resolve()
    },
    closed: () => {},
  }
  const fallbackCalls: unknown[] = []
  const ask = createConsentWindow({
    BrowserWindow: function () {
      if (over.failToOpen) throw new Error('no display')
      return win
    },
    ipcMain: {
      on: (_c: string, cb: (event: unknown, message: unknown) => void) =>
        listeners.push(cb),
      removeAllListeners: () => void listeners.splice(0),
      removeHandler: () => {},
    },
    fallback: (_parent: unknown, options: unknown) => {
      fallbackCalls.push(options)
      return Promise.resolve({ response: 0 })
    },
    theme: () => 'dark',
    ...(over.readyTimeoutMs === undefined
      ? {}
      : { readyTimeoutMs: over.readyTimeoutMs }),
  })
  const emit = (sender: unknown, message: unknown) => {
    for (const cb of [...listeners]) cb({ sender }, message)
  }
  return {
    ask,
    win,
    state,
    sent,
    emit,
    fallbackCalls,
    webContents,
    sized,
    guards,
    windowOpen: () => windowOpen,
  }
}

describe('the consent window', () => {
  test('hands the question to its own page, and shows itself only once that page is ready', async () => {
    const h = harness()
    const answer = h.ask(null, OPTIONS)
    expect(h.state.shown).toBe(false)

    h.emit(h.webContents, { type: 'ready' })
    expect(h.sent).toEqual([{ type: 'request', options: OPTIONS }])
    expect(h.state.shown).toBe(true)

    h.emit(h.webContents, { type: 'answer', approved: true })
    expect(await answer).toEqual({ response: 1 })
  })

  // Nothing bounded this wait, and `win.show()` only runs in the `ready` branch — so a page that
  // loaded but never signalled left an invisible window, a promise that never settled, and a caller
  // whose one-at-a-time guard was held forever. Every later request was then refused with no dialog
  // anywhere on screen, which reads exactly like the app ignoring you.
  test('a page that loads but never signals falls back to the OS dialog instead of hanging', async () => {
    const h = harness({ readyTimeoutMs: 20 })
    const answer = h.ask(null, OPTIONS)

    expect(await answer).toEqual({ response: 0 })
    // the decision went to a human via the OS box, not to a default
    expect(h.fallbackCalls).toEqual([OPTIONS])
    // and the dead window is gone rather than left invisible on screen
    expect(h.state.shown).toBe(false)
    expect(h.state.destroyed).toBe(true)
  })

  test('a page that does signal is never overtaken by that deadline', async () => {
    const h = harness({ readyTimeoutMs: 20 })
    const answer = h.ask(null, OPTIONS)
    h.emit(h.webContents, { type: 'ready' })
    await new Promise((r) => setTimeout(r, 40))

    // the human is still being asked; the deadline did not answer for them
    expect(h.fallbackCalls).toEqual([])
    expect(h.state.destroyed).toBe(false)
    h.emit(h.webContents, { type: 'answer', approved: true })
    expect(await answer).toEqual({ response: 1 })
  })

  test('the window cannot be navigated, redirected, or made to open another', async () => {
    const h = harness()
    const answer = h.ask(null, OPTIONS)
    h.emit(h.webContents, { type: 'ready' })

    for (const event of ['will-navigate', 'will-redirect', 'will-attach-webview']) {
      let prevented = false
      h.guards.get(event)?.({ preventDefault: () => (prevented = true) })
      expect(`${event}:${prevented}`).toBe(`${event}:true`)
    }
    expect(h.windowOpen()?.()).toEqual({ action: 'deny' })

    h.emit(h.webContents, { type: 'answer', approved: false })
    await answer
  })

  test('an answer from any other webContents is ignored', async () => {
    // this is the whole reason the window exists rather than a modal in the app: the renderer that
    // draws agent output and MCP tool results must not be able to approve anything
    const h = harness()
    const answer = h.ask(null, OPTIONS)
    h.emit(h.webContents, { type: 'ready' })

    const impostor = { send: () => {} }
    h.emit(impostor, { type: 'answer', approved: true })
    let settled = false
    void answer.then(() => (settled = true))
    await new Promise((r) => setTimeout(r, 10))
    expect(settled).toBe(false)

    // and the real one still works
    h.emit(h.webContents, { type: 'answer', approved: true })
    expect(await answer).toEqual({ response: 1 })
  })

  test('it takes the height the page asks for, within bounds', async () => {
    // a four-line command in a fixed 420px window is mostly empty; a forty-line argv is a scrollbar
    const h = harness()
    const answer = h.ask(null, OPTIONS)
    h.emit(h.webContents, { type: 'ready' })

    h.emit(h.webContents, { type: 'height', px: 288 })
    h.emit(h.webContents, { type: 'height', px: 4000 })
    h.emit(h.webContents, { type: 'height', px: 10 })
    h.emit(h.webContents, { type: 'height', px: NaN })
    expect(h.sized).toEqual([
      [520, 288],
      [520, 720],
      [520, 200],
    ])

    // and a height message is not an answer
    h.emit(h.webContents, { type: 'answer', approved: false })
    expect(await answer).toEqual({ response: 0 })
  })

  test('closing the window is a refusal, not a shrug', async () => {
    const h = harness()
    const answer = h.ask(null, OPTIONS)
    h.emit(h.webContents, { type: 'ready' })
    ;(h.win as unknown as { closed: () => void }).closed()
    expect(await answer).toEqual({ response: 0 })
  })

  test('declining resolves as declining', async () => {
    const h = harness()
    const answer = h.ask(null, OPTIONS)
    h.emit(h.webContents, { type: 'ready' })
    h.emit(h.webContents, { type: 'answer', approved: false })
    expect(await answer).toEqual({ response: 0 })
  })

  test('a second answer cannot change the first', async () => {
    const h = harness()
    const answer = h.ask(null, OPTIONS)
    h.emit(h.webContents, { type: 'ready' })
    h.emit(h.webContents, { type: 'answer', approved: false })
    h.emit(h.webContents, { type: 'answer', approved: true })
    expect(await answer).toEqual({ response: 0 })
  })

  test('a host that cannot open a window falls back to the OS dialog, never to yes', async () => {
    const h = harness({ failToOpen: true })
    expect(await h.ask(null, OPTIONS)).toEqual({ response: 0 })
    expect(h.fallbackCalls).toEqual([OPTIONS])
  })

  test('the channel is the one the preload talks on', () => {
    expect(CHANNEL).toBe('freebuff:mcp-consent')
  })
})
