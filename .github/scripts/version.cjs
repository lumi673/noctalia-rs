'use strict'

const fs = require('node:fs')

const KINDS = ['patch', 'minor', 'major']
const SEMVER = /^(\d+)\.(\d+)\.(\d+)$/u
const SECTION = /^\[([^\]]+)\]$/u
const VERSION_KEY = /^(\s*version\s*=\s*")([^"]*)(")/u
const DEP_PATH = /\bpath\s*=/u
const DEP_VERSION = /(\bversion\s*=\s*")[^"]*(")/u

function fail(message) {
  throw new Error(message)
}

function parse(version) {
  const match = SEMVER.exec(version)
  if (!match) fail(`Unsupported version "${version}". Expected MAJOR.MINOR.PATCH.`)
  return match.slice(1, 4).map(Number)
}

function bump(version, kind) {
  if (!KINDS.includes(kind)) fail(`Unknown bump "${kind}". Use ${KINDS.join(', ')}.`)
  const [major, minor, patch] = parse(version)
  if (kind === 'major') return `${major + 1}.0.0`
  if (kind === 'minor') return `${major}.${minor + 1}.0`
  return `${major}.${minor}.${patch + 1}`
}

// Rewrites [workspace.package] version and the version requirement of every
// path dependency in [workspace.dependencies].
function setVersion(toml, version) {
  parse(version)
  const lines = toml.split('\n')
  let section = null
  let packageVersions = 0
  let dependencies = 0

  const updated = lines.map((line) => {
    const header = SECTION.exec(line)
    if (header) {
      section = header[1].trim()
      return line
    }
    if (section === 'workspace.package' && VERSION_KEY.test(line)) {
      packageVersions += 1
      return line.replace(VERSION_KEY, `$1${version}$3`)
    }
    if (section === 'workspace.dependencies' && DEP_PATH.test(line) && DEP_VERSION.test(line)) {
      dependencies += 1
      return line.replace(DEP_VERSION, `$1${version}$2`)
    }
    return line
  })

  if (packageVersions !== 1) fail(`Found ${packageVersions} version keys in [workspace.package], expected 1.`)
  if (dependencies === 0) fail('Found no path dependencies with a version requirement.')
  return updated.join('\n')
}

function currentVersion(toml) {
  let section = null
  for (const line of toml.split('\n')) {
    const header = SECTION.exec(line)
    if (header) {
      section = header[1].trim()
      continue
    }
    const match = section === 'workspace.package' ? VERSION_KEY.exec(line) : null
    if (match) return match[2]
  }
  return fail('No version key in [workspace.package].')
}

function main() {
  const [command, value, file = 'Cargo.toml'] = process.argv.slice(2)
  if (command === 'current') {
    process.stdout.write(`${currentVersion(fs.readFileSync(file, 'utf8'))}\n`)
    return
  }
  if (command === 'next') {
    process.stdout.write(`${bump(currentVersion(fs.readFileSync(file, 'utf8')), value)}\n`)
    return
  }
  if (command === 'set') {
    fs.writeFileSync(file, setVersion(fs.readFileSync(file, 'utf8'), value))
    return
  }
  fail('Use current, next, or set.')
}
module.exports = { KINDS, bump, currentVersion, setVersion }

if (require.main === module) {
  try {
    main()
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exit(1)
  }
}
