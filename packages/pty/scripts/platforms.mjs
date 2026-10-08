// Prebuilt binaries for @termysh/pty. Each entry becomes an npm package named
// `@termysh/pty-<key>` containing `termy-pty.<key>.node`. The runtime loader
// (src/native.ts) derives the same key from process.platform, process.arch
// and, on Linux, the C library.
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
  if (!platform) throw new Error(`unsupported target ${target}; see scripts/platforms.mjs`)
  return platform
}

export function hostPlatform() {
  const { platform, arch } = process
  let libc
  if (platform === 'linux') {
    process.report.excludeNetwork = true
    libc = process.report.getReport().header.glibcVersionRuntime ? 'glibc' : 'musl'
  }
  const match = platforms.find((entry) => entry.os === platform && entry.cpu === arch && entry.libc === libc)
  if (!match) throw new Error(`@termysh/pty has no prebuilt binary for ${platform}-${arch}`)
  return match
}
