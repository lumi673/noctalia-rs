'use strict'

const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const test = require('node:test')

const perf = require('./performance.cjs')
const { merge } = require('./performance-merge.cjs')

const HEAD = 'a'.repeat(40)
const BASE = 'b'.repeat(40)
const CEILING = 'c'.repeat(40)
const OTHER = 'd'.repeat(40)
const NOW = '2026-11-22T00:00:00.000Z'

function metric(name, medianMs) {
  return {
    name,
    criterion: 'test criterion',
    budgetMs: 10,
    samples: 60,
    warmup: 6,
    medianMs,
    worstMedianMs: medianMs,
    p95Ms: medianMs,
    worstP95Ms: medianMs,
    p99Ms: medianMs,
    worstP99Ms: medianMs,
  }
}

function report(results) {
  return { method: perf.METHOD, runtime: 'rustc 1.99.1', os: 'linux', cpu: 'test', results }
}

function write(file, value) {
  fs.writeFileSync(file, JSON.stringify(value))
}


function tempDir() {
  const root = path.join(process.cwd(), '.tmp')
  fs.mkdirSync(root, { recursive: true })
  return fs.mkdtempSync(path.join(root, 'perf-merge-'))
}
function ceilingRecord(sha) {
  return { sha, method: perf.METHOD, medianMs: 1, worstMedianMs: 1, p95Ms: 1, worstP95Ms: 1, p99Ms: 1, worstP99Ms: 1, budgetMs: 10, updatedAt: NOW }
}

function floor() {
  return {
    version: 1,
    method: perf.METHOD,
    metrics: {
      a_ceiling: ceilingRecord(CEILING),
      b_ceiling: ceilingRecord(OTHER),
      c_base: ceilingRecord(BASE),
    },
  }
}

function shards(root, data = floor()) {
  const meta = {
    kind: 'pull_request',
    headSha: HEAD,
    baseSha: BASE,
    floor: Object.fromEntries(Object.entries(data.metrics).map(([name, record]) => [name, record.sha])),
    budgets: {},
  }
  const plan = perf.shardPlan(data, 2, HEAD, BASE)
  const dirs = []
  for (const shard of plan) {
    const dir = path.join(root, `performance-shard-${shard.index}`)
    fs.mkdirSync(dir, { recursive: true })
    write(path.join(dir, 'shard.json'), shard)
    write(path.join(dir, 'meta.json'), meta)
    write(path.join(dir, 'floor.json'), data)
    dirs.push(dir)
  }
  return dirs
}

test('merged measurements pair each workload with the reference from its own shard', () => {
  const root = tempDir()
  const out = path.join(root, 'performance')
  try {
    const [first, second] = shards(root)
    write(path.join(first, `${HEAD}.json`), report([
      metric('a_ceiling', 1), metric('b_ceiling', 9), metric('c_base', 1),
    ]))
    write(path.join(second, `${HEAD}.json`), report([
      metric('a_ceiling', 9), metric('b_ceiling', 1), metric('c_base', 9),
    ]))
    write(path.join(first, `${BASE}.json`), report([
      metric('a_ceiling', 1), metric('b_ceiling', 1), metric('c_base', 1),
    ]))
    write(path.join(first, `${CEILING}.json`), report([
      metric('a_ceiling', 1), metric('b_ceiling', 1),
    ]))
    write(path.join(second, `${OTHER}.json`), report([
      metric('b_ceiling', 1),
    ]))

    assert.equal(merge(root, out), 4)
    const read = (sha) => JSON.parse(fs.readFileSync(path.join(out, `${sha}.json`), 'utf8'))
    const values = (sha) => Object.fromEntries(read(sha).results.map((result) => [result.name, result.medianMs]))
    assert.deepEqual(values(HEAD), { a_ceiling: 1, b_ceiling: 1, c_base: 1 })
    assert.deepEqual(values(CEILING), { a_ceiling: 1 })
    assert.deepEqual(values(OTHER), { b_ceiling: 1 })
    assert.deepEqual(values(BASE), { a_ceiling: 1, b_ceiling: 1, c_base: 1 })

    perf.report({ dir: out, floor: path.join(out, 'floor.json'), out: path.join(out, 'comment.md'), result: path.join(out, 'result.txt') })
    assert.equal(fs.readFileSync(path.join(out, 'result.txt'), 'utf8').trim(), 'pass')
    const body = fs.readFileSync(path.join(out, 'comment.md'), 'utf8')
    for (const sha of [CEILING, OTHER, BASE])
      assert.ok(body.includes(`ceiling \`${sha.slice(0, 7)}\``))
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

test('a missing ceiling measurement fails the merge', () => {
  const root = tempDir()
  try {
    const [first, second] = shards(root)
    for (const dir of [first, second])
      write(path.join(dir, `${HEAD}.json`), report([
        metric('a_ceiling', 1), metric('b_ceiling', 1), metric('c_base', 1),
      ]))
    write(path.join(first, `${BASE}.json`), report([metric('c_base', 1)]))
    write(path.join(first, `${CEILING}.json`), report([metric('b_ceiling', 1)]))
    write(path.join(second, `${OTHER}.json`), report([metric('b_ceiling', 1)]))
    assert.throws(() => merge(root, path.join(root, 'performance')), /a_ceiling/)
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

test('the base report survives when all its metrics belong to another shard', () => {
  const root = tempDir()
  const out = path.join(root, 'performance')
  try {
    const data = {
      version: 1,
      method: perf.METHOD,
      metrics: { a_ceiling: ceilingRecord(CEILING), b_ceiling: ceilingRecord(OTHER) },
    }
    const [first, second] = shards(root, data)
    for (const dir of [first, second])
      write(path.join(dir, `${HEAD}.json`), report([metric('a_ceiling', 1), metric('b_ceiling', 1)]))
    write(path.join(first, `${BASE}.json`), report([metric('b_ceiling', 1)]))
    write(path.join(first, `${CEILING}.json`), report([metric('a_ceiling', 1)]))
    write(path.join(second, `${OTHER}.json`), report([metric('b_ceiling', 1)]))

    assert.equal(merge(root, out), 4)
    const base = JSON.parse(fs.readFileSync(path.join(out, `${BASE}.json`), 'utf8'))
    assert.deepEqual(base.results.map((result) => result.name), ['b_ceiling'])
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

test('a workload missing from the base is still reported as new', () => {
  const root = tempDir()
  const out = path.join(root, 'performance')
  try {
    const [first, second] = shards(root)
    for (const dir of [first, second])
      write(path.join(dir, `${HEAD}.json`), report([
        metric('a_ceiling', 1), metric('b_ceiling', 1), metric('c_base', 1), metric('new_workload', 1),
      ]))
    write(path.join(first, `${BASE}.json`), report([metric('c_base', 1)]))
    write(path.join(first, `${CEILING}.json`), report([metric('a_ceiling', 1)]))
    write(path.join(second, `${OTHER}.json`), report([metric('b_ceiling', 1)]))

    assert.equal(merge(root, out), 4)
    perf.report({ dir: out, floor: path.join(out, 'floor.json'), out: path.join(out, 'comment.md'), result: path.join(out, 'result.txt') })
    assert.equal(fs.readFileSync(path.join(out, 'result.txt'), 'utf8').trim(), 'pass')
    assert.match(fs.readFileSync(path.join(out, 'comment.md'), 'utf8'), /new_workload[\s\S]*🆕/)
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

test('boundary metrics survive the shard merge into the PR report', () => {
  const root = tempDir()
  const out = path.join(root, 'performance')
  const boundary = [{
    workload: 'generated_hello_bar',
    compiled_functions: 2,
    native_functions: 0,
    helper_calls_per_event: { render: 0, click: 0 },
    compiled_bytes: 1200,
    handwritten_bytes: 900,
    parse: { ok: true },
  }]
  try {
    const [first, second] = shards(root)
    const headMetrics = [
      metric('a_ceiling', 1), metric('b_ceiling', 1), metric('c_base', 1),
    ]
    write(path.join(first, `${HEAD}.json`), { ...report(headMetrics), boundary })
    write(path.join(second, `${HEAD}.json`), { ...report(headMetrics), boundary })
    write(path.join(first, `${BASE}.json`), report([metric('c_base', 1)]))
    write(path.join(first, `${CEILING}.json`), report([metric('a_ceiling', 1)]))
    write(path.join(second, `${OTHER}.json`), report([metric('b_ceiling', 1)]))

    assert.equal(merge(root, out), 4)
    const merged = JSON.parse(fs.readFileSync(path.join(out, `${HEAD}.json`), 'utf8'))
    assert.deepEqual(merged.boundary, boundary)
    perf.report({ dir: out, floor: path.join(out, 'floor.json'), out: path.join(out, 'comment.md'), result: path.join(out, 'result.txt') })
    assert.match(fs.readFileSync(path.join(out, 'comment.md'), 'utf8'), /### Boundary metrics/)
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})
