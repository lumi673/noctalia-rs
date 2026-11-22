'use strict'

const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const test = require('node:test')

const perf = require('./performance.cjs')

const HEAD = 'a'.repeat(40)
const BASE = 'b'.repeat(40)
const CEILING = 'c'.repeat(40)
const NOW = '2026-11-22T00:00:00.000Z'

function metric(name, values = {}) {
  const {
    medianMs = 1,
    worstMedianMs = medianMs,
    p95Ms = medianMs * 1.2,
    worstP95Ms = p95Ms,
    p99Ms = medianMs * 1.4,
    worstP99Ms = p99Ms,
    budgetMs = 10,
    criterion = 'test criterion',
  } = values
  return {
    name,
    criterion,
    budgetMs,
    samples: 60,
    warmup: 6,
    medianMs,
    worstMedianMs,
    p95Ms,
    worstP95Ms,
    p99Ms,
    worstP99Ms,
  }
}

function artifact(results) {
  return { method: perf.METHOD, runtime: 'rustc 1.99.1', os: 'linux', cpu: 'test', results }
}

function meta(overrides = {}) {
  return { kind: 'pull_request', headSha: HEAD, baseSha: BASE, floor: {}, budgets: {}, ...overrides }
}

test('an unchanged workload passes against main and produces the PR message', () => {
  const head = artifact([metric('spool_dispatch')])
  const base = artifact([metric('spool_dispatch', { medianMs: 1.05 })])
  const compared = perf.compare({ meta: meta(), floor: perf.emptyFloor(), head, base })
  assert.equal(compared.failed, false)
  assert.deepEqual(
    compared.rows.map((row) => row.status),
    ['ok']
  )
  const body = perf.markdown(compared)
  assert.match(body, /<!-- performance-report -->/)
  assert.match(body, /✅ All 1 workloads are within budget/)
})

test('a slowdown beyond the reference spread fails at the median, p95, and p99', () => {
  const base = artifact([
    metric('slow-median', { medianMs: 1, p95Ms: 1.2, p99Ms: 1.4 }),
    metric('slow-tail', { medianMs: 1, p95Ms: 1.2, p99Ms: 1.4 }),
    metric('slow-p99', { medianMs: 1, p95Ms: 1.2, p99Ms: 1.4 }),
  ])
  const head = artifact([
    metric('slow-median', { medianMs: 2, p95Ms: 1.2, p99Ms: 1.4 }),
    metric('slow-tail', { medianMs: 1, p95Ms: 2.4, p99Ms: 1.4 }),
    metric('slow-p99', { medianMs: 1, p95Ms: 1.2, p99Ms: 2.8 }),
  ])
  const compared = perf.compare({ meta: meta(), floor: perf.emptyFloor(), head, base })
  assert.equal(compared.failed, true)
  for (const row of compared.rows) assert.equal(row.status, 'fail')
  const body = perf.markdown(compared)
  assert.match(body, /median 2/)
  assert.match(body, /p95 2/)
  assert.match(body, /p99 2/)
})

test('a recorded workload missing from the PR fails', () => {
  const floor = {
    version: 1,
    method: perf.METHOD,
    metrics: {
      json_encode_rust: {
        sha: CEILING,
        method: perf.METHOD,
        medianMs: 0.3,
        worstMedianMs: 0.4,
        p95Ms: 0.4,
        worstP95Ms: 0.5,
        p99Ms: 0.5,
        worstP99Ms: 0.6,
        budgetMs: 1,
        updatedAt: NOW,
      },
    },
  }
  const compared = perf.compare({
    meta: meta({ floor: { json_encode_rust: CEILING } }),
    floor,
    head: artifact([metric('spool_dispatch')]),
    base: artifact([metric('spool_dispatch')]),
  })
  assert.equal(compared.failed, true)
  const missing = compared.rows.find((row) => row.name === 'json_encode_rust')
  assert.equal(missing.status, 'fail')
  assert.match(missing.findings[0].note, /not measured/)
})

test('a median over budget fails when the reference is within budget', () => {
  const base = artifact([metric('luau_vm_cache_reuse', { budgetMs: 1, medianMs: 0.5 })])
  const head = artifact([metric('luau_vm_cache_reuse', { budgetMs: 1, medianMs: 2 })])
  const compared = perf.compare({ meta: meta(), floor: perf.emptyFloor(), head, base })
  assert.equal(compared.failed, true)
  assert.match(compared.rows[0].findings[0].note, /over budget|budget 1\.00 ms/)
})

test('a ceiling is recorded once, tightens budgets, and only moves on improvement', () => {
  const root = path.join(process.cwd(), '.tmp')
  fs.mkdirSync(root, { recursive: true })
  const dir = fs.mkdtempSync(path.join(root, 'perf-test-'))
  try {
    const floorPath = path.join(dir, 'floor.json')
    const metaPath = path.join(dir, 'meta.json')
    const rawDir = path.join(dir, 'raw')
    fs.mkdirSync(rawDir)
    const writeRun = (sha, values) =>
      fs.writeFileSync(
        path.join(rawDir, `${sha}.json`),
        JSON.stringify(artifact([metric('spool_dispatch', values)])),
      )
    const writeMeta = (headSha, ceilingSha = null) =>
      fs.writeFileSync(
        metaPath,
        JSON.stringify({
          kind: 'push',
          headSha,
          baseSha: null,
          floor: ceilingSha ? { spool_dispatch: ceilingSha } : {},
          budgets: {},
        }),
      )
    const update = (headSha, ceilingSha = null) => {
      writeMeta(headSha, ceilingSha)
      perf.updateFloor({
        floor: floorPath,
        meta: metaPath,
        dir: rawDir,
        out: floorPath,
        marker: path.join(dir, 'changed.txt'),
        now: NOW,
      })
      return JSON.parse(fs.readFileSync(floorPath, 'utf8'))
    }

    writeRun(HEAD, { medianMs: 10, worstMedianMs: 11, budgetMs: 20 })
    let floor = update(HEAD)
    assert.equal(floor.metrics.spool_dispatch.medianMs, 10)
    assert.equal(floor.metrics.spool_dispatch.budgetMs, 20)

    const NEXT = 'd'.repeat(40)
    writeRun(NEXT, { medianMs: 12, worstMedianMs: 13, budgetMs: 10 })
    floor = update(NEXT, HEAD)
    assert.equal(floor.metrics.spool_dispatch.medianMs, 10)
    assert.equal(floor.metrics.spool_dispatch.budgetMs, 10)

    const BETTER = 'e'.repeat(40)
    writeRun(BETTER, { medianMs: 8, worstMedianMs: 9, budgetMs: 10 })
    floor = update(BETTER, HEAD)
    assert.equal(floor.metrics.spool_dispatch.sha, BETTER)
    assert.equal(floor.metrics.spool_dispatch.medianMs, 8)
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

test('plan packs the ceiling into the artifact and the report uses it', () => {
  const root = path.join(process.cwd(), '.tmp')
  fs.mkdirSync(root, { recursive: true })
  const dir = fs.mkdtempSync(path.join(root, 'perf-test-'))
  try {
    const floorPath = path.join(dir, 'source-floor.json')
    fs.writeFileSync(
      floorPath,
      JSON.stringify({
        version: 1,
        method: perf.METHOD,
        metrics: {
          spool_dispatch: {
            sha: CEILING,
            method: perf.METHOD,
            medianMs: 1,
            worstMedianMs: 1.1,
            p95Ms: 1.2,
            worstP95Ms: 1.3,
            p99Ms: 1.4,
            worstP99Ms: 1.5,
            budgetMs: 10,
            updatedAt: NOW,
          },
        },
      })
    )
    const metaPath = path.join(dir, 'meta.json')
    perf.plan({ floor: floorPath, head: HEAD, base: BASE, kind: 'pull_request', meta: metaPath })
    assert.ok(fs.existsSync(path.join(dir, 'floor.json')))
    for (const sha of [HEAD, BASE, CEILING])
      fs.writeFileSync(path.join(dir, `${sha}.json`), JSON.stringify(artifact([metric('spool_dispatch')])))
    const out = path.join(dir, 'comment.md')
    const result = path.join(dir, 'result.txt')
    perf.report({ dir, floor: path.join(dir, 'floor.json'), out, result })
    assert.equal(fs.readFileSync(result, 'utf8').trim(), 'pass')
    assert.match(fs.readFileSync(out, 'utf8'), /ceiling/)
    fs.unlinkSync(path.join(dir, `${CEILING}.json`))
    assert.throws(
      () => perf.report({ dir, floor: path.join(dir, 'floor.json'), out, result }),
      /ENOENT/
    )
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

test('a ceiling gate uses the artifact measured in this session, not the stored time', () => {
  const floor = {
    version: 1,
    method: perf.METHOD,
    metrics: {
      spool_dispatch: {
        sha: CEILING,
        method: perf.METHOD,
        medianMs: 1,
        worstMedianMs: 1.1,
        p95Ms: 1.2,
        worstP95Ms: 1.3,
        p99Ms: 1.4,
        worstP99Ms: 1.5,
        budgetMs: 10,
        updatedAt: NOW,
      },
    },
  }
  const values = { medianMs: 1.5, worstMedianMs: 1.6, p95Ms: 1.8, p99Ms: 2 }
  const variants = new Map([[CEILING, artifact([metric('spool_dispatch', values)])]])
  const compared = perf.compare({
    meta: meta({ floor: { spool_dispatch: CEILING } }),
    floor,
    head: artifact([metric('spool_dispatch', values)]),
    base: null,
    variants,
  })
  assert.equal(compared.failed, false)
  assert.equal(compared.rows[0].status, 'ok')
})

test('a ceiling only moves from its same-session anchor and matching floor SHA', () => {
  const root = path.join(process.cwd(), '.tmp')
  fs.mkdirSync(root, { recursive: true })
  const dir = fs.mkdtempSync(path.join(root, 'perf-test-'))
  try {
    const floorPath = path.join(dir, 'floor.json')
    const metaPath = path.join(dir, 'meta.json')
    const rawDir = path.join(dir, 'raw')
    fs.mkdirSync(rawDir)
    const writeFloor = () =>
      fs.writeFileSync(
        floorPath,
        JSON.stringify({
          version: 1,
          method: perf.METHOD,
          metrics: {
            spool_dispatch: {
              sha: CEILING,
              method: perf.METHOD,
              medianMs: 10,
              worstMedianMs: 11,
              p95Ms: 12,
              worstP95Ms: 13,
              p99Ms: 14,
              worstP99Ms: 15,
              budgetMs: 10,
              updatedAt: NOW,
            },
          },
        }),
      )
    const update = (ceilingSha) => {
      fs.writeFileSync(
        metaPath,
        JSON.stringify({
          kind: 'push',
          headSha: HEAD,
          baseSha: null,
          floor: { spool_dispatch: ceilingSha },
          budgets: {},
        }),
      )
      perf.updateFloor({ floor: floorPath, meta: metaPath, dir: rawDir, out: floorPath, now: NOW })
      return JSON.parse(fs.readFileSync(floorPath, 'utf8'))
    }

    writeFloor()
    fs.writeFileSync(
      path.join(rawDir, `${CEILING}.json`),
      JSON.stringify(artifact([metric('spool_dispatch', { medianMs: 50, worstMedianMs: 55 })])),
    )
    fs.writeFileSync(
      path.join(rawDir, `${HEAD}.json`),
      JSON.stringify(artifact([metric('spool_dispatch', { medianMs: 30, worstMedianMs: 31 })])),
    )
    let floor = update(CEILING)
    assert.equal(floor.metrics.spool_dispatch.medianMs, 30)

    writeFloor()
    fs.writeFileSync(
      path.join(rawDir, `${HEAD}.json`),
      JSON.stringify(artifact([metric('spool_dispatch', { medianMs: 1, worstMedianMs: 2 })])),
    )
    floor = update(HEAD)
    assert.equal(floor.metrics.spool_dispatch.medianMs, 10)
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})
