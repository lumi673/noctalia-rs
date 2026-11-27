'use strict'

const fs = require('node:fs')
const path = require('node:path')

const {
  validateFloor,
  validateMeta,
  validateReport,
  shardPlan,
} = require('./performance.cjs')

function fail(message) {
  throw new Error(message)
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'))
}

function merge(input, output) {
  if (!input || !output) fail('--input and --output are required')
  const dirs = fs
    .readdirSync(input, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && fs.existsSync(path.join(input, entry.name, 'shard.json')))
    .map((entry) => path.join(input, entry.name))
    .sort()
  if (dirs.length === 0) fail(`no shard directories found in ${input}`)

  const shards = dirs.map((dir) => ({ dir, shard: readJson(path.join(dir, 'shard.json')) }))
  const indexes = shards.map(({ shard }) => shard.index)
  if (
    indexes.some((index) => !Number.isSafeInteger(index) || index < 0 || index >= shards.length) ||
    new Set(indexes).size !== indexes.length
  )
    fail('shards must have unique indexes from 0 to the shard count')

  const meta = validateMeta(readJson(path.join(shards[0].dir, 'meta.json')))
  const floor = validateFloor(readJson(path.join(shards[0].dir, 'floor.json')))
  const plan = shardPlan(floor, shards.length, meta.headSha, meta.baseSha)
  for (const { shard } of shards) {
    const expected = plan[shard.index]
    if (!expected || JSON.stringify(expected.shas) !== JSON.stringify(shard.shas ?? []))
      fail(`shard ${shard.index} does not match the floor plan`)
  }

  const ownerFor = (name) => {
    const sha = floor.metrics[name]?.sha
    if (!sha || sha === meta.headSha || sha === meta.baseSha) return 0
    const index = plan.find((entry) => entry.shas.includes(sha))?.index
    if (index === undefined) fail(`no shard measures ceiling ${sha}`)
    return index
  }

  const reports = new Map()
  const add = (sha, result) => {
    let record = reports.get(sha)
    if (!record) {
      record = new Map()
      reports.set(sha, record)
    }
    if (record.has(result.name)) fail(`${sha} has a duplicate metric ${result.name}`)
    record.set(result.name, result)
  }

  const names = new Set()
  for (const { dir, shard } of shards) {
    const head = validateReport(
      readJson(path.join(dir, `${meta.headSha}.json`)),
      `${meta.headSha} in shard ${shard.index}`
    )
    for (const result of head.results) {
      names.add(result.name)
      if (ownerFor(result.name) === shard.index) add(meta.headSha, result)
    }
    for (const sha of shard.shas) {
      const report = validateReport(readJson(path.join(dir, `${sha}.json`)), `${sha} in shard ${shard.index}`)
      for (const result of report.results)
        if (sha === meta.baseSha || ownerFor(result.name) === shard.index) add(sha, result)
    }
  }

  for (const name of names)
    if (!reports.get(meta.headSha)?.has(name)) fail(`the PR has no merged result for ${name}`)
  for (const [name, record] of Object.entries(floor.metrics))
    if (!reports.get(record.sha)?.has(name)) fail(`ceiling ${record.sha} has no merged result for ${name}`)

  const baseline = validateReport(
    readJson(path.join(shards[0].dir, `${meta.headSha}.json`)),
    meta.headSha
  )
  fs.mkdirSync(output, { recursive: true })
  fs.writeFileSync(path.join(output, 'meta.json'), `${JSON.stringify(meta, null, 2)}\n`)
  fs.writeFileSync(path.join(output, 'floor.json'), `${JSON.stringify(floor, null, 2)}\n`)
  for (const [sha, results] of reports) {
    if (results.size === 0) continue
    const merged = {
      method: baseline.method,
      runtime: baseline.runtime,
      os: baseline.os,
      cpu: baseline.cpu,
      results: [...results.values()].sort((left, right) => left.name.localeCompare(right.name)),
    }
    validateReport(merged, sha)
    fs.writeFileSync(path.join(output, `${sha}.json`), JSON.stringify(merged))
  }
  return reports.size
}

function main() {
  const [input = 'shards', output = 'performance'] = process.argv.slice(2)
  const commits = merge(input, output)
  console.log(`Merged ${commits} commits into ${output}.`)
}

module.exports = { merge }

if (require.main === module) {
  try {
    main()
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exit(1)
  }
}
