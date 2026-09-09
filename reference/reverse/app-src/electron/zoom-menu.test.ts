import { expect, test } from 'bun:test'

const { zoomMenuItems } = require('./zoom-menu.cjs')

test('win32 binds the visible zoom-in to CmdOrCtrl+= and keeps the role default and numpad add as hidden duplicates', () => {
  expect(zoomMenuItems('win32')).toEqual([
    { role: 'zoomIn', accelerator: 'CmdOrCtrl+=' },
    { role: 'zoomIn', accelerator: 'CmdOrCtrl+Plus', visible: false },
    { role: 'zoomIn', accelerator: 'CmdOrCtrl+numadd', visible: false },
  ])
})

test('darwin and linux keep the single bare zoomIn role with no accelerator override', () => {
  expect(zoomMenuItems('darwin')).toEqual([{ role: 'zoomIn' }])
  expect(zoomMenuItems('linux')).toEqual([{ role: 'zoomIn' }])
})
