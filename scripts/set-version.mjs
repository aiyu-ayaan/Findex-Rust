#!/usr/bin/env node
// Writes one version into the workspace manifest and the lockfile, so every crate and the
// `findex --version` output carry the release's name.
//   node scripts/set-version.mjs 0.0.1-alpha.1
import { readFileSync, writeFileSync } from 'node:fs'

const [version] = process.argv.slice(2)
if (!/^\d+\.\d+\.\d+(-(alpha|beta)\.\d+)?$/.test(version ?? '')) throw new Error(`Invalid version '${version}'.`)

const edit = (path, change) => writeFileSync(path, change(readFileSync(path, 'utf8')))
// Fails loudly when a manifest no longer has the line, rather than releasing a stale version.
const replace = (text, pattern, value, path) => {
  if (!pattern.test(text)) throw new Error(`${path}: nothing matches ${pattern}`)
  return text.replace(pattern, value)
}

edit('Cargo.toml', (t) => replace(t, /(\[workspace\.package\][^[]*?\nversion = ")[^"]*/, `$1${version}`, 'Cargo.toml'))
edit('Cargo.lock', (t) => t.replace(/(name = "findex-[a-z-]+"\nversion = ")[^"]*/g, `$1${version}`))
console.log(`version ${version}`)
