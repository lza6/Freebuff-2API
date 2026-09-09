/**
 * Two gates in here are the difference between a normal test run and one that
 * reaches out to the release server, or worse: `quitAndInstall` in the middle of
 * a developer's session. `isPackaged` and `FREEBUFF_DISABLE_UPDATE_CHECK` both
 * have to hold on their own, so each is pinned separately. The rest of the file
 * is the UX contract around electron-updater: no dismissal survives a restart
 * (the next launch installs on its own), a background check must never nag, and
 * every way an install can fail must still end at a manual download rather than
 * silence.
 *
 * The launch path installs on its own, so the tests that pin it are load-bearing in a way the
 * others are not: a regression there swaps a running app's binary, and the only thing standing
 * between a user and that is the Cancel button and the busy check.
 *
 * `init()` reaches for `require('electron')` and `require('electron-updater')`
 * itself, so there is no injection seam — electron is replaced with a plain
 * object (it does not exist under bun at all), while the packaged updater is a
 * REAL file in a temp `resourcesPath`, which is what lets the two require
 * branches be told apart. `init()` also memoizes a module-level singleton, so
 * each boot() re-requires updater.cjs from a cleared require cache.
 */

import {
  afterAll,
  afterEach,
  beforeEach,
  describe,
  expect,
  mock,
  spyOn,
  test,
} from 'bun:test'
import { EventEmitter } from 'node:events'
import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'

const UPDATER = path.join(import.meta.dir, 'updater.cjs')

interface DialogOptions {
  message: string
  detail?: string
  buttons: string[]
  cancelId?: number
}

/** stands in for builder-util-runtime's token; the module only constructs it and cancels it */
class FakeCancellationToken {
  cancelled = false
  cancel() {
    this.cancelled = true
  }
}

class FakeAutoUpdater extends EventEmitter {
  autoDownload?: boolean
  autoInstallOnAppQuit?: boolean
  autoRunAppAfterInstall?: boolean
  disableWebInstaller?: boolean
  requestHeaders?: Record<string, string>
  logger?: unknown
  checks = 0
  downloads = 0
  /** the cancellation token handed to the last downloadUpdate call */
  token: FakeCancellationToken | null = null
  quitArgs: unknown[][] = []
  checkImpl: () => unknown = () => ({ isUpdateAvailable: false })
  downloadImpl: () => Promise<unknown> = async () => ({})
  quitImpl: () => void = () => {}

  async checkForUpdates() {
    this.checks++
    return this.checkImpl()
  }
  downloadUpdate(token?: FakeCancellationToken) {
    this.downloads++
    this.token = token ?? null
    return this.downloadImpl()
  }
  quitAndInstall(...args: unknown[]) {
    this.quitArgs.push(args)
    this.quitImpl()
  }
  // bun freezes a mocked module's exports on first require, so these two live for the whole file and
  // are reset per test rather than replaced
  reset() {
    this.removeAllListeners()
    this.autoDownload = undefined
    this.autoInstallOnAppQuit = undefined
    this.autoRunAppAfterInstall = undefined
    this.disableWebInstaller = undefined
    this.requestHeaders = undefined
    this.logger = undefined
    this.checks = 0
    this.downloads = 0
    this.token = null
    this.quitArgs = []
    this.checkImpl = () => ({ isUpdateAvailable: false })
    this.downloadImpl = async () => ({})
    this.quitImpl = () => {}
  }
}

let dialogs: DialogOptions[] = []
let opened: string[] = []
let openExternalImpl: (url: string) => Promise<void> = async (url) =>
  void opened.push(url)
let userData = ''

type IpcFn = (event: unknown, ...args: unknown[]) => unknown
const handlers = new Map<string, IpcFn>()
const listeners = new Map<string, IpcFn>()
const powerListeners = new Map<string, (() => void)[]>()

// electron does not exist under bun and init() requires it directly, so this stands in for it. The
// objects are stable for the whole file (bun freezes a mocked module's exports on first require);
// each method reads the per-test state above.
mock.module('electron', () => ({
  app: {
    getPath: (name: string) =>
      name === 'userData' ? userData : path.join(userData, name),
  },
  dialog: {
    showMessageBox: async (_parent: unknown, options: DialogOptions) => {
      dialogs.push(options)
      return { response: options.cancelId ?? 0 }
    },
  },
  shell: { openExternal: (url: string) => openExternalImpl(url) },
  ipcMain: {
    handle: (channel: string, fn: IpcFn) => void handlers.set(channel, fn),
    on: (channel: string, fn: IpcFn) => void listeners.set(channel, fn),
  },
  powerMonitor: {
    on: (event: string, fn: () => void) =>
      void powerListeners.set(event, [
        ...(powerListeners.get(event) ?? []),
        fn,
      ]),
  },
}))

/** what the renderer would invoke over the preload bridge */
const action = (name: string): unknown =>
  handlers.get('update:action')!(null, name)
/** what the renderer's busy heartbeat sends */
const reportBusy = (busy: boolean): void =>
  void listeners.get('update:busy')!(null, busy)
const wake = (): void => powerListeners.get('resume')?.forEach((fn) => fn())

/** the dev branch: `require('electron-updater')` out of node_modules */
const devUpdater = new FakeAutoUpdater()
mock.module('electron-updater', () => ({
  autoUpdater: devUpdater,
  CancellationToken: FakeCancellationToken,
}))

/** the packaged branch: a REAL file at `resourcesPath/electron-updater.cjs`, so the two require
 *  branches can be told apart by which fake ends up configured. It re-exports CancellationToken
 *  too — scripts/electron-updater-entry.ts has to keep doing the same, or downloads stop being
 *  cancellable in packaged builds only */
const packagedUpdater = new FakeAutoUpdater()
const resourcesDir = fs.mkdtempSync(
  path.join(os.tmpdir(), 'freebuff-resources-'),
)
fs.writeFileSync(
  path.join(resourcesDir, 'electron-updater.cjs'),
  'module.exports = {\n' +
    '  get autoUpdater() { return globalThis.__freebuffPackagedAutoUpdater },\n' +
    '  get CancellationToken() { return globalThis.__freebuffCancellationToken },\n' +
    '}\n',
)

/** let every already-queued continuation run — a real macrotask, not a guessed number of ticks */
const flush = () => new Promise((resolve) => setTimeout(resolve, 0))

function deferred<T = void>() {
  let resolve!: (value: T) => void
  let reject!: (error: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

const restores: (() => void)[] = []

/** process.platform / arch / execPath / resourcesPath are read at init and at install time */
function stubProcess(prop: string, value: string) {
  const previous = Object.getOwnPropertyDescriptor(process, prop)
  Object.defineProperty(process, prop, {
    value,
    configurable: true,
    writable: true,
  })
  restores.push(() => {
    if (previous) Object.defineProperty(process, prop, previous)
    else delete (process as unknown as Record<string, unknown>)[prop]
  })
}

interface Booted {
  updater: FakeAutoUpdater
  /** what the module scheduled: [delay, callback] pairs */
  timeouts: { delay: number; run: () => void }[]
  intervals: { delay: number; run: () => void }[]
  checkNow: (mode?: 'launch' | 'session' | 'interactive') => Promise<void>
  handle: unknown
  /** the launch check, which downloads and installs on its own */
  launch: () => void
  /** the wall-clock ticker, which decides for itself whether anything is due */
  tick: () => void
}

interface BootOptions {
  isPackaged?: boolean
  currentVersion?: string
  disableCheck?: boolean
  releaseFlavor?: 'standard' | 'baseline'
  /** simulates the window having gone away mid-install */
  window?: { setProgressBar: (fraction: number) => void } | null
  /** the shared busy signal main.cjs also reads; omitted, init makes its own */
  busy?: unknown
}

const progress: number[] = []

interface CardState {
  phase: string
  version?: string
  currentVersion: string
  fraction?: number
  notes?: string
}
/** every card state main pushed to the renderer, oldest first */
let pushed: (CardState | null)[] = []
/** the phases the card moved through, ignoring repeated progress frames */
const phases = (): string[] =>
  pushed
    .map((c) => c?.phase ?? 'gone')
    .filter((phase, i, all) => phase !== all[i - 1])
const shown = (): CardState | null =>
  pushed.length ? pushed[pushed.length - 1] : null

// the module reads the wall clock to decide when a check is due, so tests own it
let now = 1_700_000_000_000
const advance = (ms: number): void => void (now += ms)
const MINUTE = 60 * 1000
const reportSettledIdle = (): void => {
  reportBusy(false)
  advance(MINUTE + 1)
}

function boot(options: BootOptions = {}): Booted {
  const {
    isPackaged = true,
    currentVersion = '1.0.0',
    disableCheck = false,
    releaseFlavor = 'standard',
    busy,
  } = options
  const window =
    options.window === undefined
      ? {
          setProgressBar: (fraction: number) => progress.push(fraction),
          isDestroyed: () => false,
          webContents: {
            send: (_channel: string, payload: CardState | null) =>
              pushed.push(payload),
          },
        }
      : options.window

  if (disableCheck) process.env.FREEBUFF_DISABLE_UPDATE_CHECK = '1'
  else delete process.env.FREEBUFF_DISABLE_UPDATE_CHECK

  const timeouts: { delay: number; run: () => void }[] = []
  const intervals: { delay: number; run: () => void }[] = []
  const timer = spyOn(globalThis, 'setTimeout').mockImplementation(((
    run: () => void,
    delay: number,
  ) => {
    timeouts.push({ run, delay })
    return { unref: () => {} }
  }) as never)
  const interval = spyOn(globalThis, 'setInterval').mockImplementation(((
    run: () => void,
    delay: number,
  ) => {
    intervals.push({ run, delay })
    return { unref: () => {} }
  }) as never)

  let handle: {
    checkNow: (mode?: 'launch' | 'session' | 'interactive') => Promise<void>
  }
  try {
    delete require.cache[UPDATER]
    handle = require(UPDATER).init({
      currentVersion,
      isPackaged,
      releaseFlavor,
      getWindow: () => window,
      ...(busy === undefined ? {} : { busy }),
    })
  } finally {
    timer.mockRestore()
    interval.mockRestore()
  }

  return {
    updater: isPackaged ? packagedUpdater : devUpdater,
    timeouts,
    intervals,
    checkNow: (mode) => handle.checkNow(mode),
    handle,
    launch: () => timeouts[0].run(),
    tick: () => intervals[0].run(),
  }
}

/** an update is waiting; the returned boot is one `checkNow` away from the prompt */
function bootWithUpdate(version = '2.0.0', options: BootOptions = {}): Booted {
  const booted = boot(options)
  booted.updater.checkImpl = () => ({
    isUpdateAvailable: true,
    updateInfo: { version },
  })
  return booted
}

function stateFile(): string {
  return path.join(userData, 'update-state.json')
}

beforeEach(() => {
  const inheritedGate = process.env.FREEBUFF_DISABLE_UPDATE_CHECK
  restores.push(() => {
    if (inheritedGate === undefined)
      delete process.env.FREEBUFF_DISABLE_UPDATE_CHECK
    else process.env.FREEBUFF_DISABLE_UPDATE_CHECK = inheritedGate
  })
  userData = fs.mkdtempSync(path.join(os.tmpdir(), 'freebuff-userdata-'))
  dialogs = []
  opened = []
  openExternalImpl = async (url) => void opened.push(url)
  pushed = []
  progress.length = 0
  handlers.clear()
  listeners.clear()
  // each boot registers its own resume listener; a leaked one would fire a
  // previous test's closure
  powerListeners.clear()
  now = 1_700_000_000_000
  const clock = spyOn(Date, 'now').mockImplementation(() => now)
  restores.push(() => clock.mockRestore())
  devUpdater.reset()
  packagedUpdater.reset()
  ;(globalThis as Record<string, unknown>).__freebuffPackagedAutoUpdater =
    packagedUpdater
  ;(globalThis as Record<string, unknown>).__freebuffCancellationToken =
    FakeCancellationToken
  stubProcess('resourcesPath', resourcesDir)
  // the module logs through console; keep the suite output readable
  for (const level of ['debug', 'info', 'warn', 'error'] as const) {
    const spy = spyOn(console, level).mockImplementation(() => {})
    restores.push(() => spy.mockRestore())
  }
})

afterEach(() => {
  // LIFO: a test that re-stubs a property it already stubbed (execPath, below) records two
  // restores, and unwinding them in order would leave the stub installed for every later FILE
  for (const restore of restores.splice(0).reverse()) restore()
  delete (globalThis as Record<string, unknown>).__freebuffPackagedAutoUpdater
  delete (globalThis as Record<string, unknown>).__freebuffCancellationToken
  fs.rmSync(userData, { recursive: true, force: true })
  delete require.cache[UPDATER]
})

afterAll(() => {
  fs.rmSync(resourcesDir, { recursive: true, force: true })
})

describe('the dev gate', () => {
  test('an unpackaged app schedules nothing and checks nothing', async () => {
    // every `bun test` run and every `bun run app` is this branch: a check here talks to the release
    // server, and an accepted prompt would quitAndInstall over the developer's checkout
    const b = boot({ isPackaged: false })
    expect(b.timeouts).toEqual([])
    expect(b.intervals).toEqual([])

    await b.checkNow()
    expect(b.updater.checks).toBe(0)
    expect(dialogs).toEqual([])
  })

  test('an explicit check in dev says why instead of doing nothing', async () => {
    // the menu item is always enabled; silence would read as "checked, nothing found"
    const b = boot({ isPackaged: false })
    await b.checkNow('interactive')
    expect(b.updater.checks).toBe(0)
    expect(shown()).toEqual({ phase: 'unavailable', currentVersion: '1.0.0' })
    expect(dialogs).toEqual([])
  })
})

describe('the FREEBUFF_DISABLE_UPDATE_CHECK gate', () => {
  test('a packaged build with the env var set never schedules a check', async () => {
    // the e2e harness runs the packaged app; a background prompt there steals focus mid-test, and an
    // accepted one replaces the binary under test
    const b = boot({ disableCheck: true })
    expect(b.timeouts).toEqual([])
    expect(b.intervals).toEqual([])
    expect(b.updater.checks).toBe(0)
  })

  test('it disables the schedule, not the menu item', async () => {
    // the var means "do not check on your own", so an explicit user request still works
    const b = bootWithUpdate('2.0.0', { disableCheck: true })
    await b.checkNow('interactive')
    expect(b.updater.checks).toBe(1)
  })
})

describe('the feed User-Agent', () => {
  // Names the build that is ASKING, so the feed measures what is installed
  // instead of inferring it from download volume. The shape is fixed by
  // `parseUpdaterUserAgent` in web's api/desktop/updates/[target]/[file] — read
  // its docstring before changing it. These are the two shapes route.test.ts
  // proves that parser accepts, so asserting them here closes the loop.
  for (const [platform, arch, flavour, expected] of [
    ['darwin', 'arm64', 'standard', 'Freebuff/0.0.84 (darwin; arm64; standard)'],
    // baseline is its own Windows artifact — a rollout that cannot tell the two
    // apart cannot tell whether either landed
    ['win32', 'x64', 'baseline', 'Freebuff/0.0.84 (win32; x64; baseline)'],
  ] as const) {
    test(`a ${flavour} ${platform}/${arch} build announces itself`, () => {
      stubProcess('platform', platform)
      stubProcess('arch', arch)
      const b = boot({ currentVersion: '0.0.84', releaseFlavor: flavour })
      expect(b.updater.requestHeaders?.['User-Agent']).toBe(expected)
    })
  }
})

describe('scheduling', () => {
  test('a packaged build checks immediately at launch and then ticks every minute', async () => {
    // no waiting period: the launch check downloads on its own now, so there is nothing to hold
    // back for a half-drawn window
    const b = boot()
    expect(b.timeouts.map((t) => t.delay)).toEqual([0])
    // the timer only TICKS on the minute; whether a check is due is re-derived from the wall clock
    expect(b.intervals.map((t) => t.delay)).toEqual([MINUTE])
  })

  test('both scheduled checks are background ones — no dialog when there is nothing to install', async () => {
    // these fire with no user waiting on them; an "up to date" box would be a popup out of nowhere
    // at launch and then again every couple of hours, forever
    const b = boot()
    b.launch()
    await flush()
    expect(b.updater.checks).toBe(1)

    advance(2 * 60 * MINUTE)
    b.tick()
    await flush()
    expect(b.updater.checks).toBe(2)
    expect(dialogs).toEqual([])
  })

  test('a tick before the interval is up checks nothing', async () => {
    // it fires 60x an hour; without the wall-clock gate that would be 60 requests an hour
    const b = boot()
    b.launch()
    await flush()

    advance(30 * MINUTE)
    b.tick()
    await flush()
    expect(b.updater.checks).toBe(1)

    advance(90 * MINUTE + 1000)
    b.tick()
    await flush()
    expect(b.updater.checks).toBe(2)
  })

  test('an app left open across a sleep checks as soon as the machine wakes', async () => {
    // THE bug the wall-clock derivation exists for: timers do not run while the machine sleeps, so
    // a laptop that is closed every night never reaches the next interval and never sees a release
    const b = boot()
    b.launch()
    await flush()

    advance(18 * 60 * MINUTE) // slept through the whole interval
    wake()
    await flush()
    expect(b.updater.checks).toBe(2)
  })

  test('a failed check retries in minutes, not hours', async () => {
    // being offline for the launch check must not mean no updates for the rest of the session
    const b = boot()
    b.updater.checkImpl = () => {
      throw new Error('offline')
    }
    b.launch()
    await flush()
    expect(b.updater.checks).toBe(1)

    advance(9 * MINUTE)
    b.tick()
    await flush()
    expect(b.updater.checks).toBe(1)

    advance(2 * MINUTE)
    b.tick()
    await flush()
    expect(b.updater.checks).toBe(2)
  })

  test('init is idempotent, so a dock re-activate cannot double the timers', async () => {
    // main.cjs re-runs boot() when macOS re-activates the app with no window open
    const b = boot()
    const again = require(UPDATER).init({
      currentVersion: '9.9.9',
      isPackaged: true,
      getWindow: () => null,
    })
    expect(again).toBe(b.handle)
    expect(b.timeouts).toHaveLength(1)
    expect(b.intervals).toHaveLength(1)
  })

  test('the packaged build loads the updater bundled into resources, not node_modules', async () => {
    // electron-builder ships no node_modules (`"!**/node_modules/**"`), so a packaged require of
    // 'electron-updater' would throw at startup and there would be no updates at all
    boot({ isPackaged: true })
    expect(packagedUpdater.autoDownload).toBe(false)
    expect(devUpdater.autoDownload).toBeUndefined()

    boot({ isPackaged: false })
    expect(devUpdater.autoDownload).toBe(false)
  })
})

describe('checking', () => {
  test('being up to date is reported only when the user asked', async () => {
    const b = boot()
    await b.checkNow()
    expect(shown()).toBeNull()

    await b.checkNow('interactive')
    expect(phases()).toEqual(['checking', 'current'])
    expect(shown()).toEqual({ phase: 'current', currentVersion: '1.0.0' })
    expect(dialogs).toEqual([])
  })

  test('a failed check is silent in the background and reported when interactive', async () => {
    const b = boot()
    b.updater.checkImpl = () => {
      throw new Error('getaddrinfo ENOTFOUND')
    }
    await b.checkNow()
    expect(shown()).toBeNull()

    await b.checkNow('interactive')
    expect(shown()).toEqual({ phase: 'check-failed', currentVersion: '1.0.0' })
    expect(dialogs).toEqual([])
  })

  test('a failed check does not wedge the checker', async () => {
    // the in-flight flag is cleared in a `finally`; leaving it set would mean one offline moment
    // disables updates until the app is restarted
    const b = boot()
    b.updater.checkImpl = () => {
      throw new Error('offline')
    }
    await b.checkNow()

    b.updater.checkImpl = () => ({
      isUpdateAvailable: true,
      updateInfo: { version: '2.0.0' },
    })
    await b.checkNow()
    expect(b.updater.checks).toBe(2)
    expect(shown()!.version).toBe('2.0.0')
  })

  test('a check that arrives while one is running is dropped', async () => {
    // the 6-hourly timer and the menu item can land together; two in-flight checks means two prompts
    // for the same version
    const b = boot()
    const gate = deferred<{ isUpdateAvailable: boolean }>()
    b.updater.checkImpl = () => gate.promise

    const first = b.checkNow()
    await Promise.resolve()
    await b.checkNow('interactive')
    expect(b.updater.checks).toBe(1)

    gate.resolve({ isUpdateAvailable: false })
    await first
  })

  test('a malformed check result is treated as "no update", not as one', async () => {
    // an update server that answers with nothing useful must not produce a prompt for `undefined`
    const b = boot()
    b.updater.checkImpl = () => null
    await b.checkNow()
    expect(dialogs).toEqual([])
  })

  test('an update announced with no version is dropped, not downloaded', async () => {
    // the launch path acts on this answer with no user in the loop, so a provider that says
    // "there is an update" and names nothing must not reach downloadUpdate — and must not throw
    // out of a promise nobody is awaiting, which in the main process takes the app with it
    const b = boot()
    b.updater.checkImpl = () => ({ isUpdateAvailable: true, updateInfo: {} })
    b.launch()
    await flush()
    expect(b.updater.downloads).toBe(0)
    expect(shown()).toBeNull()
  })
})

/** an installed bundle on a mac, so the platform the tests run on stops mattering */
function pinInstalledMac() {
  stubProcess('platform', 'darwin')
  stubProcess('arch', 'arm64')
  stubProcess('execPath', '/Applications/Freebuff.app/Contents/MacOS/Freebuff')
}

describe('the launch update', () => {
  beforeEach(pinInstalledMac)

  test('a new version downloads and installs itself, without asking', async () => {
    // the point of the whole flow: a user who never restarts, and a user who does, both end up on
    // the current version without being handed a decision
    const b = bootWithUpdate('2.0.0')
    reportSettledIdle()
    b.launch()
    await flush()

    expect(dialogs).toEqual([])
    expect(b.updater.downloads).toBe(1)
    // (isSilent=false, isForceRunAfter=true): they get the app back after the restart
    expect(b.updater.quitArgs).toEqual([[false, true]])
    expect(phases()).toEqual(['downloading', 'installing'])
  })

  test('a launch download cannot restart before the renderer reports activity', async () => {
    const b = bootWithUpdate('2.0.0')
    b.launch()
    await flush()

    expect(b.updater.quitArgs).toEqual([])
    expect(shown()!.phase).toBe('armed')

    reportBusy(false)
    advance(MINUTE + 1)
    reportBusy(false)
    expect(b.updater.quitArgs).toEqual([[false, true]])
  })

  test('the card shows what is happening while it downloads', async () => {
    // the download is the only warning that the app is about to restart, so it has to be visible
    // from the first byte, not once it finishes
    const b = bootWithUpdate('2.0.0')
    const finish = deferred()
    b.updater.downloadImpl = () => finish.promise
    reportSettledIdle()

    b.launch()
    await flush()
    expect(shown()).toEqual({
      phase: 'downloading',
      fraction: 0,
      version: '2.0.0',
      currentVersion: '1.0.0',
    })

    b.updater.emit('download-progress', { percent: 42 })
    expect(shown()).toEqual({
      phase: 'downloading',
      fraction: 0.42,
      version: '2.0.0',
      currentVersion: '1.0.0',
    })
    expect(progress).toEqual([0.42])

    finish.resolve()
    await flush()
    expect(shown()!.phase).toBe('installing')
  })

  test('Cancel stops the download and leaves the app on the version it has', async () => {
    // the escape hatch. Without a working cancel, "it updates on its own" is a threat rather than a
    // feature: the app would restart under someone mid-thought with no way to stop it
    const b = bootWithUpdate('2.0.0')
    const finish = deferred()
    b.updater.downloadImpl = () => finish.promise

    b.launch()
    await flush()
    expect(action('cancel')).toBe('cancelled')

    expect(b.updater.token!.cancelled).toBe(true)
    expect(shown()).toBeNull()
    expect(progress.at(-1)).toBe(-1) // the taskbar bar is cleared too
    b.updater.emit('download-progress', { percent: 75 })
    expect(progress.at(-1)).toBe(-1)
    expect(shown()).toBeNull()

    // and even if the download resolves anyway, nothing is installed
    finish.resolve()
    await flush()
    expect(b.updater.quitArgs).toEqual([])
  })

  test('a cancelled version is not picked back up for the rest of the session', async () => {
    // the next tick would otherwise start the same download again, minutes later
    const b = bootWithUpdate('2.0.0')
    const finish = deferred()
    b.updater.downloadImpl = () => finish.promise
    b.launch()
    await flush()
    action('cancel')
    finish.resolve()
    await flush()

    b.updater.downloadImpl = async () => ({})
    advance(3 * 60 * MINUTE)
    b.tick()
    await flush()
    expect(b.updater.downloads).toBe(1)
    expect(shown()).toBeNull()

    // but a restart tries again: cancelling is "not now", not "never"
    const restarted = bootWithUpdate('2.0.0')
    restarted.launch()
    await flush()
    expect(restarted.updater.downloads).toBe(2)
  })

  test('a failed cancellation keeps a second update out until the first settles', async () => {
    const b = bootWithUpdate('2.0.0')
    const finish = deferred()
    b.updater.downloadImpl = () => finish.promise
    b.launch()
    await flush()
    b.updater.token!.cancel = () => {
      throw new Error('provider refused cancellation')
    }

    expect(action('cancel')).toBe('cancelled')
    await b.checkNow('interactive')
    expect(b.updater.checks).toBe(1)

    finish.resolve()
    await flush()
    await b.checkNow('interactive')
    expect(b.updater.checks).toBe(2)
  })

  test('a turn already running at launch defers the restart to an idle moment', async () => {
    // a restart auto-resumes the turn that was in flight, so "install as soon as it lands" would
    // routinely kill the work the user came back for
    const b = bootWithUpdate('2.0.0')
    const finish = deferred()
    b.updater.downloadImpl = () => finish.promise
    b.launch()
    await flush()

    reportBusy(true)
    finish.resolve()
    await flush()
    expect(b.updater.quitArgs).toEqual([])
    expect(shown()!.phase).toBe('armed')

    reportBusy(false)
    advance(70 * 1000)
    reportBusy(false)
    await flush()
    expect(b.updater.quitArgs).toEqual([[false, true]])
  })

  test('a restart waiting on idle survives the next check', async () => {
    // the card IS the update state, so a scheduled check that replaced it with a fresh offer would
    // quietly drop a download that was already sitting there waiting for a quiet moment
    const b = bootWithUpdate('2.0.0')
    const finish = deferred()
    b.updater.downloadImpl = () => finish.promise
    b.launch()
    await flush()
    reportBusy(true)
    finish.resolve()
    await flush()
    expect(shown()!.phase).toBe('armed')

    advance(3 * 60 * MINUTE)
    b.tick()
    await flush()
    expect(shown()!.phase).toBe('armed')

    reportBusy(false)
    advance(70 * 1000)
    reportBusy(false)
    await flush()
    expect(b.updater.quitArgs).toEqual([[false, true]])
  })

  test('a skip persisted by an older build no longer blocks the install', async () => {
    fs.writeFileSync(stateFile(), JSON.stringify({ skippedVersion: '2.0.0' }))
    const b = bootWithUpdate('2.0.0')
    reportSettledIdle()
    b.launch()
    await flush()
    expect(b.updater.downloads).toBe(1)
    expect(b.updater.quitArgs).toEqual([[false, true]])
  })

  test('legacy state keys disappear from disk at the next state write', async () => {
    // skippedVersion and the retired update-schedule preference must not ride along forever
    fs.writeFileSync(
      stateFile(),
      JSON.stringify({
        skippedVersion: '2.0.0',
        updateSchedule: 'daily',
        lastScheduledCheckAt: now,
      }),
    )
    const b = bootWithUpdate('2.0.0')
    advance(3 * 60 * MINUTE)
    b.tick()
    await flush()
    expect(JSON.parse(fs.readFileSync(stateFile(), 'utf8'))).toEqual({
      promptedVersion: '2.0.0',
    })
  })

  test('a bundle that cannot self-update is offered instead of downloaded', async () => {
    // downloading 150MB that this bundle can never apply is pure waste; the card explains it when
    // they press Install
    stubProcess(
      'execPath',
      '/Volumes/Freebuff 2.0.0/Freebuff.app/Contents/MacOS/Freebuff',
    )
    const b = bootWithUpdate('2.0.0')
    b.launch()
    await flush()
    expect(b.updater.downloads).toBe(0)
    expect(dialogs).toEqual([])
    expect(shown()!.phase).toBe('translocated')
  })

  test('a renderer that mounts mid-download still finds the Cancel button', async () => {
    // the card is pushed once, and the window is very often still loading when the launch check
    // lands — asking for the current state is the only thing that makes cancel reachable
    const b = bootWithUpdate('2.0.0')
    b.updater.downloadImpl = () => deferred().promise
    b.launch()
    await flush()
    expect(handlers.get('update:pending')!(null)).toEqual({
      phase: 'downloading',
      fraction: 0,
      version: '2.0.0',
      currentVersion: '1.0.0',
    })
  })

  test('a failed download leaves the update on the table', async () => {
    const b = bootWithUpdate('2.0.0')
    b.updater.downloadImpl = async () => {
      throw new Error('net::ERR_CONNECTION_RESET')
    }
    b.launch()
    await flush()

    expect(dialogs).toEqual([])
    expect(shown()).toEqual({
      phase: 'failed',
      version: '2.0.0',
      currentVersion: '1.0.0',
    })
  })

  test('a straggling progress event cannot redraw a download that already failed', async () => {
    // A late provider frame must not put the compact progress card back over the
    // recovery sheet after the failed download has already settled.
    const b = bootWithUpdate('2.0.0')
    b.updater.downloadImpl = async () => {
      throw new Error('net::ERR_CONNECTION_RESET')
    }

    b.launch()
    await flush()
    expect(shown()!.phase).toBe('failed')

    b.updater.emit('download-progress', { percent: 80 })
    expect(shown()!.phase).toBe('failed')
    expect(progress.at(-1)).toBe(-1) // and the taskbar stays cleared
  })
})

describe('the mid-session card', () => {
  beforeEach(pinInstalledMac)

  /** the app has been open a while and a release lands */
  async function midSession(version = '2.0.0'): Promise<Booted> {
    const b = bootWithUpdate(version)
    advance(3 * 60 * MINUTE)
    b.tick()
    await flush()
    return b
  }

  test('a release found mid-session asks instead of restarting the app', async () => {
    // the launch rule cannot apply here: the user is in the middle of something, and an unannounced
    // restart would take it with them
    const b = await midSession()
    expect(b.updater.downloads).toBe(0)
    expect(dialogs).toEqual([])
    expect(shown()).toEqual({
      phase: 'offered',
      version: '2.0.0',
      currentVersion: '1.0.0',
    })
  })

  test('one card per version, however long the app stays open', async () => {
    // it waits for an answer rather than timing out, so re-pushing it would just stack nags on
    // someone who already decided
    const b = await midSession()
    for (let i = 0; i < 3; i++) {
      advance(3 * 60 * MINUTE)
      b.tick()
      await flush()
    }
    expect(b.updater.checks).toBe(4)
    expect(pushed).toHaveLength(1)

    // a later release is a new decision, so it gets its own card
    b.updater.checkImpl = () => ({
      isUpdateAvailable: true,
      updateInfo: { version: '2.1.0' },
    })
    advance(3 * 60 * MINUTE)
    b.tick()
    await flush()
    expect(shown()!.version).toBe('2.1.0')
  })

  test('a version already carded is still installed at the next launch', async () => {
    // the card is one-shot; the launch path is what keeps people current after they ignore it
    await midSession()
    const restarted = bootWithUpdate('2.0.0')
    reportSettledIdle()
    restarted.launch()
    await flush()
    expect(restarted.updater.quitArgs).toEqual([[false, true]])
  })

  test('with no renderer to show it, nothing is pushed and the next tick retries', async () => {
    // a windowless app (macOS keeps running with every window closed) has nobody to ask, and a
    // dialog nobody is looking at is worse than waiting
    const b = bootWithUpdate('2.0.0', { window: null })
    advance(3 * 60 * MINUTE)
    b.tick()
    await flush()
    expect(dialogs).toEqual([])
    // nothing was recorded as prompted, so the version is still on the table
    expect(fs.existsSync(stateFile())).toBe(false)
  })

  test('"Install" downloads and restarts', async () => {
    const b = await midSession()
    expect(action('install')).toBe('installing')
    await flush()
    expect(b.updater.downloads).toBe(1)
    expect(b.updater.quitArgs).toEqual([[false, true]])
  })

  test('dismissing the card persists nothing, so the next launch installs anyway', async () => {
    const b = await midSession()
    expect(action('later')).toBe('dismissed')
    expect(shown()).toBeNull()

    const restarted = bootWithUpdate('2.0.0')
    reportSettledIdle()
    restarted.launch()
    await flush()
    expect(restarted.updater.quitArgs).toEqual([[false, true]])
  })

  test('an action with no card left does nothing at all', async () => {
    // a stale renderer (or a double click) must not install a version nobody offered
    const b = boot()
    expect(action('install')).toBe('ignored')
    expect(b.updater.downloads).toBe(0)
  })

  test('an action this build does not know is ignored, not treated as a dismissal', async () => {
    // a renderer from a different version is exactly the situation an updater is in the middle of
    await midSession()
    expect(action('some-future-button')).toBe('ignored')
    expect(shown()!.phase).toBe('offered')
  })

  test('a stale sheet action cannot hide a download already in progress', async () => {
    const b = await midSession()
    const finish = deferred()
    b.updater.downloadImpl = () => finish.promise
    expect(action('install')).toBe('installing')
    await Promise.resolve()
    expect(shown()!.phase).toBe('downloading')

    // Escape from the offer sheet can race with its Install click. Once main
    // has moved on, that stale dismissal must not remove the Cancel control.
    expect(action('later')).toBe('ignored')
    expect(shown()!.phase).toBe('downloading')

    action('cancel')
    finish.resolve()
    await flush()
  })

  test('status-only sheets reject install and download actions', async () => {
    const b = boot()
    await b.checkNow('interactive')
    expect(shown()!.phase).toBe('current')
    expect(action('install')).toBe('ignored')
    expect(action('manual-download')).toBe('ignored')
    expect(b.updater.downloads).toBe(0)
    expect(opened).toEqual([])
  })

  test('a corrupt state file does not suppress every future update', async () => {
    // a half-written file (a crash during save) must degrade to "no state", not to a throw
    // inside the background check
    fs.writeFileSync(stateFile(), '{"promptedVersion":')
    const b = await midSession()
    expect(shown()!.version).toBe('2.0.0')
  })

  test('an unwritable userData directory does not break the card', async () => {
    // best-effort persistence: the worst outcome of a failed write is being asked again
    fs.rmSync(userData, { recursive: true, force: true })
    const b = await midSession()
    expect(shown()!.version).toBe('2.0.0')
  })
})

describe('install when idle', () => {
  beforeEach(pinInstalledMac)

  /** an offered card, the state every one of these starts from */
  async function offered(): Promise<Booted> {
    const b = bootWithUpdate('2.0.0')
    advance(3 * 60 * MINUTE)
    b.tick()
    await flush()
    return b
  }

  test('it downloads straight away and installs once the app is quiet', async () => {
    // downloading up front is what makes the eventual restart quick; waiting for idle to START a
    // 150MB download would put the restart minutes past the quiet moment it was promised for
    const b = await offered()
    expect(action('install-when-idle')).toBe('armed')
    await flush()
    expect(b.updater.downloads).toBe(1)
    expect(shown()!.phase).toBe('armed')
    expect(b.updater.quitArgs).toEqual([])

    reportBusy(false)
    advance(30 * 1000)
    reportBusy(false)
    await flush()
    expect(b.updater.quitArgs).toEqual([])

    advance(40 * 1000)
    reportBusy(false)
    await flush()
    expect(b.updater.quitArgs).toEqual([[false, true]])
    // already downloaded: the quiet moment must not be spent downloading again
    expect(b.updater.downloads).toBe(1)
  })

  test('a running turn is never interrupted', async () => {
    // the promise the button makes; swapping the binary out from under a running agent loses work
    const b = await offered()
    action('install-when-idle')
    await flush()

    for (let i = 0; i < 10; i++) {
      advance(30 * 1000)
      reportBusy(true)
      await flush()
    }
    expect(b.updater.quitArgs).toEqual([])

    // and the idle clock starts from the moment work stops, not from when it was armed
    reportBusy(false)
    advance(30 * 1000)
    reportBusy(false)
    await flush()
    expect(b.updater.quitArgs).toEqual([])

    advance(40 * 1000)
    reportBusy(false)
    await flush()
    expect(b.updater.quitArgs).toEqual([[false, true]])
  })

  test('a renderer that went quiet does not count as idle', async () => {
    // a reloading or crashed renderer reports nothing; "no news" is not "nothing is running"
    const b = await offered()
    action('install-when-idle')
    await flush()
    reportBusy(false)

    advance(10 * MINUTE)
    b.tick()
    await flush()
    expect(b.updater.quitArgs).toEqual([])

    // the stale stretch does not count toward the settle time either
    reportBusy(false)
    await flush()
    expect(b.updater.quitArgs).toEqual([])

    advance(70 * 1000)
    reportBusy(false)
    await flush()
    expect(b.updater.quitArgs).toEqual([[false, true]])
  })

  test('arming while already idle still leaves a beat before the restart', async () => {
    // otherwise "Install when idle" behaves exactly like "Install" for anyone who is not mid-turn,
    // and the app vanishes a second after they chose the gentler option
    const b = await offered()
    reportBusy(false)
    advance(5 * MINUTE)
    reportBusy(false)
    await flush()

    action('install-when-idle')
    await flush()
    advance(30 * 1000)
    reportBusy(false)
    await flush()
    expect(b.updater.quitArgs).toEqual([])

    advance(40 * 1000)
    reportBusy(false)
    await flush()
    expect(b.updater.quitArgs).toEqual([[false, true]])
  })

  test('"Install now" on an armed card does not wait, and does not re-download', async () => {
    const b = await offered()
    action('install-when-idle')
    await flush()
    reportBusy(true)

    expect(action('install')).toBe('installing')
    expect(b.updater.downloads).toBe(1)
    expect(b.updater.quitArgs).toEqual([[false, true]])
  })

  test('cancelling disarms it', async () => {
    const b = await offered()
    action('install-when-idle')
    await flush()
    expect(action('cancel')).toBe('cancelled')
    expect(shown()!.phase).toBe('offered')

    for (let i = 0; i < 10; i++) {
      advance(30 * 1000)
      reportBusy(false)
      await flush()
    }
    expect(b.updater.quitArgs).toEqual([])
  })
})

describe('the explicit check', () => {
  // The menu item opens the same branded renderer sheet as a mid-session offer;
  // main still owns the decisions and persistence behind its buttons.
  beforeEach(pinInstalledMac)

  test('the retired "skip" action is ignored, not treated as a dismissal', async () => {
    const b = bootWithUpdate('2.0.0')
    await b.checkNow('interactive')
    expect(shown()!.phase).toBe('offered')
    expect(action('skip')).toBe('ignored')
    expect(shown()!.phase).toBe('offered')
    expect(fs.existsSync(stateFile())).toBe(false)
  })

  test('"Remind Me Later" persists nothing, so the next check asks again', async () => {
    const b = bootWithUpdate('2.0.0')
    await b.checkNow('interactive')
    expect(action('later')).toBe('dismissed')
    expect(fs.existsSync(stateFile())).toBe(false)

    await b.checkNow('interactive')
    expect(shown()!.phase).toBe('offered')
    expect(b.updater.downloads).toBe(0)
  })

  test('"Install" downloads and restarts', async () => {
    const b = bootWithUpdate('2.0.0')
    await b.checkNow('interactive')
    expect(action('install')).toBe('installing')
    await flush()
    expect(b.updater.downloads).toBe(1)
    expect(b.updater.quitArgs).toEqual([[false, true]])
  })

  test('a failed explicit check can be retried from the sheet', async () => {
    const b = boot()
    b.updater.checkImpl = () => {
      throw new Error('offline')
    }
    await b.checkNow('interactive')
    expect(shown()!.phase).toBe('check-failed')

    b.updater.checkImpl = () => ({ isUpdateAvailable: false })
    expect(action('check-again')).toBe('checking')
    await flush()
    expect(b.updater.checks).toBe(2)
    expect(shown()!.phase).toBe('current')
  })
})

describe('installing', () => {
  beforeEach(pinInstalledMac)

  test('Windows hands the downloaded update to NSIS silently', async () => {
    // The renderer has already shown download/install progress in Freebuff's
    // own card; the stock NSIS window would duplicate it in unrelated chrome.
    stubProcess('platform', 'win32')
    const b = bootWithUpdate()
    reportSettledIdle()
    b.launch()
    await flush()
    expect(b.updater.quitArgs).toEqual([[true, true]])
  })

  test('the updater never downloads or installs behind the user’s back', async () => {
    // autoInstallOnAppQuit would apply an update at the next ordinary quit even after a cancel;
    // autoDownload would take the download out of Freebuff's hands, and with it the Cancel button
    const b = boot()
    expect(b.updater.autoDownload).toBe(false)
    expect(b.updater.autoInstallOnAppQuit).toBe(false)
    expect(b.updater.disableWebInstaller).toBe(true)
    expect(b.updater.autoRunAppAfterInstall).toBe(true)
  })

  test('download progress drives the taskbar and stops when the download does', async () => {
    const b = bootWithUpdate()
    const started = deferred()
    const finish = deferred()
    b.updater.downloadImpl = () => {
      started.resolve()
      return finish.promise
    }

    b.launch()
    await started.promise
    b.updater.emit('download-progress', { percent: 42 })
    // a provider that reports no total sends percent: NaN — Infinity or NaN on the taskbar is a
    // stuck progress bar
    b.updater.emit('download-progress', { percent: NaN })
    b.updater.emit('download-progress', { percent: -20 })
    b.updater.emit('download-progress', { percent: 180 })
    expect(progress).toEqual([0.42, 0, 0, 1])

    finish.resolve()
    await flush()
    expect(progress).toEqual([0.42, 0, 0, 1, -1]) // -1 clears the bar

    // the listener is per-download; leaking one means a later download drives the bar twice
    b.updater.emit('download-progress', { percent: 90 })
    expect(progress).toEqual([0.42, 0, 0, 1, -1])
  })

  test('a failed download offers a manual download and can be retried', async () => {
    const b = bootWithUpdate()
    b.updater.downloadImpl = async () => {
      throw new Error('net::ERR_CONNECTION_RESET')
    }

    b.launch()
    await flush()
    expect(progress.at(-1)).toBe(-1)
    expect(shown()!.phase).toBe('failed')
    expect(dialogs).toEqual([])

    expect(action('manual-download')).toBe('opened-download')
    await flush()
    expect(opened).toEqual([
      'https://freebuff.com/api/desktop/download/mac-arm64',
    ])

    // A separate failure can be retried without restarting the app; the
    // in-flight flag must have been cleared before the recovery sheet appeared.
    const retry = bootWithUpdate()
    retry.updater.downloadImpl = async () => {
      throw new Error('first attempt')
    }
    retry.launch()
    await flush()
    retry.updater.downloadImpl = async () => ({})
    expect(action('install')).toBe('installing')
    await flush()
    expect(retry.updater.quitArgs).toHaveLength(1)
  })

  test('dismissing a failed update opens nothing', async () => {
    const b = bootWithUpdate()
    b.updater.downloadImpl = async () => {
      throw new Error('boom')
    }
    b.launch()
    await flush()
    expect(action('later')).toBe('dismissed')
    expect(opened).toEqual([])
    expect(shown()).toBeNull()
  })

  test('a failed browser handoff restores the recovery sheet', async () => {
    const b = bootWithUpdate()
    b.updater.downloadImpl = async () => {
      throw new Error('download failed')
    }
    b.launch()
    await flush()
    expect(shown()!.phase).toBe('failed')
    openExternalImpl = async () => {
      throw new Error('no default browser')
    }

    expect(action('manual-download')).toBe('opened-download')
    expect(shown()).toBeNull()
    await flush()
    expect(shown()!.phase).toBe('failed')
    expect(opened).toEqual([])
  })

  test('an install failure that arrives as an event still reaches the manual fallback', async () => {
    // on macOS the verified ZIP is handed to Squirrel after downloadUpdate resolves, and that final
    // native step reports failure by emitting 'error' — nothing rejects
    const b = bootWithUpdate()
    reportSettledIdle()
    b.launch()
    await flush()
    expect(dialogs).toEqual([])

    b.updater.emit('error', new Error('ditto: Couldn’t posix_spawn'))
    await flush()
    expect(shown()!.phase).toBe('failed')
    expect(action('manual-download')).toBe('opened-download')
    await flush()
    expect(opened).toEqual([
      'https://freebuff.com/api/desktop/download/mac-arm64',
    ])
  })

  test('a quitAndInstall that throws falls back once, not twice', async () => {
    // the late-error listener is registered before quitAndInstall; if the throw path did not remove
    // it, a later 'error' would open a second dialog on top of the first
    const b = bootWithUpdate()
    b.updater.quitImpl = () => {
      throw new Error('EACCES')
    }
    reportSettledIdle()
    b.launch()
    await flush()
    expect(pushed.filter((card) => card?.phase === 'failed')).toHaveLength(1)
    expect(b.updater.listenerCount('error')).toBe(0)
  })

  test('a check that fires mid-download is ignored', async () => {
    // a tick during a long download would otherwise start a second download over the first
    const b = bootWithUpdate()
    const started = deferred()
    const finish = deferred()
    b.updater.downloadImpl = () => {
      started.resolve()
      return finish.promise
    }

    b.launch()
    await started.promise
    advance(3 * 60 * MINUTE)
    b.tick()
    await b.checkNow('interactive')
    expect(b.updater.checks).toBe(1)
    expect(b.updater.downloads).toBe(1)

    finish.resolve()
    await flush()
  })

  test('a window that closed mid-install does not break the install', async () => {
    // setProgressBar on a destroyed BrowserWindow throws; the install itself must survive it
    const b = bootWithUpdate('2.0.0', {
      window: {
        setProgressBar: () => {
          throw new Error('Object has been destroyed')
        },
      },
    })
    reportSettledIdle()
    b.launch()
    await flush()
    expect(b.updater.quitArgs).toEqual([[false, true]])
  })
})

describe('the manual download link', () => {
  // the wrong link here hands a user a binary that will not run on their machine, and it is only
  // ever exercised on a path where the automatic update has already failed
  // (darwin/arm64 is not repeated here — the install-failure tests above already assert it)
  const cases: { platform: string; arch: string; url: string }[] = [
    {
      platform: 'darwin',
      arch: 'x64',
      url: 'https://freebuff.com/api/desktop/download/mac-intel',
    },
    {
      platform: 'win32',
      arch: 'x64',
      url: 'https://freebuff.com/api/desktop/download/windows',
    },
    {
      platform: 'win32',
      arch: 'arm64',
      url: 'https://freebuff.com/api/desktop/download/windows-arm64',
    },
    {
      platform: 'linux',
      arch: 'x64',
      url: 'https://freebuff.com/api/desktop/download/linux',
    },
    { platform: 'freebsd', arch: 'x64', url: 'https://freebuff.com/desktop' },
  ]

  for (const { platform, arch, url } of cases) {
    test(`${platform}/${arch} is sent to ${url}`, async () => {
      stubProcess('platform', platform)
      stubProcess('arch', arch)
      stubProcess(
        'execPath',
        '/Applications/Freebuff.app/Contents/MacOS/Freebuff',
      )
      const b = bootWithUpdate()
      b.updater.downloadImpl = async () => {
        throw new Error('nope')
      }
      await b.checkNow('interactive')
      expect(action('install')).toBe('installing')
      await flush()
      expect(shown()!.phase).toBe('failed')
      expect(action('manual-download')).toBe('opened-download')
      await flush()
      expect(opened).toEqual([url])
    })
  }

  test('a baseline Windows build falls back to the compatible installer', async () => {
    stubProcess('platform', 'win32')
    stubProcess('arch', 'x64')
    const b = bootWithUpdate('2.0.0', { releaseFlavor: 'baseline' })
    b.updater.downloadImpl = async () => {
      throw new Error('nope')
    }

    await b.checkNow('interactive')
    expect(action('install')).toBe('installing')
    await flush()
    expect(action('manual-download')).toBe('opened-download')
    await flush()

    expect(opened).toEqual([
      'https://freebuff.com/api/desktop/download/windows-baseline',
    ])
  })
})

describe('macOS translocation', () => {
  beforeEach(() => {
    stubProcess('platform', 'darwin')
    stubProcess('arch', 'arm64')
  })

  // a bundle running from the DMG or from Gatekeeper's read-only quarantine copy cannot replace
  // itself: Squirrel reports success and the next launch is the same old build
  for (const execPath of [
    '/Volumes/Freebuff 1.0.0/Freebuff.app/Contents/MacOS/Freebuff',
    '/private/var/folders/x/AppTranslocation/ABC/d/Freebuff.app/Contents/MacOS/Freebuff',
  ]) {
    test(`a bundle at ${execPath.split('/')[1]} is sent to the installer instead of self-updating`, async () => {
      stubProcess('execPath', execPath)
      const b = bootWithUpdate()
      await b.checkNow('interactive')
      expect(action('install')).toBe('installing')
      await flush()

      expect(b.updater.downloads).toBe(0)
      expect(shown()!.phase).toBe('translocated')
      expect(dialogs).toEqual([])
      expect(action('manual-download')).toBe('opened-download')
      await flush()
      expect(opened).toEqual([
        'https://freebuff.com/api/desktop/download/mac-arm64',
      ])
    })
  }

  test('an installed bundle updates itself', async () => {
    stubProcess(
      'execPath',
      '/Applications/Freebuff.app/Contents/MacOS/Freebuff',
    )
    const b = bootWithUpdate()
    await b.checkNow('interactive')
    expect(action('install')).toBe('installing')
    await flush()
    expect(b.updater.downloads).toBe(1)
    expect(opened).toEqual([])
  })

  test('declining the move leaves the app installable again later', async () => {
    // the guard sets the in-flight flag before it returns; not clearing it would make every later
    // "Install" a no-op for the rest of the session
    stubProcess(
      'execPath',
      '/Volumes/Freebuff/Freebuff.app/Contents/MacOS/Freebuff',
    )
    const b = bootWithUpdate()
    await b.checkNow('interactive')
    expect(action('install')).toBe('installing')
    await flush()
    expect(shown()!.phase).toBe('translocated')
    expect(action('later')).toBe('dismissed')
    expect(opened).toEqual([])

    stubProcess(
      'execPath',
      '/Applications/Freebuff.app/Contents/MacOS/Freebuff',
    )
    await b.checkNow('interactive')
    expect(action('install')).toBe('installing')
    await flush()
    expect(b.updater.downloads).toBe(1)
  })
})

/**
 * main.cjs owns the busy signal and hands it in; the updater only listens for the IPC that
 * feeds it. If that wiring ever came apart there would be no error anywhere — the updater
 * would keep working off its own private signal, and the quit guard would see silence
 * forever, which reads exactly like "nothing is running" and abandons the turn.
 */
describe('the busy signal main.cjs shares with the quit guard', () => {
  const signal = () =>
    (
      require('./busy.cjs') as {
        createBusySignal: () => { busy(now: number): boolean; idle(now: number): boolean }
      }
    ).createBusySignal()

  test('the renderer heartbeat lands in the injected signal, not a private one', () => {
    const busy = signal()
    boot({ busy })
    reportBusy(true)
    expect(busy.busy(Date.now())).toBe(true)
    reportBusy(false)
    expect(busy.busy(Date.now())).toBe(false)
    expect(busy.idle(Date.now())).toBe(true)
  })

  test('the injected signal is the one the idle install reads', () => {
    // both consumers off one state: a busy app must not be restarted under it
    pinInstalledMac()
    const busy = signal()
    const b = bootWithUpdate('2.0.0', { busy })
    return b.checkNow('session').then(async () => {
      expect(action('install-when-idle')).toBe('armed')
      await flush()

      reportBusy(true)
      advance(MINUTE + 1)
      b.tick()
      expect(shown()!.phase).toBe('armed')

      reportSettledIdle()
      b.tick()
      await flush()
      expect(shown()!.phase).toBe('installing')
    })
  })
})

describe('release notes', () => {
  beforeEach(pinInstalledMac)

  /** a release lands mid-session carrying whatever electron-updater parsed out of latest.yml */
  async function offeredWith(releaseNotes: unknown): Promise<Booted> {
    const b = boot()
    b.updater.checkImpl = () => ({
      isUpdateAvailable: true,
      updateInfo: { version: '2.0.0', releaseNotes },
    })
    advance(3 * 60 * MINUTE)
    b.tick()
    await flush()
    return b
  }

  test('the offer carries the notes latest.yml came with', async () => {
    // the sheet is where a user decides whether to restart, so what changed has to be on it
    await offeredWith('- Fixed the thing.\n- Added the other thing.')
    expect(shown()).toEqual({
      phase: 'offered',
      version: '2.0.0',
      currentVersion: '1.0.0',
      notes: '- Fixed the thing.\n- Added the other thing.',
    })
  })

  test('the notes follow the version through every later card', async () => {
    // a failed install re-offers the same version; its notes must not vanish on the way
    const b = await offeredWith('- One.')
    b.updater.downloadImpl = async () => {
      throw new Error('net::ERR_CONNECTION_RESET')
    }
    expect(action('install')).toBe('installing')
    await flush()
    expect(shown()).toMatchObject({ phase: 'failed', version: '2.0.0', notes: '- One.' })
  })

  test('the manual check offers them too', async () => {
    const b = boot()
    b.updater.checkImpl = () => ({
      isUpdateAvailable: true,
      updateInfo: { version: '2.0.0', releaseNotes: '- One.' },
    })
    await b.checkNow('interactive')
    expect(shown()).toMatchObject({ phase: 'offered', notes: '- One.' })
  })

  test('the per-version array shape is flattened', async () => {
    await offeredWith([
      { version: '2.0.0', note: '- Two.' },
      { version: '1.9.0', note: '- One.' },
      { version: '1.8.0', note: null },
    ])
    expect(shown()!.notes).toBe('- Two.\n- One.')
  })

  // the renderer keys the list off the key's existence; an empty string would render a box
  for (const junk of [undefined, null, '', '  \n ', 42, { note: 'x' }, []]) {
    test(`no notes, blank notes, or junk (${JSON.stringify(junk)}) means no notes key at all`, async () => {
      await offeredWith(junk)
      expect(shown()!.phase).toBe('offered')
      expect('notes' in shown()!).toBe(false)
    })
  }

  test('a runaway entry is cut off rather than pushed whole', async () => {
    await offeredWith('x'.repeat(10_000))
    expect(shown()!.notes!.length).toBe(4000)
  })
})
