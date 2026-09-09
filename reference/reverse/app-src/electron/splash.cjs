'use strict'

/**
 * The launch splash — what the window paints before the renderer bundle exists, and therefore the
 * one loading frame the app has to draw twice. The other is src/ui/shell/LoadingScreen.tsx; the
 * geometry below is asserted against its stylesheet in splash.test.ts, because a bar widened in one
 * and not the other puts a visible jump in the middle of every launch.
 *
 * No `require('electron')`, so this unit-tests under Bun.
 */

const fs = require('node:fs')
const path = require('node:path')

// The renderer's mark, from the one copy of it the app has — packaged builds carry the file because
// build.files lists it (package.json). Read once, at require time: it is decoration, so a missing
// file costs the mark and not the splash.
let logo = ''
try {
  logo = fs.readFileSync(path.join(__dirname, '..', 'src', 'ui', 'shell', 'freebuff-logo.svg'), 'utf8')
} catch {}

/**
 * @param {{background: string, splashTrack: string, splashBar: string}} colors
 * @param {'dark'|'light'} theme  the mark is drawn for a dark ground; light inverts it, as app.css does
 */
function splashHtml(colors, theme) {
  return `<!doctype html><html><head><meta charset="utf-8">
<style>
  html,body{height:100%;margin:0}
  /* frameless (linux) has no native frame while booting, so the splash itself must be draggable */
  body{-webkit-app-region:drag;display:flex;align-items:center;justify-content:center;flex-direction:column;
    gap:16px;background:${colors.background}}
  .mark{width:56px;height:56px;${theme === 'light' ? 'filter:invert(1)' : ''}}
  .mark svg{display:block;width:100%;height:100%}
  .bar{width:128px;height:2px;overflow:hidden;border-radius:2px;background:${colors.splashTrack}}
  .bar span{display:block;width:45%;height:100%;border-radius:inherit;background:${colors.splashBar};
    animation:sweep 1.1s ease-in-out infinite alternate}
  @keyframes sweep{from{transform:translateX(-15%)}to{transform:translateX(135%)}}
  @media (prefers-reduced-motion:reduce){.bar span{animation:none}}
</style></head><body>
  ${logo ? `<div class="mark">${logo}</div>` : ''}
  <div class="bar"><span></span></div>
</body></html>`
}

module.exports = { splashHtml }
