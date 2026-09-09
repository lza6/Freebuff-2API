/**
 * The splash is the app's loading frame drawn a second time, in raw HTML, for a window that exists
 * before the renderer does. So the geometry is asserted against the stylesheet the other copy uses
 * rather than against itself: a bar widened in one and not the other is a visible jump in the middle
 * of every launch, and nothing else would catch it.
 */

import { describe, expect, test } from 'bun:test'
import { createRequire } from 'node:module'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const require_ = createRequire(import.meta.url)
const { splashHtml } = require_('./splash.cjs') as {
  splashHtml: (
    colors: { background: string; splashTrack: string; splashBar: string },
    theme: 'dark' | 'light',
  ) => string
}

const COLORS = { background: '#0c0d0f', splashTrack: '#1b1b1e', splashBar: '#5bbf2f' }
// comments stripped so a selector list is not preceded by prose containing braces
const css = readFileSync(join(import.meta.dir, '../src/ui/styles/app.css'), 'utf8').replace(
  /\/\*[\s\S]*?\*\//g,
  '',
)

describe('the launch splash', () => {
  test('paints the mark and the bar, and says nothing at all', () => {
    // it read "Starting Freebuff orchestrator…": the name of a child process the user does not know
    // they own, in front of a wait they cannot act on
    const html = splashHtml(COLORS, 'dark')

    expect(html).toContain('<svg')
    expect(html).toContain(COLORS.background)
    expect(html).toContain(COLORS.splashTrack)
    expect(html).toContain(COLORS.splashBar)
    expect(html.toLowerCase()).not.toContain('orchestrator')
    // no prose at all: strip the markup and what is left is whitespace
    expect(html.replace(/<style>[\s\S]*?<\/style>/, '').replace(/<[^>]*>/g, '').trim()).toBe('')
  })

  test('inverts the mark under a light theme, as the app does', () => {
    expect(splashHtml(COLORS, 'light')).toContain('filter:invert(1)')
    expect(splashHtml(COLORS, 'dark')).not.toContain('filter:invert(1)')
  })

  test('matches the renderer frame it hands over to', () => {
    const html = splashHtml(COLORS, 'dark')
    // the declarations of the first rule whose selector LIST contains this exact selector
    const rule = (selector: string): string => {
      for (const [, selectors, body] of css.matchAll(/([^{}]+){([^}]*)}/g)) {
        if (selectors.split(',').some((s) => s.trim() === selector)) return body
      }
      return ''
    }

    expect(rule('.loading-screen-bar')).toContain('width: 128px')
    expect(html).toContain('width:128px')
    expect(rule('.loading-screen-bar span')).toContain('width: 45%')
    expect(html).toContain('width:45%')
    expect(rule('.loading-screen-logo')).toContain('width: 56px')
    expect(html).toContain('width:56px')
    expect(rule('.loading-screen')).toContain('gap: 16px')
    expect(html).toContain('gap:16px')
    // the sweep, so the two animations are in step across the handover — and both stand still for
    // anyone who asked the OS for that
    expect(css).toContain('animation: hydration-progress 1.1s ease-in-out infinite alternate')
    expect(html).toContain('animation:sweep 1.1s ease-in-out infinite alternate')
    expect(html).toContain('translateX(-15%)')
    expect(html).toContain('translateX(135%)')
    expect(html).toContain('prefers-reduced-motion:reduce')
  })
})
