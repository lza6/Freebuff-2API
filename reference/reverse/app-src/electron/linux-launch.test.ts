/**
 * The bug this guards: an AppImage on Ubuntu 24.04 dies in Chromium's bootstrap
 * (`GPU process launch failed: error_code=1002`) because the kernel refuses the
 * only sandbox mechanism an AppImage can use. Detection has to be right in BOTH
 * directions — missing the condition means the app does not start, and
 * over-firing silently drops the renderer sandbox on machines that never needed
 * it, which is a security regression nobody would notice.
 *
 * The other half of the contract is the RELAUNCH, because appending
 * `--no-sandbox` from JS is too late to affect the zygote: the flag has to reach
 * the child's real command line, exactly once, without dropping the user's own
 * arguments and without any chance of an infinite re-exec loop.
 *
 * Every kernel and env read is injected, so the whole Linux matrix runs from
 * macOS/CI without a Linux box.
 */

import { describe, expect, test } from 'bun:test'
import { createRequire } from 'node:module'

const require_ = createRequire(import.meta.url)
const { planRelaunch, detectSandbox, resolveRelaunchExecPath, describeSandboxFailure } = require_(
  './linux-launch.cjs',
) as any

/** existsSync stand-in: only the listed paths are present. */
const onDisk =
  (...present: string[]) =>
  (p: string) =>
    present.includes(p)

/** readFileSync stand-in over a {path: contents} map; anything absent throws
 *  ENOENT exactly as the real /proc read does. */
const procfs = (files: Record<string, string>) => (file: string) => {
  if (file in files) return files[file]
  throw Object.assign(new Error(`ENOENT: ${file}`), { code: 'ENOENT' })
}

const APPARMOR = '/proc/sys/kernel/apparmor_restrict_unprivileged_userns'
const CLONE = '/proc/sys/kernel/unprivileged_userns_clone'
const MAXNS = '/proc/sys/user/max_user_namespaces'

const APPIMAGE_PATH = '/home/u/Freebuff.AppImage'

/** The reported environment: AppImage on a userns-restricting Ubuntu. */
const brokenUbuntu = {
  platform: 'linux' as const,
  argv: ['/tmp/.mount_ab/freebuff'],
  env: { APPIMAGE: APPIMAGE_PATH },
  readFile: procfs({ [APPARMOR]: '1' }),
  // the AppImage is still where it was launched from — the normal case
  exists: onDisk(APPIMAGE_PATH),
  execPath: '/tmp/.mount_ab/freebuff',
}

describe('sandbox detection', () => {
  test('off Linux nothing is inspected', () => {
    for (const platform of ['darwin', 'win32']) {
      expect(detectSandbox({ platform })).toEqual({ usable: true, reason: 'not-linux' })
    }
  })

  test('the reported case: AppImage + apparmor userns restriction', () => {
    const result = detectSandbox(brokenUbuntu)
    expect(result.usable).toBe(false)
    expect(result.reason).toBe('apparmor-userns-restricted')
    // shown to a user, so it must name the knob they can change
    expect(result.detail).toContain('kernel.apparmor_restrict_unprivileged_userns=1')
    expect(result.fix).toContain('sysctl')
  })

  test('the Debian toggle and the mainline cap are each decisive', () => {
    expect(detectSandbox({ ...brokenUbuntu, readFile: procfs({ [CLONE]: '0' }) }).reason).toBe(
      'userns-clone-disabled',
    )
    expect(detectSandbox({ ...brokenUbuntu, readFile: procfs({ [MAXNS]: '0' }) }).reason).toBe(
      'max-user-namespaces-zero',
    )
  })

  test('permissive values are not restrictions', () => {
    // the ordinary healthy Linux box; firing here would drop the sandbox for
    // everyone running a modern kernel
    expect(
      detectSandbox({
        ...brokenUbuntu,
        readFile: procfs({ [APPARMOR]: '0', [CLONE]: '1', [MAXNS]: '31231' }),
      }),
    ).toEqual({ usable: true, reason: 'userns' })
  })

  test('absent knobs mean "allowed", not "disabled"', () => {
    // mainline kernels have no unprivileged_userns_clone at all; treating a
    // failed read as a restriction would unsandbox every non-Debian distro
    expect(detectSandbox({ ...brokenUbuntu, readFile: procfs({}) })).toEqual({
      usable: true,
      reason: 'userns',
    })
  })

  test('outside an AppImage the kernel is never probed', () => {
    // a .deb install has a working setuid helper and a dev run has a human at
    // the keyboard — probing there could only ever disable a working sandbox
    expect(
      detectSandbox({
        ...brokenUbuntu,
        env: {},
        readFile: () => {
          throw new Error('must not probe /proc outside an AppImage')
        },
      }),
    ).toEqual({ usable: true, reason: 'not-appimage' })
  })
})

describe('relaunch decision', () => {
  test('the reported machine re-execs with --no-sandbox appended', () => {
    const plan = planRelaunch({ ...brokenUbuntu, argv: ['/tmp/.mount_ab/freebuff'] })
    expect(plan.relaunchArgs).toEqual(['--no-sandbox'])
    expect(plan.sandboxOff).toBe(false)
  })

  test("the user's own arguments survive the re-exec", () => {
    // argv[0] is the executable and must NOT be forwarded as an argument
    const plan = planRelaunch({
      ...brokenUbuntu,
      argv: ['/tmp/.mount_ab/freebuff', '--inspect=9229', 'file.txt'],
    })
    expect(plan.relaunchArgs).toEqual(['--inspect=9229', 'file.txt', '--no-sandbox'])
  })

  test('the relaunched process does NOT relaunch again', () => {
    // the loop guard: without it the app would re-exec itself forever
    const child = planRelaunch({
      ...brokenUbuntu,
      argv: ['/tmp/.mount_ab/freebuff', '--no-sandbox'],
    })
    expect(child.relaunchArgs).toBeNull()
    expect(child.sandboxOff).toBe(true)
  })

  test('a healthy machine neither relaunches nor reports the sandbox off', () => {
    const plan = planRelaunch({ ...brokenUbuntu, readFile: procfs({ [APPARMOR]: '0' }) })
    expect(plan.relaunchArgs).toBeNull()
    expect(plan.sandboxOff).toBe(false)
    expect(plan.diagnostic).toContain('sandbox=on')
  })

  test('FREEBUFF_FORCE_SANDBOX suppresses the relaunch and says so', () => {
    // the opt-out for users who would rather fix the sysctl than lose isolation
    const plan = planRelaunch({
      ...brokenUbuntu,
      env: { ...brokenUbuntu.env, FREEBUFF_FORCE_SANDBOX: '1' },
    })
    expect(plan.relaunchArgs).toBeNull()
    expect(plan.diagnostic).toContain('forced-on')
  })

  test('off Linux there is no decision and no log noise', () => {
    for (const platform of ['darwin', 'win32']) {
      expect(planRelaunch({ platform, env: {}, argv: [] })).toEqual({
        sandbox: null,
        sandboxOff: false,
        relaunchArgs: null,
        relaunchExecPath: null,
        diagnostic: null,
      })
    }
  })

  test('the diagnostic names the state, the reason and the fix', () => {
    const plan = planRelaunch(brokenUbuntu)
    expect(plan.diagnostic).toContain('sandbox=off-after-relaunch')
    expect(plan.diagnostic).toContain('apparmor-userns-restricted')
    expect(plan.diagnostic).toContain('sudo sysctl')
  })
})

describe('where the re-exec points', () => {
  test('under an AppImage it is $APPIMAGE, never the /tmp mount', () => {
    // process.execPath lives inside the squashfs mount, which unmounts when we
    // exit — exec'ing it would land on a path that no longer exists
    expect(
      resolveRelaunchExecPath({
        env: { APPIMAGE: APPIMAGE_PATH },
        execPath: '/tmp/.mount_ab/freebuff',
        exists: onDisk(APPIMAGE_PATH, '/tmp/.mount_ab/freebuff'),
      }),
    ).toBe(APPIMAGE_PATH)
  })

  test('a moved or deleted AppImage resolves to null, not a doomed fallback', () => {
    // falling back to execPath here would be worse than useless: that path is
    // inside the mount that is about to disappear
    expect(
      resolveRelaunchExecPath({
        env: { APPIMAGE: APPIMAGE_PATH },
        execPath: '/tmp/.mount_ab/freebuff',
        exists: onDisk('/tmp/.mount_ab/freebuff'),
      }),
    ).toBeNull()
  })

  test('outside an AppImage it is execPath, and only if it exists', () => {
    expect(
      resolveRelaunchExecPath({ env: {}, execPath: '/opt/f/freebuff', exists: onDisk('/opt/f/freebuff') }),
    ).toBe('/opt/f/freebuff')
    expect(resolveRelaunchExecPath({ env: {}, execPath: '/opt/f/freebuff', exists: onDisk() })).toBeNull()
    expect(resolveRelaunchExecPath({ env: {}, execPath: '', exists: onDisk() })).toBeNull()
  })

  test('a path that cannot be stat-ed is not exec-ed into', () => {
    expect(
      resolveRelaunchExecPath({
        env: { APPIMAGE: APPIMAGE_PATH },
        exists: () => {
          throw Object.assign(new Error('EACCES'), { code: 'EACCES' })
        },
      }),
    ).toBeNull()
  })

  test('a healthy launch never touches the filesystem', () => {
    // no re-exec is on the table, so there is nothing to resolve
    const plan = planRelaunch({
      ...brokenUbuntu,
      readFile: procfs({ [APPARMOR]: '0' }),
      exists: () => {
        throw new Error('must not stat when no relaunch is planned')
      },
    })
    expect(plan.relaunchExecPath).toBeNull()
    expect(plan.relaunchArgs).toBeNull()
  })
})

describe('when the app cannot re-exec at all', () => {
  const stranded = { ...brokenUbuntu, exists: onDisk() }

  test('the relaunch is still wanted, but has nowhere to go', () => {
    // relaunchArgs stays non-null so the caller can tell "nothing to do" apart
    // from "needed to act and could not" — the two demand different messages
    const plan = planRelaunch(stranded)
    expect(plan.relaunchArgs).toEqual(['--no-sandbox'])
    expect(plan.relaunchExecPath).toBeNull()
  })

  test('the log says so, and names the path that went missing', () => {
    const plan = planRelaunch(stranded)
    expect(plan.diagnostic).toContain('sandbox=relaunch-blocked')
    expect(plan.diagnostic).toContain(`missing-exec-path=${APPIMAGE_PATH}`)
  })

  test('the dialog withholds a retry button it could not honour', () => {
    // clicking a retry that exits into nothing would make the app vanish
    // mid-click — strictly worse than the silent death being fixed
    const copy = describeSandboxFailure({
      sandbox: detectSandbox(stranded),
      sandboxOff: false,
      canRelaunch: false,
    })
    expect(copy.canRetryWithoutSandbox).toBe(false)
    // it must hand over the manual command instead of leaving them stuck
    expect(copy.detail).toContain('--no-sandbox')
    expect(copy.detail).toContain('moved or deleted')
    // and still name the real underlying cause
    expect(copy.detail).toContain('kernel.apparmor_restrict_unprivileged_userns=1')
  })

  test('an omitted canRelaunch cannot silently suppress the retry', () => {
    const copy = describeSandboxFailure({ sandbox: detectSandbox(brokenUbuntu), sandboxOff: false })
    expect(copy.canRetryWithoutSandbox).toBe(true)
  })

  test('already-unsandboxed wins over a missing exec path', () => {
    // both say "no retry", but the already-off copy is the accurate one: the
    // sandbox is not the problem any more, so do not blame a missing file
    const copy = describeSandboxFailure({ sandbox: {}, sandboxOff: true, canRelaunch: false })
    expect(copy.canRetryWithoutSandbox).toBe(false)
    expect(copy.detail).toContain('already running without the Chromium sandbox')
    expect(copy.detail).not.toContain('moved or deleted')
  })
})

describe('failure dialog copy', () => {
  test('offers the retry, and names the cost and the fix, while the sandbox is on', () => {
    const copy = describeSandboxFailure({
      sandbox: detectSandbox(brokenUbuntu),
      sandboxOff: false,
    })
    expect(copy.canRetryWithoutSandbox).toBe(true)
    expect(copy.detail).toContain('kernel.apparmor_restrict_unprivileged_userns=1')
    // the user is told what they give up before they click
    expect(copy.detail).toContain('weakens isolation')
    expect(copy.detail).toContain('sudo sysctl')
  })

  test('never offers to disable a sandbox the preflight found healthy', () => {
    // a dev run or a .deb reports {usable: true, reason: 'not-appimage'} and has
    // no `detail`; without this branch the copy fell back to asserting the sandbox
    // had failed and made "Relaunch Without Sandbox" the DEFAULT button — talking
    // the user into weakening isolation to fix something that is not the sandbox
    const copy = describeSandboxFailure({
      sandbox: { usable: true, reason: 'not-appimage' },
      sandboxOff: false,
    })
    expect(copy.canRetryWithoutSandbox).toBe(false)
    expect(copy.detail).toContain('sandbox is available on this system')
    expect(copy.detail).not.toContain('weakens isolation')
  })

  test('a usable sandbox outranks a missing exec path', () => {
    // both suppress the retry, but blaming a missing file would be wrong here
    const copy = describeSandboxFailure({
      sandbox: { usable: true, reason: 'userns' },
      sandboxOff: false,
      canRelaunch: false,
    })
    expect(copy.detail).not.toContain('moved or deleted')
  })

  test('does not re-offer a workaround that is already in effect', () => {
    const copy = describeSandboxFailure({ sandbox: { reason: 'userns' }, sandboxOff: true })
    expect(copy.canRetryWithoutSandbox).toBe(false)
    expect(copy.detail).not.toContain('sysctl')
  })
})
