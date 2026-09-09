/// <reference types="bun" />

import { describe, expect, test } from 'bun:test'
import { createRequire } from 'node:module'

const require_ = createRequire(import.meta.url)
const { clearWindowAttention, requestWindowAttention } = require_('./window-attention.cjs') as {
  clearWindowAttention: (win: FakeWindow | null) => boolean
  requestWindowAttention: (win: FakeWindow | null, appFocused?: boolean) => boolean
}

interface FakeWindow {
  destroyed: boolean
  focused: boolean
  flashes: boolean[]
  focusHandlers: (() => void)[]
  isDestroyed(): boolean
  isFocused(): boolean
  flashFrame(value: boolean): void
  once(event: 'focus', handler: () => void): void
}

const win = (focused = false): FakeWindow => ({
  destroyed: false,
  focused,
  flashes: [],
  focusHandlers: [],
  isDestroyed() {
    return this.destroyed
  },
  isFocused() {
    return this.focused
  },
  flashFrame(value) {
    this.flashes.push(value)
  },
  once(_event, handler) {
    this.focusHandlers.push(handler)
  },
})

describe('requestWindowAttention', () => {
  test('does nothing when the requesting window is already focused', () => {
    const focused = win(true)
    expect(requestWindowAttention(focused)).toBe(false)
    expect(focused.flashes).toEqual([])
  })

  test('does nothing while another app window is focused', () => {
    const background = win()
    expect(requestWindowAttention(background, true)).toBe(false)
    expect(background.flashes).toEqual([])
  })

  test('flashes an unfocused window until it receives focus', () => {
    const background = win()
    expect(requestWindowAttention(background)).toBe(true)
    expect(background.flashes).toEqual([true])

    background.focused = true
    background.focusHandlers[0]()
    expect(background.flashes).toEqual([true, false])
  })

  test('coalesces repeated requests while the window is already flashing', () => {
    const background = win()
    expect(requestWindowAttention(background)).toBe(true)
    expect(requestWindowAttention(background)).toBe(false)
    expect(background.flashes).toEqual([true])
    expect(background.focusHandlers).toHaveLength(1)
  })

  test('can stop flashing before the window receives focus', () => {
    const background = win()
    requestWindowAttention(background)
    expect(clearWindowAttention(background)).toBe(true)
    expect(background.flashes).toEqual([true, false])
  })

  test('does not touch a destroyed window', () => {
    const destroyed = win()
    destroyed.destroyed = true
    expect(requestWindowAttention(destroyed)).toBe(false)
    expect(destroyed.flashes).toEqual([])
  })
})
