const { createHmac, randomBytes, timingSafeEqual } = require('node:crypto')
const net = require('node:net')
const { isMainThread, parentPort, Worker, workerData } = require('node:worker_threads')

function runLifetimeServer(token) {
  const sockets = new Set()
  const server = net.createServer((socket) => {
    const challenge = randomBytes(32).toString('base64url')
    let input = ''
    let heartbeat
    const timeout = setTimeout(() => socket.destroy(), 5000)
    sockets.add(socket)
    socket.on('error', () => {})
    socket.setEncoding('utf8')
    socket.write(`${challenge}\n`)
    socket.on('data', (chunk) => {
      input += chunk
      if (input.length > 1024) {
        socket.destroy()
        return
      }
      const newline = input.indexOf('\n')
      if (newline < 0) return
      const [clientChallenge, proof, extra] = input.slice(0, newline).split(':')
      if (!clientChallenge || !proof || extra !== undefined) {
        socket.destroy()
        return
      }
      const received = Buffer.from(proof, 'base64url')
      const expected = createHmac('sha256', token)
        .update(`client:${challenge}:${clientChallenge}`)
        .digest()
      if (received.length !== expected.length || !timingSafeEqual(received, expected)) {
        socket.destroy()
        return
      }
      clearTimeout(timeout)
      socket.removeAllListeners('data')
      const response = createHmac('sha256', token)
        .update(`server:${challenge}:${clientChallenge}`)
        .digest('base64url')
      socket.write(`${response}\n`)
      heartbeat = setInterval(() => socket.write('ping\n'), 1000)
    })
    socket.once('close', () => {
      clearTimeout(timeout)
      clearInterval(heartbeat)
      sockets.delete(socket)
    })
  })
  server.on('error', (error) => parentPort.postMessage({ type: 'error', message: error.message }))
  parentPort.once('message', () => {
    for (const socket of sockets) socket.destroy()
    sockets.clear()
    server.close()
  })
  server.listen(0, '127.0.0.1', () => {
    const address = server.address()
    if (!address || typeof address === 'string') {
      parentPort.postMessage({ type: 'error', message: 'Could not resolve the shell lifetime server port' })
      server.close()
      return
    }
    parentPort.postMessage({ type: 'listening', port: address.port })
  })
}

if (!isMainThread && workerData?.shellLifetimeToken) {
  runLifetimeServer(workerData.shellLifetimeToken)
}

/**
 * Create a loopback server whose authenticated connection represents the Electron shell's life.
 * A worker thread owns the socket, so native dialogs cannot pause heartbeats and process death still
 * closes everything atomically.
 */
function createShellLifetimeServer(onFailure = () => {}) {
  return new Promise((resolve, reject) => {
    const token = randomBytes(32).toString('base64url')
    const worker = new Worker(__filename, { workerData: { shellLifetimeToken: token } })
    let settled = false
    let closing = false
    let failed = false
    let closePromise
    const fail = (error) => {
      if (closing || failed) return
      failed = true
      if (settled) onFailure(error)
      else reject(error)
    }
    worker.once('error', fail)
    worker.once('exit', (code) => {
      fail(new Error(`Shell lifetime worker exited unexpectedly (code ${code})`))
    })
    worker.on('message', (message) => {
      if (message.type === 'error') {
        fail(new Error(message.message))
        return
      }
      if (message.type !== 'listening' || settled) return
      settled = true
      resolve({
        port: message.port,
        token,
        close() {
          if (closePromise) return closePromise
          if (worker.threadId === -1) return Promise.resolve()
          closing = true
          closePromise = new Promise((done) => worker.once('exit', () => done()))
          worker.postMessage('close')
          return closePromise
        },
      })
    })
  })
}

module.exports = { createShellLifetimeServer }
