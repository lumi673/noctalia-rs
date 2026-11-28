'use strict'

const assert = require('node:assert/strict')
const test = require('node:test')

const { combine, toGateReport, WORKLOADS } = require('./performance-aggregate.cjs')

function raw(medianUs, p95Us = medianUs * 1.2, p99Us = medianUs * 1.4) {
  return {
    metadata: {
      rustc: 'rustc 1.99.1',
      os: 'Linux 6.12 x86_64',
      cpu_model: 'test cpu',
    },
    results: [
      {
        surface: 'spool_dispatch',
        samples: 60,
        warmup: 6,
        median_us: medianUs,
        p95_us: p95Us,
        p99_us: p99Us,
        min_us: medianUs * 0.9,
        max_us: p99Us * 1.1,
      },
    ],
  }
}

test('the aggregator converts microseconds, attaches the criterion, and keeps the budget', () => {
  const report = toGateReport(raw(1500, 1800, 2100))
  assert.equal(report.method, 1)
  assert.equal(report.runtime, 'rustc 1.99.1')
  assert.deepEqual(report.results[0], {
    name: 'spool_dispatch',
    criterion: WORKLOADS.spool_dispatch[0],
    budgetMs: 10,
    samples: 60,
    warmup: 6,
    medianMs: 1.5,
    worstMedianMs: 1.5,
    p95Ms: 1.8,
    worstP95Ms: 1.8,
    p99Ms: 2.1,
    worstP99Ms: 2.1,
  })
})

test('three rounds produce the best median and the worst spread', () => {
  const combined = combine([
    toGateReport(raw(1500)),
    toGateReport(raw(1200)),
    toGateReport(raw(1800)),
  ])
  const result = combined.results[0]
  assert.equal(result.medianMs, 1.2)
  assert.equal(result.worstMedianMs, 1.8)
  assert.equal(result.noisePercent, 50)
})

test('an unknown workload has no silent budget', () => {
  const report = raw(1000)
  report.results[0].surface = 'unknown'
  assert.throws(() => toGateReport(report), /no criterion or budget/)
})

test('boundary metrics travel through the gate report', () => {
  const boundary = [{
    workload: 'generated_hello_bar',
    compiled_functions: 2,
    native_functions: 0,
    helper_calls_per_event: { render: 0 },
    compiled_bytes: 1200,
    handwritten_bytes: 900,
    parse: { ok: true },
  }]
  const first = { ...raw(1500), boundary }
  const second = { ...raw(1200), boundary }
  assert.deepEqual(toGateReport(first).boundary, boundary)
  const combined = combine([
    toGateReport(first),
    toGateReport(second),
    toGateReport({ ...raw(1800), boundary }),
  ])
  assert.deepEqual(combined.boundary, boundary)

  assert.throws(
    () => combine([
      toGateReport(first),
      toGateReport({ ...raw(1200), boundary: [] }),
    ]),
    /boundary metrics/
  )
})
