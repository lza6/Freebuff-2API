'use strict'

/**
 * The only bridge this window gets: receive one request, send one answer.
 *
 * Deliberately tiny. It exposes no filesystem, no shell, no app state and no way to ask for
 * anything — a page that somehow ran hostile script here could still only answer the question it
 * was already being asked, which the user is looking at.
 */

const { contextBridge, ipcRenderer } = require('electron')

const CHANNEL = 'freebuff:mcp-consent'

contextBridge.exposeInMainWorld('consent', {
  ready: (onRequest) => {
    ipcRenderer.on(CHANNEL, (_event, message) => {
      if (message?.type === 'request') onRequest(message.options)
    })
    ipcRenderer.send(CHANNEL, { type: 'ready' })
  },
  answer: (approved) => ipcRenderer.send(CHANNEL, { type: 'answer', approved }),
  height: (px) => ipcRenderer.send(CHANNEL, { type: 'height', px }),
})
