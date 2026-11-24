'use strict'

const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const test = require('node:test')

const { bump, currentVersion, setVersion } = require('./version.cjs')

const MANIFEST = `[workspace]
members = ["crates/*"]

[workspace.package]
version      = "0.2.0"
edition      = "2024"

[workspace.dependencies]
chrono             = { version = "0.4.41", default-features = false }
noctalia-rs-build  = { path = "crates/build", version = "0.2.0" }
noctalia-rs        = { path = "crates/lib", version = "0.2.0" }
mlua               = { version = "0.10", features = ["luau"] }
globset            = { version = "0.4", features = ["path"] }

[workspace.lints.rust]
unsafe_code = "deny"
`

test('bump moves one component and resets the rest', () => {
  assert.equal(bump('0.2.0', 'patch'), '0.2.1')
  assert.equal(bump('0.2.0', 'minor'), '0.3.0')
  assert.equal(bump('0.2.0', 'major'), '1.0.0')
  assert.equal(bump('1.9.9', 'patch'), '1.9.10')
  assert.equal(bump('1.9.9', 'minor'), '1.10.0')
})

test('bump rejects unknown kinds and unsupported versions', () => {
  assert.throws(() => bump('0.2.0', 'huge'), /Unknown bump/u)
  assert.throws(() => bump('0.2', 'patch'), /Unsupported version/u)
  assert.throws(() => bump('v0.2.0', 'patch'), /Unsupported version/u)
})

test('currentVersion reads the workspace package version', () => {
  assert.equal(currentVersion(MANIFEST), '0.2.0')
  assert.throws(() => currentVersion('[package]\nname = "x"\n'), /No version key/u)
})

test('setVersion rewrites the workspace version and path dependencies only', () => {
  const updated = setVersion(MANIFEST, '0.3.0')
  assert.match(updated, /^version {6}= "0\.3\.0"$/mu)
  assert.match(updated, /noctalia-rs-build {2}= \{ path = "crates\/build", version = "0\.3\.0" \}/u)
  assert.match(updated, /noctalia-rs {8}= \{ path = "crates\/lib", version = "0\.3\.0" \}/u)
  assert.match(updated, /chrono {13}= \{ version = "0\.4\.41", default-features = false \}/u)
  assert.match(updated, /mlua {15}= \{ version = "0\.10", features = \["luau"\] \}/u)
  assert.match(updated, /globset {12}= \{ version = "0\.4", features = \["path"\] \}/u)
  assert.equal(currentVersion(updated), '0.3.0')
  assert.equal(setVersion(updated, '0.3.0'), updated)
})

test('setVersion refuses a manifest it would rewrite incorrectly', () => {
  assert.throws(() => setVersion('[workspace.package]\nedition = "2024"\n', '1.0.0'), /version keys/u)
  assert.throws(
    () => setVersion('[workspace.package]\nversion = "0.2.0"\n[workspace.dependencies]\nserde = "1.0"\n', '1.0.0'),
    /no path dependencies/u,
  )
  assert.throws(() => setVersion(MANIFEST, 'next'), /Unsupported version/u)
})

test('the repository manifest bumps cleanly', () => {
  const toml = fs.readFileSync(path.join(__dirname, '..', '..', 'Cargo.toml'), 'utf8')
  const version = bump(currentVersion(toml), 'patch')
  const updated = setVersion(toml, version)
  assert.equal(currentVersion(updated), version)
  assert.doesNotMatch(updated, new RegExp(`path = "[^"]+", version = "${currentVersion(toml)}"`, 'u'))
})
