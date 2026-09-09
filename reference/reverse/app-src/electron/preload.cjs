/**
 * Preload for the Freebuff renderer. The renderer is the existing
 * self-contained UI served by the Bun orchestrator and talks to it over local
 * HTTP/SSE, so it needs almost nothing from here. We expose a tiny read-only
 * surface for environment/version display and keep contextIsolation on.
 */

const { contextBridge, ipcRenderer, webUtils } = require('electron')

contextBridge.exposeInMainWorld('freebuffDesktop', {
  platform: process.platform,
  // True when this window is frameless and the renderer must draw the
  // minimize/maximize/close buttons. Arrives as an argv flag derived from the
  // frame option this window was built with — a sandboxed preload's require
  // resolves built-ins only, so it cannot ask a shared module.
  customTitleBar: (process.argv || []).includes('--custom-title-bar=1'),
  // The renderer's authorization for state-changing `/api/` calls: a secret
  // the main process minted for this orchestrator. Over IPC, never through
  // argv -- argv is readable with `ps` by anything running as the same user,
  // which is exactly the reader this is meant to exclude.
  // SYNCHRONOUS on purpose. The api layer reads it on the way into every
  // fetch, and making that path await would insert a microtask before the
  // request is issued -- which changes the ordering every caller and test
  // already depends on. Read once and cached on the renderer side.
  apiToken: () => {
    try {
      return ipcRenderer.sendSync('freebuff:apiToken') ?? null
    } catch {
      return null
    }
  },
  // Menu → renderer commands ('new-tab' | 'reopen-tab' | 'close-tab' | 'open-project' | 'pop-out-tab').
  onMenuCommand: (handler) => {
    const listener = (_event, name) => handler(name)
    ipcRenderer.on('menu-cmd', listener)
    return () => ipcRenderer.removeListener('menu-cmd', listener)
  },
  // Resolve a dropped File to its absolute path. Electron 32+ removed File.path,
  // so the composer's drag-and-drop relies on this. Returns '' for non-file blobs.
  getPathForFile: (file) => {
    try {
      return webUtils.getPathForFile(file)
    } catch {
      return ''
    }
  },
  // Native open dialog for the paperclip button — files AND folders, multi-select.
  // Resolves to [{ path, name, isDirectory }] (the main process stats each pick).
  pickAttachments: () => ipcRenderer.invoke('dialog:pickAttachments'),
  // Native open dialog for the image button — files only, filtered to image
  // extensions. Same [{ path, name, isDirectory }] shape as pickAttachments.
  pickImages: () => ipcRenderer.invoke('dialog:pickImages'),
  // Native folder chooser for the project picker. Resolves to the chosen
  // absolute path, or null when the user cancels.
  pickDirectory: () => ipcRenderer.invoke('dialog:pickDirectory'),
  // Write pasted image bytes (a screenshot has no path) to a temp file so it can be
  // attached like any other file. Resolves to { path, name } or null on failure.
  saveClipboardImage: (bytes, ext) => ipcRenderer.invoke('clipboard:saveImage', { bytes, ext }),
  // A dropped path under the OS temp dir (a screenshot dragged from its thumbnail) is deleted
  // right after the drag; resolves to a durable copy, or the same path when it is not temp.
  keepTransientFile: (filePath) => ipcRenderer.invoke('attachment:keepTransient', filePath),
  // The composer only has a local path before the message is sent. Keep file
  // reads in the main process rather than exposing file:// URLs to the renderer.
  readImage: (filePath) => ipcRenderer.invoke('attachment:readImage', filePath),
  // Bring up the system terminal for recovery flows the app can't drive itself
  // (e.g. re-running `claude /login`). mac-only; resolves false elsewhere.
  openTerminal: () => ipcRenderer.invoke('shell:openTerminal'),
  // Open a web URL in the system browser without relying on Chromium's popup
  // activation window. The main process accepts http(s) URLs only.
  openExternal: (url) => ipcRenderer.invoke('shell:openExternal', url),
  // Reveal a changed file in Finder/Explorer without exposing a general-purpose
  // shell path opener to the renderer. The main process contains the relative
  // path inside the supplied thread worktree.
  revealChange: (root, relativePath) =>
    ipcRenderer.invoke('shell:revealChange', { root, relativePath }),
  // Keep app-local project databases discoverable without exposing a renderer-chosen path or the
  // profile's sibling state.json to the native shell.
  revealAppData: () => ipcRenderer.invoke('shell:revealAppData'),
  detectOpenTargets: () => ipcRenderer.invoke('shell:detectOpenTargets'),
  openIn: (request) => ipcRenderer.invoke('shell:openIn', request),
  // Current { fullScreen, maximized } — same shape onWindowStateChange pushes,
  // for renderers that subscribe after the initial push (or after a reload).
  windowState: () => ipcRenderer.invoke('window:state'),
  // Frameless-window controls; the resulting state arrives on 'window-state'.
  minimizeWindow: () => ipcRenderer.invoke('window:minimize'),
  toggleMaximizeWindow: () => ipcRenderer.invoke('window:toggleMaximize'),
  closeWindow: (rendererClosedThread) =>
    ipcRenderer.invoke('window:close', rendererClosedThread === true),
  openThreadWindow: (threadId, projectPath, focus) =>
    ipcRenderer.invoke('window:openThread', { threadId, projectPath, focus }),
  // Mark this window as needing attention without taking focus from the user's current app.
  requestAttention: () => ipcRenderer.send('window:requestAttention'),
  clearAttention: () => ipcRenderer.send('window:clearAttention'),
  // A sign-in the user left the app to finish has just completed, so the shell brings its window
  // back in front of the browser. `send`, not `invoke`: nothing waits on the result, and a renderer
  // that is merely reporting an event should not be able to hold a promise open on the main process.
  signInCompleted: () => ipcRenderer.send('window:signInCompleted'),
  tabContextMenu: () => ipcRenderer.invoke('menu:tabContext'),
  saveMarkdown: (filename, content) => ipcRenderer.invoke('dialog:saveMarkdown', { filename, content }),
  onWindowStateChange: (handler) => {
    const listener = (_event, state) => handler(state)
    ipcRenderer.on('window-state', listener)
    return () => ipcRenderer.removeListener('window-state', listener)
  },
  // In-app updater surface (see updater.cjs). Main owns the state and pushes
  // the whole value; null means there is nothing to show.
  onUpdateCard: (handler) => {
    const listener = (_event, card) => handler(card)
    ipcRenderer.on('update-card', listener)
    return () => ipcRenderer.removeListener('update-card', listener)
  },
  // What main is showing right now, for a renderer that mounted or reloaded
  // after the push — including a download already in progress.
  pendingUpdate: () => ipcRenderer.invoke('update:pending'),
  // Update decisions are validated against the current main-process phase.
  updateAction: (action) => ipcRenderer.invoke('update:action', action),
  // Drives "Install When Idle": main installs once this has been false a while.
  reportBusy: (busy) => ipcRenderer.send('update:busy', busy),
  setTheme: (theme) => ipcRenderer.send('theme:set', theme),
  onTheme: (handler) => {
    const listener = (_event, theme) => handler(theme)
    ipcRenderer.on('theme', listener)
    return () => ipcRenderer.removeListener('theme', listener)
  },
})
