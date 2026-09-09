/**
 * PATH repair for a Finder-launched app. Two classes of failure are pinned here
 * because both have already shipped:
 *
 *  - the probe not running (no $SHELL under launchd, a win32 build) or coming
 *    back garbage (rc banners, `set -x` echoes, a fish $PATH flattened by
 *    spaces) — the app boots with `codex: command not found`;
 *  - the probe running and making things WORSE. The merge is documented as
 *    purely additive, so every test below that touches ordering exists to keep a
 *    misbehaving rc file from shadowing or dropping a binary the parent process
 *    could already resolve.
 *
 * Everything is injectable (exec/userInfo/access/platform/home), so no test here
 * reads the ambient environment — a machine without fish, or a developer whose
 * $SHELL is unset, must not change a single result.
 */

import { afterEach, describe, expect, test } from 'bun:test'
import { chmodSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const require_ = createRequire(import.meta.url)
const { fixPath, resolvePath, mergePath, probeLoginShellPath, findOnPath } = require_(
  './shell-path.cjs',
) as {
  fixPath: (opts?: Record<string, unknown>) => string
  resolvePath: (opts?: Record<string, unknown>) => string
  mergePath: (current: string, extra: string[]) => string
  probeLoginShellPath: (opts?: Record<string, unknown>) => string | null
  findOnPath: (bin: string, pathStr: string, access?: (p: string, mode: number) => void) => string | null
}

interface ExecCall {
  file: string
  args: string[]
  opts: { timeout?: number; encoding?: string; stdio?: unknown }
}

/** Records what the login-shell probe actually ran, and replies with `out`. */
function recordingExec(out: string | (() => string)) {
  const calls: ExecCall[] = []
  const exec = (file: string, args: string[], opts: ExecCall['opts']) => {
    calls.push({ file, args, opts })
    return typeof out === 'function' ? out() : out
  }
  return { exec, calls }
}

const MARKER = '__FREEBUFF_PATH__'
/** printf output the shell would produce: marker immediately followed by the value. */
const emits = (value: string, banner = '') => `${banner}${MARKER}${value}`

const dirs = (p: string) => p.split(':')

describe('mergePath', () => {
  test('appends discovered dirs and keeps every entry the parent already had', () => {
    expect(mergePath('/usr/bin:/bin', ['/opt/homebrew/bin'])).toBe('/usr/bin:/bin:/opt/homebrew/bin')
  })

  test('a rediscovered dir keeps its original precedence instead of moving to the end', () => {
    // the union is documented as additive: /usr/bin must still win over /opt/mise/bin
    // for a binary that exists in both, no matter what order the shell reported them
    expect(mergePath('/usr/bin:/sbin', ['/opt/mise/bin', '/usr/bin'])).toBe(
      '/usr/bin:/sbin:/opt/mise/bin',
    )
  })

  test('drops duplicates already present inside the inherited PATH', () => {
    expect(mergePath('/usr/bin:/bin:/usr/bin', [])).toBe('/usr/bin:/bin')
  })

  test('drops empty segments so a trailing or doubled separator cannot survive', () => {
    // an empty PATH entry means "current directory" to execvp — never pass one on
    expect(mergePath('/usr/bin::/bin:', [])).toBe('/usr/bin:/bin')
  })

  test('drops relative entries, including an rc file that printed ~ unexpanded', () => {
    expect(mergePath('/usr/bin', ['~/bin', '.', 'node_modules/.bin', '/opt/homebrew/bin'])).toBe(
      '/usr/bin:/opt/homebrew/bin',
    )
  })

  test('trims surrounding whitespace instead of discarding the entry', () => {
    // the probe's value arrives with a trailing newline and rc files pad around ':' — untrimmed,
    // ' /opt/homebrew/bin' is not absolute and would be dropped as a relative entry, silently
    // losing the exact dir the probe exists to find
    expect(mergePath('/usr/bin', ['  /opt/homebrew/bin  '])).toBe('/usr/bin:/opt/homebrew/bin')
    expect(mergePath('/usr/bin', ['/opt/homebrew/bin\n'])).toBe('/usr/bin:/opt/homebrew/bin')
    // and the trimmed form is what dedupe compares, so the same dir cannot land twice
    expect(mergePath('/usr/bin', ['  /opt/homebrew/bin  ', '/opt/homebrew/bin'])).toBe(
      '/usr/bin:/opt/homebrew/bin',
    )
  })

  test('an empty inherited PATH does not produce a leading empty entry', () => {
    // launchd can hand a GUI app an empty PATH; ':/opt/homebrew/bin' would put cwd first
    expect(mergePath('', ['/opt/homebrew/bin'])).toBe('/opt/homebrew/bin')
  })
})

describe('probeLoginShellPath', () => {
  test('does not run a shell at all on win32', () => {
    const { exec, calls } = recordingExec(emits('/whatever'))
    expect(probeLoginShellPath({ exec, platform: 'win32', shell: '/bin/zsh' })).toBeNull()
    expect(calls).toEqual([])
  })

  test('does not run a shell when none can be resolved', () => {
    const { exec, calls } = recordingExec(emits('/whatever'))
    // explicit empty shell is authoritative — it must not fall back to $SHELL or passwd
    expect(
      probeLoginShellPath({ exec, platform: 'darwin', shell: '', userInfo: () => ({ shell: '/bin/zsh' }) }),
    ).toBeNull()
    expect(calls).toEqual([])
  })

  test('runs the shell interactive+login and under a timeout', () => {
    const { exec, calls } = recordingExec(emits('/usr/bin'))
    probeLoginShellPath({ exec, platform: 'darwin', shell: '/bin/zsh' })

    expect(calls).toHaveLength(1)
    expect(calls[0].file).toBe('/bin/zsh')
    // -i as well as -l: version managers land in .zshrc as often as .zprofile
    expect(calls[0].args[0]).toBe('-ilc')
    // an rc file that blocks on a prompt must not hang app startup forever
    expect(calls[0].opts.timeout).toBeGreaterThan(0)
    expect(calls[0].opts.timeout).toBeLessThanOrEqual(5000)
    // stdin closed: an interactive login shell that reads from it (`read -p`, a stale `stty`)
    // would otherwise sit there until the timeout on every single launch
    expect((calls[0].opts.stdio as string[])[0]).toBe('ignore')
  })

  test('ignores an rc banner printed ahead of the value', () => {
    const { exec } = recordingExec(emits('/opt/homebrew/bin:/usr/bin', 'Welcome to zsh!\nnvm loaded\n'))
    expect(probeLoginShellPath({ exec, platform: 'darwin', shell: '/bin/zsh' })).toBe(
      '/opt/homebrew/bin:/usr/bin',
    )
  })

  test('takes the LAST marker when the shell echoes the command it ran', () => {
    // `set -x` in an rc file traces the printf, so the marker appears in the trace too;
    // reading the first one would return the literal script text as a PATH
    const traced = `+ printf '%s%s' ${MARKER} "$PATH"\n${emits('/opt/homebrew/bin')}`
    const { exec } = recordingExec(traced)
    expect(probeLoginShellPath({ exec, platform: 'darwin', shell: '/bin/zsh' })).toBe(
      '/opt/homebrew/bin',
    )
  })

  test('returns null when the output never contains the marker', () => {
    const { exec } = recordingExec('command not found: printf\n')
    expect(probeLoginShellPath({ exec, platform: 'darwin', shell: '/bin/zsh' })).toBeNull()
  })

  test('returns null rather than an empty string when the shell reports no PATH', () => {
    // '' would be merged as a real (empty = cwd) entry by a caller that only null-checks
    const { exec } = recordingExec(emits('   \n'))
    expect(probeLoginShellPath({ exec, platform: 'darwin', shell: '/bin/zsh' })).toBeNull()
  })

  test('swallows a shell that throws — a broken or slow rc must not stop boot', () => {
    const { exec } = recordingExec(() => {
      throw Object.assign(new Error('ETIMEDOUT'), { killed: true })
    })
    expect(probeLoginShellPath({ exec, platform: 'darwin', shell: '/bin/zsh' })).toBeNull()
  })

  test('asks fish to join its list $PATH, since "$PATH" there is space-separated', () => {
    const { exec, calls } = recordingExec(emits('/usr/bin'))
    probeLoginShellPath({ exec, platform: 'darwin', shell: '/opt/homebrew/bin/fish' })

    expect(calls[0].args[1]).toContain('string join : $PATH')
    // the posix form would collapse N dirs into one unusable space-joined entry
    expect(calls[0].args[1]).not.toContain('"$PATH"')
  })

  test('matches fish by basename only, so /bin/zsh keeps the posix expression', () => {
    const { exec, calls } = recordingExec(emits('/usr/bin'))
    probeLoginShellPath({ exec, platform: 'darwin', shell: '/bin/zsh' })
    // a shell merely containing "fish" in its path is not fish
    probeLoginShellPath({ exec, platform: 'darwin', shell: '/home/fish/bin/bash' })

    for (const call of calls) {
      expect(call.args[1]).toContain('"$PATH"')
      expect(call.args[1]).not.toContain('string join')
    }
  })

  test('uses $SHELL when no shell was passed explicitly', () => {
    const { exec, calls } = recordingExec(emits('/usr/bin'))
    probeLoginShellPath({ exec, platform: 'darwin', shellEnv: '/bin/bash' })
    expect(calls[0].file).toBe('/bin/bash')
  })

  test('falls back to the passwd shell when $SHELL is absent, as under launchd', () => {
    // this is the packaged case the whole module exists for: a Finder launch inherits
    // no $SHELL, and skipping the probe there loses every nvm/fnm-managed binary
    const { exec, calls } = recordingExec(emits('/usr/bin'))
    probeLoginShellPath({
      exec,
      platform: 'darwin',
      shellEnv: undefined,
      userInfo: () => ({ shell: '/opt/homebrew/bin/fish' }),
    })
    expect(calls[0].file).toBe('/opt/homebrew/bin/fish')
    expect(calls[0].args[1]).toContain('string join : $PATH')
  })

  test('survives a uid with no passwd entry', () => {
    const { exec, calls } = recordingExec(emits('/usr/bin'))
    expect(
      probeLoginShellPath({
        exec,
        platform: 'darwin',
        shellEnv: undefined,
        userInfo: () => {
          throw new Error('uid not found')
        },
      }),
    ).toBeNull()
    expect(calls).toEqual([])
  })
})

describe('findOnPath', () => {
  const tmps: string[] = []
  const mkdir = () => {
    const dir = mkdtempSync(join(tmpdir(), 'fb-path-'))
    tmps.push(dir)
    return dir
  }
  afterEach(() => {
    for (const dir of tmps.splice(0)) rmSync(dir, { recursive: true, force: true })
  })

  test('skips a present but non-executable file and keeps scanning', () => {
    // real fs + real accessSync: an X_OK check written as F_OK would report a
    // downloaded-but-unchmodded `bun` as reachable and the breadcrumb would lie
    const noExec = mkdir()
    const exec = mkdir()
    writeFileSync(join(noExec, 'bun'), '')
    chmodSync(join(noExec, 'bun'), 0o644)
    writeFileSync(join(exec, 'bun'), '')
    chmodSync(join(exec, 'bun'), 0o755)

    expect(findOnPath('bun', `${noExec}:${exec}`)).toBe(join(exec, 'bun'))
  })

  test('returns the earliest match, which is the one that would actually run', () => {
    const first = mkdir()
    const second = mkdir()
    for (const dir of [first, second]) {
      writeFileSync(join(dir, 'codex'), '')
      chmodSync(join(dir, 'codex'), 0o755)
    }
    expect(findOnPath('codex', `${first}:${second}`)).toBe(join(first, 'codex'))
  })

  test('tolerates empty segments and directories that do not exist', () => {
    const gone = join(mkdir(), 'nope')
    expect(findOnPath('bun', `:${gone}:`)).toBeNull()
  })
})

describe('resolvePath', () => {
  const HOME = '/Users/tester'

  test('leaves win32 alone — its GUI apps already inherit the full user PATH', () => {
    const { exec, calls } = recordingExec(emits('/anything'))
    const current = 'C:\\Windows;C:\\Windows\\System32'
    expect(resolvePath({ current, home: HOME, platform: 'win32', exec, shell: '/bin/zsh' })).toBe(
      current,
    )
    // and no posix fallback dirs get spliced into a Windows PATH
    expect(calls).toEqual([])
  })

  test('appends probe dirs after the inherited PATH and before the static fallbacks', () => {
    const { exec } = recordingExec(emits('/opt/nvm/versions/node/v20/bin'))
    const out = dirs(
      resolvePath({ current: '/usr/bin', home: HOME, platform: 'darwin', exec, shell: '/bin/zsh' }),
    )

    expect(out[0]).toBe('/usr/bin')
    expect(out[1]).toBe('/opt/nvm/versions/node/v20/bin')
    expect(out.indexOf('/opt/nvm/versions/node/v20/bin')).toBeLessThan(out.indexOf(`${HOME}/.bun/bin`))
  })

  test('a hostile probe can never drop or outrank what the parent already had', () => {
    // the whole reason the merge is a union: an rc file that exports a stripped PATH
    // must not be able to make a working environment worse
    const { exec } = recordingExec(emits('/usr/bin'))
    const out = dirs(
      resolvePath({
        current: '/my/special/bin:/usr/bin',
        home: HOME,
        platform: 'darwin',
        exec,
        shell: '/bin/zsh',
      }),
    )
    expect(out.slice(0, 2)).toEqual(['/my/special/bin', '/usr/bin'])
  })

  test('still adds the fixed version-manager shim dirs when the probe fails outright', () => {
    // volta/asdf/mise put globally-installed `codex` on a stable path, so a user with a
    // broken rc (or a passwd shell that never sources it) stays reachable without a probe
    const { exec } = recordingExec(() => {
      throw new Error('rc exploded')
    })
    const out = dirs(resolvePath({ current: '/usr/bin', home: HOME, platform: 'darwin', exec, shell: '/bin/zsh' }))

    expect(out).toEqual([
      '/usr/bin',
      `${HOME}/.bun/bin`,
      `${HOME}/.local/bin`,
      `${HOME}/.cargo/bin`,
      `${HOME}/.volta/bin`,
      `${HOME}/.asdf/shims`,
      `${HOME}/.local/share/mise/shims`,
      '/opt/homebrew/bin',
      '/usr/local/bin',
    ])
  })

  test('does not touch process.env.PATH — only fixPath may do that', () => {
    const before = process.env.PATH
    const { exec } = recordingExec(emits('/some/probe/bin'))
    resolvePath({ current: '/usr/bin', home: HOME, platform: 'darwin', exec, shell: '/bin/zsh' })
    expect(process.env.PATH).toBe(before)
  })

  test('breadcrumb reports the probe result, the growth, and where bun landed', () => {
    // both prior PATH regressions were silent; this line is the only evidence a
    // third one leaves behind, so its content is part of the contract
    const { exec } = recordingExec(emits('/opt/homebrew/bin:/usr/bin'))
    const lines: string[] = []
    resolvePath({
      current: '/usr/bin:/bin',
      home: HOME,
      platform: 'darwin',
      exec,
      shell: '/bin/zsh',
      log: (line: string) => lines.push(line),
      access: (p: string) => {
        if (p !== `${HOME}/.bun/bin/bun`) throw new Error('ENOENT')
      },
    })

    expect(lines).toHaveLength(1)
    expect(lines[0]).toContain('shell=/bin/zsh (explicit)')
    expect(lines[0]).toContain('probe=ok:2dirs')
    expect(lines[0]).toContain('dirs:2->10')
    expect(lines[0]).toContain(`bun=${HOME}/.bun/bin/bun`)
    // the one that actually keeps regressing
    expect(lines[0]).toContain('codex=MISSING')
  })

  test('breadcrumb names the passwd fallback and a failed probe', () => {
    const { exec } = recordingExec(() => {
      throw new Error('nope')
    })
    const lines: string[] = []
    resolvePath({
      current: '/usr/bin',
      home: HOME,
      platform: 'darwin',
      exec,
      shellEnv: undefined,
      userInfo: () => ({ shell: '/bin/zsh' }),
      log: (line: string) => lines.push(line),
      access: () => {
        throw new Error('ENOENT')
      },
    })
    expect(lines[0]).toContain('shell=/bin/zsh (passwd)')
    expect(lines[0]).toContain('probe=failed')
  })
})

describe('fixPath', () => {
  const original = process.env.PATH
  afterEach(() => {
    process.env.PATH = original
  })

  test('writes the repaired PATH into the env children will inherit', () => {
    // children snapshot the env at spawn time, so returning the string is not enough
    const { exec } = recordingExec(emits('/opt/homebrew/bin'))
    const next = fixPath({
      current: '/usr/bin',
      home: '/Users/tester',
      platform: 'darwin',
      exec,
      shell: '/bin/zsh',
    })
    expect(process.env.PATH).toBe(next)
    expect(dirs(next).slice(0, 2)).toEqual(['/usr/bin', '/opt/homebrew/bin'])
  })
})
