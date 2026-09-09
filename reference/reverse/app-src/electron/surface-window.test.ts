/// <reference types="bun" />
//
// The rule for bringing the app back after a sign-in finished in the browser. Extracted from
// main.cjs precisely so it can be exercised here: main.cjs requires `electron`, so nothing inside
// it is reachable from a test, and this decision has enough cases to be worth pinning.

import { describe, expect, test } from 'bun:test'
import { createRequire } from 'node:module'

const require_ = createRequire(import.meta.url)
const { surfaceWindow } = require_('./surface-window.cjs') as {
  surfaceWindow: (deps: {
    mainWindow: unknown
    threadWindows: Map<string, unknown>
    focusApp: () => void
    platform?: string
  }) => unknown
}

interface FakeWindow {
  destroyed: boolean
  minimized: boolean
  calls: string[]
  isDestroyed(): boolean
  isMinimized(): boolean
  restore(): void
  show(): void
  focus(): void
}

const win = (over: Partial<Pick<FakeWindow, 'destroyed' | 'minimized'>> = {}): FakeWindow => ({
  destroyed: over.destroyed ?? false,
  minimized: over.minimized ?? false,
  calls: [],
  isDestroyed() {
    return this.destroyed
  },
  isMinimized() {
    return this.minimized
  },
  restore() {
    this.minimized = false
    this.calls.push('restore')
  },
  show() {
    this.calls.push('show')
  },
  focus() {
    this.calls.push('focus')
  },
})

const surface = (deps: {
  mainWindow?: unknown
  threadWindows?: Map<string, unknown>
  platform?: string
}) => {
  const appFocused: number[] = []
  const surfaced = surfaceWindow({
    mainWindow: deps.mainWindow ?? null,
    threadWindows: deps.threadWindows ?? new Map(),
    focusApp: () => appFocused.push(1),
    platform: deps.platform ?? 'darwin',
  })
  return { surfaced, appFocused }
}

describe('surfacing the app after a sign-in', () => {
  test('the main window is raised, and the app with it on macOS', () => {
    // focusing a window does not bring the app forward while the browser owns the foreground —
    // which is the whole situation this runs in
    const main = win()
    const { surfaced, appFocused } = surface({ mainWindow: main })

    expect(surfaced).toBe(main)
    expect(main.calls).toEqual(['show', 'focus'])
    expect(appFocused).toHaveLength(1)
  })

  test('a minimized window is restored first', () => {
    // `focus()` on a minimized window leaves it minimized on Windows and Linux: the user would get
    // a taskbar flash and still no app
    const main = win({ minimized: true })
    surface({ mainWindow: main, platform: 'win32' })

    expect(main.calls).toEqual(['restore', 'show', 'focus'])
    expect(main.minimized).toBe(false)
  })

  test('only macOS steals the app-level focus', () => {
    // elsewhere the window call is enough, and the OS decides whether to raise or flash
    const { appFocused } = surface({ mainWindow: win(), platform: 'linux' })
    expect(appFocused).toEqual([])
  })

  test('a pop-out stands in when the main window is gone', () => {
    // macOS keeps the app running with every window closed, and the sign-in being waited on may
    // have started from a thread window's notice card
    const popOut = win()
    const { surfaced } = surface({
      mainWindow: null,
      threadWindows: new Map([['t1', popOut]]),
    })

    expect(surfaced).toBe(popOut)
    expect(popOut.calls).toEqual(['show', 'focus'])
  })

  test('destroyed windows are skipped rather than called', () => {
    // a closed window can still be sitting in the map; calling show() on it throws inside Electron
    const dead = win({ destroyed: true })
    const alive = win()
    const { surfaced } = surface({
      mainWindow: dead,
      threadWindows: new Map([
        ['t1', win({ destroyed: true })],
        ['t2', alive],
      ]),
    })

    expect(surfaced).toBe(alive)
    expect(dead.calls).toEqual([])
  })

  test('with nothing open it does nothing rather than creating a window', () => {
    // this arrives from a background state change; a user who closed every window did not ask for
    // a new one to appear
    const { surfaced, appFocused } = surface({ mainWindow: null })

    expect(surfaced).toBe(null)
    expect(appFocused).toEqual([])
  })
})
