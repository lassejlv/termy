// @vitest-environment happy-dom
import { readFileSync } from 'node:fs'
import { beforeAll, expect, it } from 'vitest'
import { Terminal, initSync, type ProgramStatusRecord } from '../src/index.ts'

beforeAll(() => {
  initSync(readFileSync('core/src/wasm/termy_wasm_bg.wasm'))
})

it('delivers status and process exit queued before the web terminal is ready', async () => {
  const terminal = new Terminal()
  const snapshots: ProgramStatusRecord[][] = []
  const subscription = terminal.onProgramStatus((records) => snapshots.push(records))
  terminal.write('\x1b]7501;state=working\x07\x1b]7501;state=done:id=tests\x07')
  terminal.processExited()
  await terminal.ready
  expect(snapshots.map((records) => records.map((record) => record.state))).toEqual([
    ['working', 'done'], ['done'],
  ])
  terminal.reset()
  expect(snapshots.at(-1)).toEqual([])
  subscription.dispose()
  terminal.dispose()
})
