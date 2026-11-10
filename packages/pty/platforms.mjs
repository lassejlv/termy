import { existsSync } from 'node:fs'

// Prebuilt binaries for @termysh/pty. Each entry becomes an npm package named
// `@termysh/pty-<key>` containing `termy-pty.<key>.node`. The runtime loader
// (src/native.ts) uses this same catalog and host detection.
export const platforms = [
  { key: 'darwin-arm64', target: 'aarch64-apple-darwin', os: 'darwin', cpu: 'arm64' },
  { key: 'darwin-x64', target: 'x86_64-apple-darwin', os: 'darwin', cpu: 'x64' },
  { key: 'linux-x64-gnu', target: 'x86_64-unknown-linux-gnu', os: 'linux', cpu: 'x64', libc: 'glibc' },
  { key: 'linux-arm64-gnu', target: 'aarch64-unknown-linux-gnu', os: 'linux', cpu: 'arm64', libc: 'glibc' },
  { key: 'linux-x64-musl', target: 'x86_64-unknown-linux-musl', os: 'linux', cpu: 'x64', libc: 'musl' },
  { key: 'linux-arm64-musl', target: 'aarch64-unknown-linux-musl', os: 'linux', cpu: 'arm64', libc: 'musl' },
  { key: 'win32-x64-msvc', target: 'x86_64-pc-windows-msvc', os: 'win32', cpu: 'x64' },
  { key: 'win32-arm64-msvc', target: 'aarch64-pc-windows-msvc', os: 'win32', cpu: 'arm64' },
]

export const binaryName = (key) => `termy-pty.${key}.node`

export function platformForTarget(target) {
  const platform = platforms.find((entry) => entry.target === target)
  if (!platform) throw new Error(`unsupported target ${target}; see platforms.mjs`)
  return platform
}

export function platformKey() {
  const { platform, arch } = process
  switch (platform) {
    case 'darwin':
      return `darwin-${arch}`
    case 'win32':
      return `win32-${arch}-msvc`
    case 'linux':
      return `linux-${arch}-${isMusl() ? 'musl' : 'gnu'}`
    default:
      return `${platform}-${arch}`
  }
}

function isMusl() {
  try {
    process.report.excludeNetwork = true
    return !process.report.getReport().header?.glibcVersionRuntime
  } catch {
    return existsSync('/etc/alpine-release')
  }
}

export function hostPlatform() {
  const key = platformKey()
  const match = platforms.find((entry) => entry.key === key)
  if (!match) throw new Error(`@termysh/pty has no prebuilt binary for ${process.platform}-${process.arch}`)
  return match
}
