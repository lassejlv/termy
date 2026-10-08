// Usage: node scripts/set-version.mjs <version>
// Sets every @termysh/* package to <version> and pins workspace dependencies
// to it, so `npm publish` never ships a `workspace:` range.
import { readFileSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const version = process.argv[2]
if (!version || !/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version)) {
  console.error('usage: set-version.mjs <semver>')
  process.exit(1)
}
const root = join(dirname(fileURLToPath(import.meta.url)), '..')
for (const dir of ['core', 'web', 'xterm', 'pty']) {
  const file = join(root, dir, 'package.json')
  const pkg = JSON.parse(readFileSync(file, 'utf8'))
  pkg.version = version
  for (const name of Object.keys(pkg.dependencies ?? {})) {
    if (name.startsWith('@termysh/')) pkg.dependencies[name] = version
  }
  writeFileSync(file, `${JSON.stringify(pkg, null, 2)}\n`)
  console.log(`${pkg.name}@${version}`)
}
