function zoomMenuItems(platform = process.platform) {
  if (platform !== 'win32') return [{ role: 'zoomIn' }]
  return [
    { role: 'zoomIn', accelerator: 'CmdOrCtrl+=' },
    { role: 'zoomIn', accelerator: 'CmdOrCtrl+Plus', visible: false },
    { role: 'zoomIn', accelerator: 'CmdOrCtrl+numadd', visible: false },
  ]
}

module.exports = { zoomMenuItems }
