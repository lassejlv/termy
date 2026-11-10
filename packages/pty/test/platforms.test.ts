import { existsSync } from 'node:fs'
import { afterEach, describe, expect, test, vi } from 'vitest'
import { binaryName, hostPlatform, platformForTarget, platformKey } from '../platforms.mjs'

vi.mock('node:fs', () => ({ existsSync: vi.fn() }))

const original = Object.getOwnPropertyDescriptors(process)

afterEach(() => {
  for (const key of ['platform', 'arch', 'report']) Object.defineProperty(process, key, original[key]!)
  vi.resetAllMocks()
})

function host(os: string, cpu: string, report: () => unknown = () => ({})): void {
  Object.defineProperty(process, 'platform', { configurable: true, value: os })
  Object.defineProperty(process, 'arch', { configurable: true, value: cpu })
  Object.defineProperty(process, 'report', { configurable: true, value: { getReport: report } })
}

describe('native platform selection', () => {
  test.each([
    ['darwin', 'arm64', undefined, 'darwin-arm64', 'aarch64-apple-darwin'],
    ['darwin', 'x64', undefined, 'darwin-x64', 'x86_64-apple-darwin'],
    ['win32', 'x64', undefined, 'win32-x64-msvc', 'x86_64-pc-windows-msvc'],
    ['win32', 'arm64', undefined, 'win32-arm64-msvc', 'aarch64-pc-windows-msvc'],
    ['linux', 'x64', '2.17', 'linux-x64-gnu', 'x86_64-unknown-linux-gnu'],
    ['linux', 'arm64', '2.17', 'linux-arm64-gnu', 'aarch64-unknown-linux-gnu'],
    ['linux', 'x64', undefined, 'linux-x64-musl', 'x86_64-unknown-linux-musl'],
    ['linux', 'arm64', undefined, 'linux-arm64-musl', 'aarch64-unknown-linux-musl'],
  ])('%s %s with glibc %s selects %s', (os, cpu, glibc, key, target) => {
    host(os!, cpu!, () => ({ header: { glibcVersionRuntime: glibc } }))
    expect(platformKey()).toBe(key)
    expect(hostPlatform().target).toBe(target)
    expect(platformForTarget(target!).key).toBe(key)
    expect(binaryName(key!)).toBe(`termy-pty.${key}.node`)
  })

  test.each([true, false])('uses Alpine detection when diagnostic reports fail (Alpine: %s)', (alpine) => {
    host('linux', 'x64', () => { throw new Error('report unavailable') })
    vi.mocked(existsSync).mockReturnValue(alpine)
    expect(platformKey()).toBe(alpine ? 'linux-x64-musl' : 'linux-x64-gnu')
    expect(hostPlatform().libc).toBe(alpine ? 'musl' : 'glibc')
  })

  test('unsupported hosts remain identifiable for the runtime error', () => {
    host('freebsd', 'x64')
    expect(platformKey()).toBe('freebsd-x64')
    expect(() => hostPlatform()).toThrow('no prebuilt binary for freebsd-x64')
    expect(() => platformForTarget('x86_64-unknown-freebsd')).toThrow('unsupported target')
  })
})
