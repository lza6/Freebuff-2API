import { describe, expect, test } from 'bun:test'
import { createRequire } from 'node:module'
import { join } from 'node:path'

const require_ = createRequire(import.meta.url)
const runtime = require_('./bun-runtime.cjs') as {
  bunRuntimeCandidates: (
    resources: string,
    platform: string,
    exists: (p: string) => boolean,
  ) => { standard: string | null; baseline: string | null }
  installerFlavor: (c: {
    standard: string | null
    baseline: string | null
  }) => string
  probeBunRuns: (exe: string, exec: (...args: any[]) => string) => boolean
  binaryIdentity: (
    exe: string,
    stat: (p: string) => { size: number; mtimeMs: number },
  ) => string | null
  createRuntimeMemory: (opts: { file: string; fs: any }) => {
    read: (id: string) => string | null
    write: (id: string, v: string) => void
    forget: () => void
  }
  chooseBunRuntime: (input: {
    candidates: { standard: string | null; baseline: string | null }
    probe: (exe: string) => boolean
    memory?: {
      read: (id: string) => string | null
      write: (id: string, v: string) => void
    }
    identity?: string | null
  }) => {
    bun: string
    runtimeFlavor: string
    installerFlavor: string
    reason: string
  }
}

const resources = join('app', 'resources')
const standard = join(resources, 'bun', 'bun.exe')
const baseline = join(resources, 'bun', 'bun-baseline.exe')
const both = { standard, baseline }

function memoryStub() {
  let saved: string | null = null
  const fs = {
    readFileSync: () => {
      if (saved === null) throw new Error('ENOENT')
      return saved
    },
    writeFileSync: (_p: string, data: string) => {
      saved = data
    },
    rmSync: () => {
      saved = null
    },
  }
  return {
    memory: runtime.createRuntimeMemory({ file: 'bun-runtime.json', fs }),
    saved: () => saved,
  }
}

describe('what is on disk', () => {
  test('the standard Windows package carries both binaries', () => {
    expect(
      runtime.bunRuntimeCandidates(resources, 'win32', () => true),
    ).toEqual(both)
  })

  test('the baseline package and the unix packages carry one', () => {
    const only = (name: string) => (p: string) => p.endsWith(name)
    expect(
      runtime.bunRuntimeCandidates(
        resources,
        'win32',
        only('bun-baseline.exe'),
      ),
    ).toEqual({
      standard: null,
      baseline,
    })
    expect(
      runtime.bunRuntimeCandidates(
        resources,
        'darwin',
        only(`${join('bun', 'bun')}`),
      ),
    ).toEqual({
      standard: join(resources, 'bun', 'bun'),
      baseline: null,
    })
  })

  test('the installer flavor follows the package, not the binary chosen', () => {
    expect(runtime.installerFlavor(both)).toBe('standard')
    expect(runtime.installerFlavor({ standard: null, baseline })).toBe(
      'baseline',
    )
    expect(runtime.installerFlavor({ standard, baseline: null })).toBe(
      'standard',
    )
  })
})

describe('choosing', () => {
  test('one binary is chosen without a probe', () => {
    const probe = () => {
      throw new Error('must not probe')
    }
    expect(
      runtime.chooseBunRuntime({
        candidates: { standard, baseline: null },
        probe,
      }),
    ).toMatchObject({
      bun: standard,
      runtimeFlavor: 'standard',
      installerFlavor: 'standard',
    })
    expect(
      runtime.chooseBunRuntime({
        candidates: { standard: null, baseline },
        probe,
      }),
    ).toMatchObject({
      bun: baseline,
      runtimeFlavor: 'baseline',
      installerFlavor: 'baseline',
    })
  })

  test('with both, a standard binary that runs is the one used', () => {
    const { memory, saved } = memoryStub()
    const probed: string[] = []
    const choice = runtime.chooseBunRuntime({
      candidates: both,
      probe: (exe) => {
        probed.push(exe)
        return true
      },
      memory,
      identity: 'id-1',
    })
    expect(choice).toMatchObject({
      bun: standard,
      runtimeFlavor: 'standard',
      installerFlavor: 'standard',
    })
    expect(probed).toEqual([standard])
    // nothing to remember: a machine that runs the standard binary is asked again only because
    // the probe is cheap, and remembering "standard" would outlive a CPU swap
    expect(saved()).toBeNull()
  })

  test('with both, a standard binary that cannot run here picks the baseline and remembers it', () => {
    const { memory, saved } = memoryStub()
    const choice = runtime.chooseBunRuntime({
      candidates: both,
      probe: () => false,
      memory,
      identity: 'id-1',
    })
    expect(choice).toMatchObject({
      bun: baseline,
      runtimeFlavor: 'baseline',
      installerFlavor: 'standard',
    })
    expect(JSON.parse(saved()!)).toMatchObject({
      identity: 'id-1',
      verdict: 'baseline',
    })
  })

  test('a remembered verdict skips the probe for the same binary, and not for a replaced one', () => {
    const { memory } = memoryStub()
    memory.write('id-1', 'baseline')
    let probes = 0
    const probe = () => {
      probes++
      return true
    }
    expect(
      runtime.chooseBunRuntime({
        candidates: both,
        probe,
        memory,
        identity: 'id-1',
      }).bun,
    ).toBe(baseline)
    expect(probes).toBe(0)
    // an update replaced bun.exe: the new binary gets its own probe
    expect(
      runtime.chooseBunRuntime({
        candidates: both,
        probe,
        memory,
        identity: 'id-2',
      }).bun,
    ).toBe(standard)
    expect(probes).toBe(1)
  })

  test('forgetting the verdict re-probes', () => {
    const { memory } = memoryStub()
    memory.write('id-1', 'baseline')
    memory.forget()
    expect(
      runtime.chooseBunRuntime({
        candidates: both,
        probe: () => true,
        memory,
        identity: 'id-1',
      }).bun,
    ).toBe(standard)
  })
})

describe('the probe', () => {
  test('a binary that prints a version runs here', () => {
    expect(runtime.probeBunRuns(standard, () => '1.4.2\n')).toBe(true)
  })

  test.each([
    [
      'dies with an illegal instruction',
      () => {
        const error = new Error('spawn failed') as Error & { status: number }
        error.status = 0xc000001d
        throw error
      },
    ],
    [
      'times out',
      () => {
        throw Object.assign(new Error('ETIMEDOUT'), { code: 'ETIMEDOUT' })
      },
    ],
    ['prints garbage', () => 'panic: Illegal instruction'],
  ])('a binary that %s does not', (_name, exec) => {
    expect(runtime.probeBunRuns(standard, exec as never)).toBe(false)
  })

  test('identity is the binary s size and mtime, so an update is a new question', () => {
    const stat = () => ({ size: 123, mtimeMs: 1700000000123.9 })
    expect(runtime.binaryIdentity(standard, stat)).toBe('123:1700000000123')
    expect(
      runtime.binaryIdentity(standard, () => {
        throw new Error('ENOENT')
      }),
    ).toBeNull()
  })
})

describe('retrying on the baseline after a startup crash', () => {
  const bothBundled = {
    baselineBundled: true,
    runtimeFlavor: 'standard' as const,
  }
  const panic = {
    kind: 'exit',
    code: 3,
    stderrTail:
      'Bun v1.4.2 (abc) Windows x64\npanic(main thread): Internal assertion failure\n',
  }

  test('a Bun crash report on the standard binary, with the baseline bundled, retries', () => {
    expect(runtime.retryOnBaseline(panic, bothBundled)).toBe(true)
  })

  test('an illegal-instruction exit leaves no report and still retries', () => {
    expect(
      runtime.retryOnBaseline({ kind: 'exit', code: 0xc000001d }, bothBundled),
    ).toBe(true)
    expect(
      runtime.retryOnBaseline(
        { kind: 'exit', code: 0xc000001d - 2 ** 32 },
        bothBundled,
      ),
    ).toBe(true)
  })

  test.each([
    [
      'the baseline binary already ran',
      panic,
      { baselineBundled: true, runtimeFlavor: 'baseline' },
    ],
    [
      'no baseline is bundled',
      panic,
      { baselineBundled: false, runtimeFlavor: 'standard' },
    ],
    ['nothing was chosen yet', panic, null],
    [
      'the startup only timed out',
      { kind: 'timeout', stderrTail: panic.stderrTail },
      bothBundled,
    ],
    ['the profile was busy', { kind: 'exit', code: 75 }, bothBundled],
    [
      'the app exited on its own',
      { kind: 'exit', code: 1, stderrTail: 'Error: bad config' },
      bothBundled,
    ],
  ])('does not retry when %s', (_name, failure, choice) => {
    expect(runtime.retryOnBaseline(failure as never, choice as never)).toBe(
      false,
    )
  })
})
