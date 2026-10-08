import { mkdtempSync, realpathSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, test } from 'vitest'
import pty, { type IExitEvent, type IPty, Terminal, isSupported, spawn } from '../src/index.ts'
import { parseCommandLine } from '../src/terminal.ts'

const isWindows = process.platform === 'win32'

interface Run {
  output: string
  exit: IExitEvent
}

/** Collect all output until the program exits. */
function collect(terminal: IPty, timeout = 10_000): Promise<Run> {
  return new Promise((resolve, reject) => {
    let output = ''
    const timer = setTimeout(() => reject(new Error(`timed out; output so far: ${JSON.stringify(output)}`)), timeout)
    terminal.onData((data) => {
      output += data
    })
    terminal.onExit((exit) => {
      clearTimeout(timer)
      resolve({ output, exit })
    })
  })
}

/** Resolve once the output contains `marker`. */
function waitFor(terminal: IPty, marker: string, timeout = 10_000): Promise<string> {
  return new Promise((resolve, reject) => {
    let output = ''
    const timer = setTimeout(() => reject(new Error(`no ${JSON.stringify(marker)} in ${JSON.stringify(output)}`)), timeout)
    const subscription = terminal.onData((data) => {
      output += data
      if (output.includes(marker)) {
        clearTimeout(timer)
        subscription.dispose()
        resolve(output)
      }
    })
  })
}

const sh = (script: string, options = {}): IPty => spawn('/bin/sh', ['-c', script], options)

describe('platform', () => {
  test('reports support', () => {
    expect(isSupported()).toBe(true)
  })

  test('exposes node-pty style exports', () => {
    expect(pty.spawn).toBe(spawn)
    expect(pty.fork).toBe(spawn)
    expect(pty.createTerminal).toBe(spawn)
  })
})

describe.skipIf(isWindows)('unix', () => {
  test('runs a program and reports its exit code', async () => {
    const terminal = sh('printf hello; exit 3')
    expect(terminal).toBeInstanceOf(Terminal)
    expect(terminal.pid).toBeGreaterThan(0)
    const { output, exit } = await collect(terminal)
    expect(output).toBe('hello')
    expect(exit).toEqual({ exitCode: 3, signal: 0 })
  })

  test('reports the signal that ended the program', async () => {
    const terminal = spawn('sleep', ['30'])
    const done = collect(terminal)
    terminal.kill('SIGTERM')
    expect((await done).exit).toEqual({ exitCode: 0, signal: 15 })
  })

  test('kill sends SIGHUP by default', async () => {
    const terminal = spawn('sleep', ['30'])
    const done = collect(terminal)
    terminal.kill()
    expect((await done).exit.signal).toBe(1)
  })

  test('rejects unknown signals', () => {
    const terminal = spawn('sleep', ['30'])
    expect(() => terminal.kill('SIGNOPE')).toThrow('Unknown signal: SIGNOPE')
    terminal.kill('SIGKILL')
  })

  test('throws when the program cannot start', () => {
    expect(() => spawn('termy-pty-definitely-missing', [])).toThrow()
    expect(() => spawn('/bin/sh', [], { cwd: '/termy-pty-missing-directory' })).toThrow()
  })

  test('writes input and echoes it through the terminal', async () => {
    const terminal = sh('IFS= read -r line; printf "<%s>" "$line"')
    const done = collect(terminal)
    terminal.write('typed text\r')
    const { output } = await done
    expect(output).toContain('typed text')
    expect(output).toContain('<typed text>')
  })

  test('accepts Buffer input', async () => {
    const terminal = sh('IFS= read -r line; printf "<%s>" "$line"')
    const done = collect(terminal)
    terminal.write(Buffer.from('bytes\r'))
    expect((await done).output).toContain('<bytes>')
  })

  test('sets the size and resizes', async () => {
    const terminal = sh('stty size; IFS= read -r _; stty size', { cols: 100, rows: 30 })
    expect(terminal.cols).toBe(100)
    expect(terminal.rows).toBe(30)
    const done = collect(terminal)
    await waitFor(terminal, '30 100')
    terminal.resize(120, 40)
    expect([terminal.cols, terminal.rows]).toEqual([120, 40])
    terminal.write('\r')
    expect((await done).output).toContain('40 120')
  })

  test('validates dimensions', () => {
    expect(() => sh('true', { cols: 0 })).toThrow('cols must be a positive integer')
    const terminal = sh('sleep 30')
    expect(() => terminal.resize(80, -1)).toThrow('rows must be a positive integer')
    expect(() => terminal.resize(Number.NaN, 24)).toThrow('cols must be a positive integer')
    terminal.kill('SIGKILL')
  })

  test('uses exactly the given environment, with TERM from name', async () => {
    const terminal = sh('printf "%s|%s|%s" "$TERM" "$ONLY" "${HOME-unset}"', {
      name: 'xterm-256color',
      env: { ONLY: 'yes', PATH: process.env.PATH },
    })
    expect((await collect(terminal)).output).toBe('xterm-256color|yes|unset')
  })

  test('inherits process.env without multiplexer variables', async () => {
    process.env.TERMY_PTY_TEST = 'inherited'
    process.env.TMUX = '/tmp/tmux-socket,1,0'
    try {
      const terminal = sh('printf "%s|%s" "$TERMY_PTY_TEST" "${TMUX-unset}"')
      expect((await collect(terminal)).output).toBe('inherited|unset')
    } finally {
      delete process.env.TERMY_PTY_TEST
      delete process.env.TMUX
    }
  })

  test('runs in cwd and exports PWD', async () => {
    const directory = realpathSync(mkdtempSync(join(tmpdir(), 'termy-pty-')))
    const terminal = sh('printf "%s|%s" "$(pwd -P)" "$PWD"', { cwd: directory })
    expect((await collect(terminal)).output).toBe(`${directory}|${directory}`)
  })

  test('decodes UTF-8 split across reads', async () => {
    const text = 'æøå 漢字 🙂 '.repeat(4000)
    const terminal = sh(`printf '%s' '${text}'`)
    expect((await collect(terminal)).output).toBe(text)
  })

  test('emits Buffers with encoding null', async () => {
    const terminal = sh('printf abc', { encoding: null })
    const chunks: unknown[] = []
    terminal.onData((data) => chunks.push(data))
    await collect(terminal)
    expect(chunks.every((chunk) => Buffer.isBuffer(chunk))).toBe(true)
    expect(Buffer.concat(chunks as Buffer[]).toString()).toBe('abc')
  })

  test('delivers large output completely before exit', async () => {
    const terminal = sh('i=0; while [ $i -lt 20000 ]; do echo "line $i"; i=$((i+1)); done')
    const { output, exit } = await collect(terminal, 30_000)
    expect(exit.exitCode).toBe(0)
    const lines = output.trim().split('\r\n')
    expect(lines).toHaveLength(20000)
    expect(lines.at(-1)).toBe('line 19999')
  })

  test('pause stops output and resume continues it', async () => {
    const terminal = sh('i=0; while [ $i -lt 3000 ]; do echo "line $i"; i=$((i+1)); done')
    const done = collect(terminal, 30_000)
    await waitFor(terminal, 'line 1')
    terminal.pause()
    let received = 0
    const counter = terminal.onData((data) => {
      received += data.length
    })
    await new Promise((resolve) => setTimeout(resolve, 300))
    const whilePaused = received
    await new Promise((resolve) => setTimeout(resolve, 300))
    // At most the chunks already queued for JavaScript arrive after pausing.
    expect(received).toBe(whilePaused)
    counter.dispose()
    terminal.resume()
    expect((await done).output).toContain('line 2999')
  })

  test('handleFlowControl turns XOFF and XON writes into pause and resume', async () => {
    const terminal = sh('IFS= read -r line; printf "<%s>" "$line"', { handleFlowControl: true })
    const done = collect(terminal)
    terminal.write('\x13')
    terminal.write('\x11')
    terminal.write('ok\r')
    expect((await done).output).toContain('<ok>')
  })

  test('reports the foreground process', async () => {
    const terminal = sh('exec sleep 30')
    const deadline = Date.now() + 5000
    while (terminal.process !== 'sleep' && Date.now() < deadline) {
      await new Promise((resolve) => setTimeout(resolve, 20))
    }
    expect(terminal.process).toBe('sleep')
    terminal.kill('SIGKILL')
  })

  test('ignores writes, resizes and kills after exit', async () => {
    const terminal = sh('exit 0')
    await collect(terminal)
    expect(() => {
      terminal.write('late\r')
      terminal.resize(90, 30)
      terminal.kill()
    }).not.toThrow()
    expect(terminal.process).toBe('sh')
  })

  test('supports EventEmitter-style listeners', async () => {
    // Like node-pty, IPty does not declare on(); the Terminal class does.
    const terminal = sh('printf hi; exit 2') as Terminal
    let data = ''
    const exit = new Promise<[number, number | undefined]>((resolve) => {
      terminal.on('exit', (code, signal) => resolve([code, signal]))
    })
    terminal.on('data', (chunk) => {
      data += chunk
    })
    expect(await exit).toEqual([2, 0])
    expect(data).toBe('hi')
  })

  test('rejects uid and gid', () => {
    expect(() => spawn('/bin/sh', [], { uid: 0 })).toThrow('does not support the uid option')
  })
})

describe.runIf(isWindows)('windows', () => {
  test('runs a program and reports its exit code', async () => {
    const terminal = spawn('cmd.exe', ['/c', 'echo hello & exit /b 3'])
    const { output, exit } = await collect(terminal)
    expect(output).toContain('hello')
    expect(exit).toEqual({ exitCode: 3 })
  })

  test('kill terminates and rejects signals', async () => {
    const terminal = spawn('cmd.exe', [])
    expect(() => terminal.kill('SIGTERM')).toThrow('Signals not supported on windows.')
    const done = collect(terminal)
    terminal.kill()
    expect((await done).exit.exitCode).toBe(1)
  })

  test('writes input', async () => {
    const terminal = spawn('cmd.exe', [])
    const done = collect(terminal)
    terminal.write('echo typed-%COMSPEC:~0,1%\r')
    await waitFor(terminal, 'typed-')
    terminal.write('exit\r')
    expect((await done).output).toContain('typed-')
  })
})

describe('parseCommandLine', () => {
  test('follows CommandLineToArgvW rules', () => {
    expect(parseCommandLine('a b  c')).toEqual(['a', 'b', 'c'])
    expect(parseCommandLine('"a b" c')).toEqual(['a b', 'c'])
    expect(parseCommandLine('"" x')).toEqual(['', 'x'])
    expect(parseCommandLine('a\\\\b')).toEqual(['a\\\\b'])
    expect(parseCommandLine('a\\"b')).toEqual(['a"b'])
    expect(parseCommandLine('a\\\\"b c"')).toEqual(['a\\b c'])
    expect(parseCommandLine('"a""b"')).toEqual(['a"b'])
  })
})
