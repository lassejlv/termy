// Prepares @termysh/pty and its per-platform packages for publishing.
//
//   node scripts/prepare-publish.mjs <binaries-dir>
//
// <binaries-dir> must contain termy-pty.<key>.node for every entry in
// platforms.mjs. Writes npm/<key>/ (package.json, README, binary) and adds
// them to @termysh/pty's optionalDependencies at the same version, so the
// main package is never published pointing at a missing platform package.
// Run after scripts/set-version.mjs.
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { binaryName, platforms } from './platforms.mjs'

const source = process.argv[2]
if (!source) {
  console.error('usage: prepare-publish.mjs <binaries-dir>')
  process.exit(1)
}
const binaries = resolve(source)
const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const manifestPath = join(root, 'package.json')
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'))

const missing = platforms.filter(({ key }) => !existsSync(join(binaries, binaryName(key))))
if (missing.length) {
  throw new Error(`missing binaries in ${binaries}: ${missing.map(({ key }) => binaryName(key)).join(', ')}`)
}

const npm = join(root, 'npm')
rmSync(npm, { recursive: true, force: true })
const optionalDependencies = {}
for (const platform of platforms) {
  const name = `${manifest.name}-${platform.key}`
  const binary = binaryName(platform.key)
  const directory = join(npm, platform.key)
  mkdirSync(directory, { recursive: true })
  copyFileSync(join(binaries, binary), join(directory, binary))
  const pkg = {
    name,
    version: manifest.version,
    description: `${manifest.name} native binary for ${platform.key}.`,
    license: manifest.license,
    repository: manifest.repository,
    os: [platform.os],
    cpu: [platform.cpu],
    ...(platform.libc ? { libc: [platform.libc] } : {}),
    main: binary,
    files: [binary],
    engines: manifest.engines,
    publishConfig: manifest.publishConfig,
  }
  writeFileSync(join(directory, 'package.json'), `${JSON.stringify(pkg, null, 2)}\n`)
  writeFileSync(
    join(directory, 'README.md'),
    `# ${name}\n\nThe ${platform.key} native binary for [\`${manifest.name}\`](https://www.npmjs.com/package/${manifest.name}). ` +
      `Install \`${manifest.name}\` instead; npm picks this package automatically.\n`,
  )
  optionalDependencies[name] = manifest.version
  console.log(`${name}@${manifest.version}`)
}

manifest.optionalDependencies = optionalDependencies
writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`)
