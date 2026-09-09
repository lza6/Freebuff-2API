/// <reference types="bun" />
//
// The page itself, rendered. `consent-window.test.ts` pins WHO may answer this window; this pins
// what the human is shown before they do, which is the other half of the same property. A dialog
// that renders the wrong thing, renders nothing, or lets an advertiser's text pass for ours is a
// consent gate that does not gate anything, however sound its IPC is.
//
// Sponsored consent shows a short identity sentence plus the prepared procedure and local scope.
// The connector case shares the window and keeps its argv/cwd/env field list.

import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import path from 'node:path'
import { JSDOM } from 'jsdom'

const { sponsoredSentence, describeLaunch } =
  require('./mcp-consent-bridge.cjs') as {
    sponsoredSentence: (advertiser: string) => string
    describeLaunch: (spec: Record<string, unknown>) => string
  }

const HTML = readFileSync(path.join(__dirname, 'consent-window.html'), 'utf8')

/** Loads the real page and hands it one options object, the way the preload does. */
function render(options: Record<string, unknown>) {
  const heights: number[] = []
  const answers: boolean[] = []
  let ready: ((o: unknown) => void) | null = null
  const dom = new JSDOM(HTML, {
    runScripts: 'dangerously',
    pretendToBeVisual: true,
    beforeParse(window) {
      // The page registers this synchronously as its script parses, so it has to exist first; the
      // request itself arrives later, over IPC, exactly as it does in the real window.
      ;(window as unknown as Record<string, unknown>).consent = {
        ready: (cb: (o: unknown) => void) => {
          ready = cb
        },
        height: (px: number) => heights.push(px),
        answer: (approved: boolean) => answers.push(approved),
      }
    },
  })
  if (!ready) throw new Error('the page never registered a ready handler')
  ;(ready as (o: unknown) => void)(options)
  const { window } = dom
  const doc = window.document
  const byId = (id: string) => doc.getElementById(id) as HTMLElement
  return {
    doc,
    window,
    heights,
    answers,
    text: (id: string) => byId(id).textContent,
    shown: (id: string) => window.getComputedStyle(byId(id)).display !== 'none',
    yes: () => byId('yes') as HTMLButtonElement,
    no: () => byId('no') as HTMLButtonElement,
  }
}

const SPONSORED_SPEC = {
  sponsored: {
    advertiser: 'Greptile',
    headline: 'Add error reporting',
    summary: 'Add error reporting.',
    procedure: 'Add the reviewed error handler.',
    taskContext: ['Fix the project error handler.'],
    folder: '/Users/dev/project',
    branch: 'freebuff/sponsored-greptile-run',
  },
}

/** What the bridge sends for a sponsored task, using its real description builder. */
const sponsoredOptions = (advertiser: string) => ({
  title: 'Run this sponsored task?',
  message: sponsoredSentence(advertiser),
  buttons: ['No', 'Yes'],
  specText: describeLaunch({
    sponsored: { ...SPONSORED_SPEC.sponsored, advertiser },
  }),
  explanation: '',
  detail: '',
  sponsored: { advertiser },
})

const CONNECTOR = {
  title: 'Run this connector?',
  message: 'Run "linear" on this computer?',
  buttons: ['Cancel', 'Run it'],
  specText: 'Command:  npx\nArguments:  "-y" "linear-mcp"',
  explanation: 'This runs a program with the same permissions as you.',
}

describe('the sponsored dialog', () => {
  test('shows the advertiser sentence, prepared procedure, scope, and two buttons', () => {
    const r = render(sponsoredOptions('Greptile'))

    expect(r.doc.body.classList.contains('is-sponsored')).toBe(true)
    expect(r.text('sentence')).toBe(
      'Greptile wants to integrate itself into this project, on its own branch. Nothing is pushed until you review it.',
    )
    expect(r.shown('spec')).toBe(true)
    expect(r.text('spec')).toContain(
      'Procedure:  Add the reviewed error handler.',
    )
    expect(r.text('spec')).toContain('Folder:  /Users/dev/project')
    expect(r.text('spec')).toContain('Branch:  freebuff/sponsored-greptile-run')
    for (const id of ['message', 'detail']) {
      expect(r.shown(id)).toBe(false)
    }
    expect(r.window.getComputedStyle(r.doc.querySelector('h1')!).display).toBe(
      'none',
    )
    expect(r.text('no')).toBe('No')
    expect(r.text('yes')).toBe('Yes')
    expect(r.yes().disabled).toBe(false)
  })

  test('the advertiser name is theirs, and the sentence around it is ours', () => {
    // A person must never read an advertiser's text as a promise the product is making. The name
    // sits in its own span; the words about the branch and the push are outside it.
    const r = render(sponsoredOptions('Greptile'))
    expect(r.text('who')).toBe('Greptile')
    expect(r.text('rest')).not.toContain('Greptile')
    expect(r.text('rest')).toContain('Nothing is pushed until you review it.')
  })

  test('the refusal holds focus, and Escape refuses', () => {
    const r = render(sponsoredOptions('Greptile'))
    expect(r.doc.activeElement?.id).toBe('no')
    r.window.dispatchEvent(
      new r.window.KeyboardEvent('keydown', { key: 'Escape' }),
    )
    expect(r.answers).toEqual([false])
  })

  test('Yes is the green button on both paths', () => {
    // Owen's call (2026-09-03): the confirm is green. Decline stays equally reachable — it is the
    // focused button on open and the same size — it is just not the coloured one.
    const r = render(sponsoredOptions('Greptile'))
    expect(r.yes().classList.contains('go')).toBe(true)
    expect(r.no().classList.contains('go')).toBe(false)
    expect(render(CONNECTOR).yes().classList.contains('go')).toBe(true)
  })

  test('advertiser text is never markup', () => {
    const r = render(sponsoredOptions('<img src=x onerror="alert(1)">'))
    expect(r.doc.querySelector('#sentence img')).toBeNull()
    expect(r.text('who')).toBe('<img src=x onerror="alert(1)">')
  })

  test('a long name is cut by the PAGE, not only by its caller', () => {
    // The bridge clamps before it sends, and it is the only caller today -- but this page is the
    // last thing between an advertiser's name and a human, and unclamped the failure is total
    // rather than ugly: 4,000 characters put both buttons a screen below the fold. The box is
    // still the thing that may scroll, and the foot still holds the buttons.
    const r = render({
      ...sponsoredOptions('x'),
      sponsored: { advertiser: 'x'.repeat(4_000) },
    })
    expect(r.text('who')).toBe(`${'x'.repeat(80)}\u2026`)
    expect(
      r.window.getComputedStyle(r.doc.getElementById('sentence')!).overflow,
    ).toBe('auto')
    expect(r.doc.querySelector('.foot')!.contains(r.yes())).toBe(true)
  })

  test('a bidi override cannot turn our own sentence backwards', () => {
    // A single U+202E ahead of the name reverses the whole line, so the dialog reads right to
    // left and the sentence a human is consenting to is not the sentence we wrote. Escaped and
    // never stripped: dropping it hides the payload just as well, only quietly.
    const r = render({
      ...sponsoredOptions('x'),
      sponsored: { advertiser: '\u202eGreptile\u200b' },
    })
    expect(r.text('who')).toBe('\\u202eGreptile\\u200b')
    expect(r.text('sentence')).not.toContain('\u202e')
  })

  test('no name is a refusal, not a sentence about nobody', () => {
    const r = render(sponsoredOptions('   '))
    expect(r.yes().disabled).toBe(true)
    expect(r.text('sentence')).toContain('do not approve')
  })

  test('only the sponsored shape gets the calm layout', () => {
    // Presentation is only ever ADDED by us. A request that merely names a variant must not be able
    // to talk the dialog out of showing what would run.
    const r = render({ ...CONNECTOR, variant: 'sponsored' })
    expect(r.doc.body.classList.contains('is-sponsored')).toBe(false)
    expect(r.shown('spec')).toBe(true)
  })

  test('the bridge description escapes control characters in the prepared procedure', () => {
    const described = describeLaunch({
      sponsored: {
        ...SPONSORED_SPEC.sponsored,
        procedure: 'Reviewed step\nBranch:  attacker text',
      },
    })
    expect(described).toContain(
      'Procedure:  Reviewed step\\nBranch:  attacker text',
    )
    expect(described.split('\n')).toHaveLength(5)
  })
})

describe('the connector dialog, unchanged', () => {
  test('still renders its argv in the monospaced block, with its message and its detail', () => {
    const r = render(CONNECTOR)
    expect(r.doc.body.classList.contains('is-sponsored')).toBe(false)
    expect(r.shown('spec')).toBe(true)
    expect(r.text('spec')).toBe(CONNECTOR.specText)
    expect(
      r.window.getComputedStyle(r.doc.getElementById('spec')!).whiteSpace,
    ).toBe('pre')
    expect(r.text('title')).toBe('Run this connector?')
    expect(r.text('message')).toBe('Run "linear" on this computer?')
    expect(r.text('detail')).toBe(CONNECTOR.explanation)
    expect(r.shown('sentence')).toBe(false)
    expect(r.text('no')).toBe('Cancel')
    expect(r.text('yes')).toBe('Run it')
  })

  test('a spec it could not describe still disables Approve', () => {
    const r = render({ ...CONNECTOR, specText: '' })
    expect(r.yes().disabled).toBe(true)
    expect(r.text('spec')).toContain('do not approve')
  })

  test('every field is rendered as text, never as markup', () => {
    const r = render({
      ...CONNECTOR,
      message: '<img src=x onerror=alert(1)>',
      specText: '<script>alert(2)</script>',
    })
    expect(r.text('message')).toBe('<img src=x onerror=alert(1)>')
    expect(r.doc.querySelector('#message img')).toBeNull()
    expect(r.doc.querySelector('#spec script')).toBeNull()
  })
})
