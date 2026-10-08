// Dependency-free check of a built package (dist/ plus a native binary), for
// environments where the vitest suite cannot run, such as Alpine containers.
//   node test/smoke.mjs
import assert from 'node:assert/strict'
import { createRequire } from 'node:module'
import { spawn as spawnEsm } from '../dist/index.mjs'

const { spawn: spawnCjs } = createRequire(import.meta.url)('../dist/index.cjs')
const isWindows = process.platform === 'win32'

function run(spawn, file, args, options, input) {
  return new Promise((resolve, reject) => {
    const terminal = spawn(file, args, options)
    const timer = setTimeout(() => reject(new Error(`timed out: ${output}`)), 15_000)
    let output = ''
    terminal.onData((data) => {
      output += data
    })
    terminal.onExit((exit) => {
      clearTimeout(timer)
      resolve({ output, exit, terminal })
    })
    if (input) setTimeout(() => terminal.write(input), 200)
  })
}

for (const [name, spawn] of [['esm', spawnEsm], ['cjs', spawnCjs]]) {
  if (isWindows) {
    const { output, exit } = await run(spawn, 'cmd.exe', ['/c', 'echo smoke & exit /b 3'])
    assert.match(output, /smoke/)
    assert.deepEqual(exit, { exitCode: 3 })
  } else {
    const script = 'stty size; IFS= read -r line; printf "<%s|%s>" "$line" "$TERM"; exit 3'
    const { output, exit } = await run(spawn, '/bin/sh', ['-c', script], { name: 'xterm-256color', cols: 90, rows: 20 }, 'ok\r')
    assert.match(output, /20 90/)
    assert.match(output, /<ok\|xterm-256color>/)
    assert.deepEqual(exit, { exitCode: 3, signal: 0 })

    const sleeper = spawn('sleep', ['30'])
    const killed = new Promise((resolve) => sleeper.onExit(resolve))
    sleeper.kill('SIGTERM')
    assert.deepEqual(await killed, { exitCode: 0, signal: 15 })
  }
  console.log(`${name}: ok`)
}
