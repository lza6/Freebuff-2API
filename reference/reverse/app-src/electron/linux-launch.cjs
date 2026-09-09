/**
 * Linux launch preflight — Chromium sandbox availability.
 *
 * The AppImage had been dying on modern Ubuntu with:
 *
 *     GPU process launch failed: error_code=1002
 *     GPU process isn't usable. Goodbye.
 *     Network service crashed, restarting service.
 *     Failed to send GetTerminationStatus message to zygote
 *
 * That trio is NOT a graphics fault — a bad driver kills only the GPU process.
 * Every child failing to launch means the ZYGOTE cannot create its sandbox, and
 * the browser process then aborts. Chromium has two ways to sandbox on Linux:
 * the setuid helper (`chrome-sandbox`, mode 4755 owned by root) that a .deb/.rpm
 * install gets, or unprivileged user namespaces, the fallback everything else
 * relies on. An AppImage can have neither by construction: it is a squashfs
 * mounted `nosuid`, so the setuid bit on the bundled helper is inert no matter
 * how the file is packed. That leaves user namespaces as the only path — and
 * Ubuntu 23.10+ ships `kernel.apparmor_restrict_unprivileged_userns=1`, which
 * strips the capabilities Chromium needs inside the new namespace for any
 * unconfined binary. An AppImage is unconfined.
 *
 * WHY WE RE-EXEC INSTEAD OF SETTING A SWITCH. The obvious fix —
 * `app.commandLine.appendSwitch('no-sandbox')` at the top of main.cjs — is a
 * NO-OP for this. Chromium forks the zygote and fixes its sandbox configuration
 * during early process startup, before Electron ever evaluates the main script,
 * so a switch appended from JS arrives after the decision it is trying to
 * change. The switch has to be on the process's REAL command line, which leaves
 * exactly one lever we control from inside the app: detect the condition and
 * re-exec ourselves with the flag (see planRelaunch + main.cjs).
 *
 * Why the AppImage gets here at all: electron-builder bakes `--no-sandbox` into
 * the AppImage's bundled .desktop entry (`AppRun --no-sandbox %U`), so a launch
 * from the desktop menu is already unsandboxed. Running `./Freebuff.AppImage`
 * directly — how the reporter runs it, and how most people try an AppImage the
 * first time — goes through no .desktop file and gets no such flag.
 *
 * The same "must be there at process start" constraint applies to Wayland, which
 * is why no ozone switch is set here either. `--ozone-platform-hint=auto` lives
 * in `appImage.executableArgs` in package.json, next to the `--no-sandbox` that
 * electron-builder would otherwise supply on its own — listing that entry
 * REPLACES electron-builder's default, so `--no-sandbox` is repeated there to
 * preserve it rather than as a second opinion about sandboxing. It sits under
 * `appImage`, not `linux`, so it cannot leak into a future .deb — the one format
 * that HAS a working setuid helper and must keep its sandbox. Those args only
 * reach a desktop-menu launch; a direct `./Freebuff.AppImage` still runs under
 * XWayland, and nothing inside this process can change that.
 *
 * SECURITY TRADE-OFF, deliberate and narrow. `--no-sandbox` is a real downgrade:
 * this app hosts preview <webview>s that load arbitrary project web content, and
 * the sandbox is what contains a renderer compromise. Two rules keep it from
 * spreading further than the bug requires:
 *
 *   - Only under an AppImage ($APPIMAGE). Any other layout — a dev `electron .`,
 *     or a future .deb — has a working setuid helper or a developer at the
 *     keyboard, so probing sysctls there could only ever disable a sandbox that
 *     was fine.
 *   - Only after a kernel knob positively says no. Absence of a knob means
 *     "allowed"; we never infer breakage from a failed read.
 *
 * Users who would rather fix the sysctl and keep the sandbox set
 * FREEBUFF_FORCE_SANDBOX=1; the diagnostic we log names the knob to change.
 *
 * Everything here is pure and injectable, so the whole matrix is testable from
 * macOS/CI without a Linux box.
 */

const fs = require('node:fs')

/**
 * Kernel knobs that each independently prevent the namespace sandbox, newest
 * first. A knob that is ABSENT is not a blocker — `unprivileged_userns_clone`
 * does not exist on mainline kernels, and reading its absence as a restriction
 * would disable the sandbox on every non-Debian distro. Only the exact `blocked`
 * value counts.
 */
const USERNS_BLOCKERS = [
  {
    // Ubuntu 23.10+. The reported cause: userns creation succeeds, but an
    // unconfined binary gets none of the capabilities Chromium needs inside it.
    file: '/proc/sys/kernel/apparmor_restrict_unprivileged_userns',
    blocked: '1',
    reason: 'apparmor-userns-restricted',
    detail:
      'This system restricts unprivileged user namespaces for unpackaged apps ' +
      '(kernel.apparmor_restrict_unprivileged_userns=1), which the Chromium sandbox requires.',
    fix: 'sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0',
  },
  {
    // Debian/Ubuntu's older toggle, absent on mainline kernels.
    file: '/proc/sys/kernel/unprivileged_userns_clone',
    blocked: '0',
    reason: 'userns-clone-disabled',
    detail:
      'This system disables unprivileged user namespaces ' +
      '(kernel.unprivileged_userns_clone=0), which the Chromium sandbox requires.',
    fix: 'sudo sysctl -w kernel.unprivileged_userns_clone=1',
  },
  {
    // Mainline cap on how many user namespaces a user may hold.
    file: '/proc/sys/user/max_user_namespaces',
    blocked: '0',
    reason: 'max-user-namespaces-zero',
    detail:
      'This system allows no user namespaces (user.max_user_namespaces=0), ' +
      'which the Chromium sandbox requires.',
    fix: 'sudo sysctl -w user.max_user_namespaces=15000',
  },
]

const NO_SANDBOX = '--no-sandbox'

/** A sysctl's trimmed value, or null when it does not exist / cannot be read. */
function readSysctl(file, readFile) {
  try {
    return String(readFile(file, 'utf8')).trim()
  } catch {
    return null
  }
}

/**
 * Decide whether Chromium can sandbox on this machine.
 *
 * `reason` is a stable slug for logs; `detail` and `fix` are user-facing and
 * name the knob to change. See the module header for why this only ever
 * inspects the kernel under an AppImage.
 */
function detectSandbox(opts = {}) {
  const { platform = process.platform, env = process.env, readFile = fs.readFileSync } = opts
  if (platform !== 'linux') return { usable: true, reason: 'not-linux' }
  if (!env.APPIMAGE) return { usable: true, reason: 'not-appimage' }

  for (const blocker of USERNS_BLOCKERS) {
    if (readSysctl(blocker.file, readFile) === blocker.blocked) {
      const { reason, detail, fix } = blocker
      return { usable: false, reason, detail, fix }
    }
  }
  return { usable: true, reason: 'userns' }
}

/**
 * Where a re-exec should point, or null when it cannot safely point anywhere.
 *
 * Under an AppImage `process.execPath` is inside the /tmp squashfs mount, which
 * unmounts the moment we exit — only $APPIMAGE is durable. So if $APPIMAGE is
 * gone (the user moved or deleted it, or an in-place update was interrupted)
 * there is NO path that will still exist when the relauncher runs: relaunching
 * would exit into nothing and the app would simply disappear, which is the exact
 * silent death this module exists to prevent. Return null so the caller can say
 * so out loud instead of vanishing.
 *
 * Resolve this at the moment of use, not once at startup — the file can be
 * moved or replaced by an update while the app is running.
 */
function resolveRelaunchExecPath(opts = {}) {
  const { env = process.env, execPath = process.execPath, exists = fs.existsSync } = opts
  const target = env.APPIMAGE || execPath
  if (!target) return null
  try {
    return exists(target) ? target : null
  } catch {
    // A path we cannot even stat is not one we should exec into.
    return null
  }
}

/**
 * What this process should do about the sandbox, decided once at startup.
 *
 * Returns `{ sandbox, sandboxOff, relaunchArgs, relaunchExecPath, diagnostic }`:
 *   - `sandboxOff` — this process is ALREADY running without the sandbox,
 *     because `--no-sandbox` is on our real command line.
 *   - `relaunchArgs` — non-null means re-exec with these args (see main.cjs).
 *     Null whenever the sandbox works, is already off, or the user pinned it on.
 *   - `relaunchExecPath` — what to exec. NULL WITH A NON-NULL `relaunchArgs`
 *     means the re-exec is wanted but impossible; the caller must report that
 *     rather than relaunch into nothing.
 *   - `diagnostic` — one line for the log, or null off Linux.
 *
 * The relaunch is loop-safe by construction: the child's argv carries
 * `--no-sandbox`, so `sandboxOff` is true there and it never relaunches again.
 */
function planRelaunch(opts = {}) {
  const { platform = process.platform, env = process.env, argv = process.argv } = opts
  if (platform !== 'linux') {
    return {
      sandbox: null,
      sandboxOff: false,
      relaunchArgs: null,
      relaunchExecPath: null,
      diagnostic: null,
    }
  }

  const sandboxOff = argv.includes(NO_SANDBOX) || argv.some((a) => a.startsWith(`${NO_SANDBOX}=`))
  const sandbox = detectSandbox({ ...opts, platform, env })
  const forced = env.FREEBUFF_FORCE_SANDBOX === '1'
  const relaunch = !sandbox.usable && !sandboxOff && !forced
  // Only touch the filesystem when a re-exec is actually on the table.
  const relaunchExecPath = relaunch ? resolveRelaunchExecPath({ ...opts, env }) : null

  const state = sandboxOff
    ? 'off'
    : relaunch
      ? relaunchExecPath
        ? 'off-after-relaunch'
        : 'relaunch-blocked'
      : 'on'
  const diagnostic =
    `[linux-launch] sandbox=${state} (${sandbox.reason})` +
    `${forced && !sandbox.usable ? ' forced-on by FREEBUFF_FORCE_SANDBOX' : ''}` +
    `${relaunch && !relaunchExecPath ? ` missing-exec-path=${env.APPIMAGE ?? '(none)'}` : ''}` +
    `${sandbox.fix ? ` fix="${sandbox.fix}"` : ''}`

  return {
    sandbox,
    sandboxOff,
    // Append rather than replace: the user's own flags must survive the re-exec.
    relaunchArgs: relaunch ? [...argv.slice(1), NO_SANDBOX] : null,
    relaunchExecPath,
    diagnostic,
  }
}

/**
 * User-facing text for a launch that died in Chromium's bootstrap. Kept next to
 * the detection so the dialog can never describe a check we no longer make.
 */
function describeSandboxFailure(opts = {}) {
  // `canRelaunch` defaults true so an omitted flag cannot silently hide the
  // retry; callers that know the exec path is gone must say so explicitly.
  const { sandbox, sandboxOff, canRelaunch = true } = opts
  if (sandboxOff) {
    // The sandbox is already off and a process STILL failed to launch, so the
    // cause is elsewhere (graphics stack, missing system libraries). Do not send
    // them down the sandbox path a second time.
    return {
      message: 'Freebuff could not start a required subprocess.',
      detail:
        'Freebuff is already running without the Chromium sandbox, so this is likely a graphics ' +
        'or system library problem rather than the sandbox.\n\n' +
        'Launch from a terminal to see the underlying error:\n' +
        '  ./Freebuff.AppImage --disable-gpu',
      canRetryWithoutSandbox: false,
    }
  }
  if (sandbox?.usable) {
    // The preflight positively concluded this machine CAN sandbox — a dev run, a
    // .deb with a working setuid helper, or a kernel with no restriction. Whatever
    // just failed, the sandbox is not the diagnosis, and offering to disable it
    // (as the default button, no less) would talk the user into weakening
    // isolation to fix something else entirely. That is the over-firing direction
    // this module is most careful about; it must not sneak back in via the dialog.
    return {
      message: 'Freebuff could not start a required subprocess.',
      detail:
        'A required Chromium subprocess failed to launch. The Chromium sandbox is available on ' +
        'this system, so the cause is more likely the graphics stack or a missing system library.\n\n' +
        'Launch from a terminal to see the underlying error:\n' +
        '  ./Freebuff.AppImage --disable-gpu',
      canRetryWithoutSandbox: false,
    }
  }
  if (!canRelaunch) {
    // We would restart without the sandbox, but the file to restart FROM is
    // gone. Offering the button here would exit into nothing and the app would
    // disappear mid-click, so explain and hand over the manual command instead.
    return {
      message: 'Freebuff could not start a required subprocess.',
      detail:
        `${sandbox?.detail ?? 'The Chromium sandbox could not start on this system.'}\n\n` +
        'Freebuff would normally restart itself without the sandbox, but the application file ' +
        'it needs to restart from is missing — it may have been moved or deleted, or an update ' +
        'may have been interrupted.\n\n' +
        'Re-download Freebuff, or start it yourself with:\n' +
        '  ./Freebuff.AppImage --no-sandbox' +
        (sandbox?.fix ? `\n\nTo fix this system-wide and keep the sandbox:\n  ${sandbox.fix}` : ''),
      canRetryWithoutSandbox: false,
    }
  }
  return {
    message: 'Freebuff could not start a required subprocess.',
    detail:
      `${sandbox?.detail ?? 'The Chromium sandbox could not start on this system.'}\n\n` +
      'Freebuff can restart with the sandbox disabled. That weakens isolation for web content ' +
      'shown in preview panes, so only do it on a machine you trust.' +
      (sandbox?.fix ? `\n\nTo fix this system-wide and keep the sandbox:\n  ${sandbox.fix}` : ''),
    canRetryWithoutSandbox: true,
  }
}

module.exports = {
  planRelaunch,
  detectSandbox,
  resolveRelaunchExecPath,
  describeSandboxFailure,
  USERNS_BLOCKERS,
  NO_SANDBOX,
}
