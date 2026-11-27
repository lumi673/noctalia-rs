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
function floor() {
  return {
    version: 1,
    method: perf.METHOD,
    metrics: {
      a_ceiling: { sha: CEILING, method: perf.METHOD, medianMs: 1, worstMedianMs: 1, p95Ms: 1, worstP95Ms: 1, p99Ms: 1, worstP99Ms: 1, budgetMs: 10, updatedAt: NOW },
      b_ceiling: { sha: OTHER, method: perf.METHOD, medianMs: 1, worstMedianMs: 1, p95Ms: 1, worstP95Ms: 1, p99Ms: 1, worstP99Ms: 1, budgetMs: 10, updatedAt: NOW },
      c_base: { sha: BASE, method: perf.METHOD, medianMs: 1, worstMedianMs: 1, p95Ms: 1, worstP95Ms: 1, p99Ms: 1, worstP99Ms: 1, budgetMs: 10, updatedAt: NOW },
    },
  }
}

function shards(root) {
  const meta = { kind: 'pull_request', headSha: HEAD, baseSha: BASE, floor: {}, budgets: {} }
  const plan = perf.shardPlan(floor(), 2, HEAD, BASE)
  assert.deepEqual(plan[0].shas, [BASE, CEILING])
  assert.deepEqual(plan[1].shas, [OTHER])
  const dirs = []
  for (const shard of plan) {
    const dir = path.join(root, `performance-shard-${shard.index}`)
    fs.mkdirSync(dir, { recursive: true })
    write(path.join(dir, 'shard.json'), shard)
    write(path.join(dir, 'meta.json'), meta)
    write(path.join(dir, 'floor.json'), floor())
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
    assert.deepEqual(values(BASE), { a_ceiling: 1, c_base: 1 })

    perf.report({ dir: out, floor: path.join(out, 'floor.json'), out: path.join(out, 'comment.md'), result: path.join(out, 'result.txt') })
    assert.equal(fs.readFileSync(path.join(out, 'result.txt'), 'utf8').trim(), 'pass')
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
