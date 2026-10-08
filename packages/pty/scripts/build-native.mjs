// Builds crates/pty and copies the addon to packages/pty/termy-pty.<key>.node,
// where the loader finds it during development and tests.
//
//   node scripts/build-native.mjs                     # host, release
//   node scripts/build-native.mjs --debug             # host, debug
//   node scripts/build-native.mjs --target <triple>   # cross target (CI)
//   node scripts/build-native.mjs --target <triple> --zig --glibc 2.17
//
// --zig builds with cargo-zigbuild, which links Linux binaries against an old
// glibc (--glibc) and builds the musl targets without a musl toolchain.
import { execFileSync } from 'node:child_process'
import { copyFileSync, existsSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { parseArgs } from 'node:util'
import { fileURLToPath } from 'node:url'
import { binaryName, hostPlatform, platformForTarget } from './platforms.mjs'

const { values } = parseArgs({
  options: {
    target: { type: 'string' },
    debug: { type: 'boolean', default: false },
    zig: { type: 'boolean', default: false },
    glibc: { type: 'string' },
  },
})

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const repo = join(root, '..', '..')
const platform = values.target ? platformForTarget(values.target) : hostPlatform()
const profile = values.debug ? 'debug' : 'release'

const cargoTarget = values.glibc && platform.libc === 'glibc' ? `${platform.target}.${values.glibc}` : platform.target
const args = [values.zig ? 'zigbuild' : 'build', '-p', 'termy_pty', '--locked']
if (!values.debug) args.push('--release')
if (values.target) args.push('--target', cargoTarget)

const env = { ...process.env }
if (platform.libc === 'musl') {
  // A Node addon must be a dynamic library, which musl's static CRT forbids.
  env.RUSTFLAGS = `${env.RUSTFLAGS ?? ''} -C target-feature=-crt-static`.trim()
}
execFileSync('cargo', args, { cwd: repo, stdio: 'inherit', env })

const library =
  platform.os === 'win32' ? 'termy_pty.dll' : platform.os === 'darwin' ? 'libtermy_pty.dylib' : 'libtermy_pty.so'
const built = join(repo, 'target', ...(values.target ? [platform.target] : []), profile, library)
if (!existsSync(built)) throw new Error(`cargo did not produce ${built}`)
const out = join(root, binaryName(platform.key))
copyFileSync(built, out)
console.log(`built ${out}`)
