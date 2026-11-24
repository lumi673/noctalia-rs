'use strict'

const fs = require('node:fs')
const path = require('node:path')

const VERSION = 1
const METHOD = 1
const MARKER = '<!-- performance-report -->'
const FAIL_MARGIN = 0.1
// Identical code swings by a couple of microseconds on the smallest workloads,
// so a difference below this never fails the gate on its own.
const MIN_MARGIN_MS = 0.002
const NAME = /^[a-z0-9][a-z0-9/_-]*$/u
const SHA = /^[0-9a-f]{40}$/u

function fail(message) {
  throw new Error(message)
}

function finite(value, label, minimum = Number.EPSILON) {
  if (typeof value !== 'number' || !Number.isFinite(value) || value < minimum)
    fail(`${label} must be a finite number >= ${minimum}`)
}

function object(value, label) {
  if (value === null || typeof value !== 'object' || Array.isArray(value))
    fail(`${label} must be an object`)
}

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'))
}

function emptyFloor() {
  return { version: VERSION, method: METHOD, metrics: {} }
}

function readFloor(file) {
  if (!file || !fs.existsSync(file)) return emptyFloor()
  return validateFloor(readJson(file), file)
}

function validateReport(value, label) {
  object(value, label)
  if (value.method !== METHOD) fail(`${label}.method must be ${METHOD}`)
  for (const key of ['runtime', 'os', 'cpu'])
    if (typeof value[key] !== 'string' || !value[key]) fail(`${label}.${key} must be a string`)
  if (!Array.isArray(value.results) || value.results.length === 0)
    fail(`${label}.results must be a non-empty array`)

  const seen = new Set()
  for (const result of value.results) {
    object(result, `${label}.result`)
    if (typeof result.name !== 'string' || !NAME.test(result.name))
      fail(`${label}.result.name is invalid`)
    if (seen.has(result.name)) fail(`${label} has a duplicate metric ${result.name}`)
    seen.add(result.name)
    if (typeof result.criterion !== 'string' || !result.criterion)
      fail(`${label}.${result.name}.criterion must be a non-empty string`)
    finite(result.budgetMs, `${label}.${result.name}.budgetMs`)
    if (!Number.isSafeInteger(result.samples) || result.samples < 1)
      fail(`${label}.${result.name}.samples must be a positive integer`)
    for (const key of ['medianMs', 'worstMedianMs', 'p95Ms', 'worstP95Ms', 'p99Ms', 'worstP99Ms'])
      finite(result[key], `${label}.${result.name}.${key}`)
    if (result.worstMedianMs < result.medianMs)
      fail(`${label}.${result.name}.worstMedianMs must not be below medianMs`)
  }
  return value
}

function validateFloor(value, label = 'floor') {
  object(value, label)
  if (value.version !== VERSION) fail(`${label}.version must be ${VERSION}`)
  if (value.method !== METHOD) fail(`${label}.method must be ${METHOD}`)
  object(value.metrics, `${label}.metrics`)
  for (const [name, record] of Object.entries(value.metrics)) {
    if (!NAME.test(name)) fail(`${label}.metrics has an invalid metric name`)
    object(record, `${label}.metrics.${name}`)
    if (typeof record.sha !== 'string' || !SHA.test(record.sha))
      fail(`${label}.metrics.${name}.sha must be a commit SHA`)
    finite(record.medianMs, `${label}.metrics.${name}.medianMs`)
    finite(record.worstMedianMs, `${label}.metrics.${name}.worstMedianMs`)
    if (record.worstMedianMs < record.medianMs)
      fail(`${label}.metrics.${name}.worstMedianMs must not be below medianMs`)
    finite(record.budgetMs, `${label}.metrics.${name}.budgetMs`)
    if (typeof record.updatedAt !== 'string' || !record.updatedAt)
      fail(`${label}.metrics.${name}.updatedAt must be a timestamp`)
  }
  return value
}

function validateMeta(value, label = 'meta.json') {
  object(value, label)
  if (value.kind !== 'pull_request' && value.kind !== 'push')
    fail(`${label}.kind must be pull_request or push`)
  if (typeof value.headSha !== 'string' || !SHA.test(value.headSha))
    fail(`${label}.headSha must be a commit SHA`)
  if (value.baseSha !== null && (typeof value.baseSha !== 'string' || !SHA.test(value.baseSha)))
    fail(`${label}.baseSha must be a commit SHA or null`)
  object(value.floor, `${label}.floor`)
  for (const [name, sha] of Object.entries(value.floor)) {
    if (!NAME.test(name)) fail(`${label}.floor has an invalid metric name`)
    if (typeof sha !== 'string' || !SHA.test(sha))
      fail(`${label}.floor.${name} must be a commit SHA`)
  }
  if (value.budgets !== undefined) {
    object(value.budgets, `${label}.budgets`)
    for (const [name, budget] of Object.entries(value.budgets)) {
      if (!NAME.test(name)) fail(`${label}.budgets has an invalid metric name`)
      finite(budget, `${label}.budgets.${name}`)
    }
  }
  return value
}

function mapping(floor) {
  return Object.fromEntries(Object.entries(floor.metrics).map(([name, record]) => [name, record.sha]))
}

function budgetMap(floor) {
  const budgets = {}
  for (const [name, record] of Object.entries(floor.metrics))
    budgets[name] = Math.min(budgets[name] ?? record.budgetMs, record.budgetMs)
  return budgets
}

function loadArtifacts(dir, shas) {
  const artifacts = new Map()
  for (const sha of new Set(shas.filter(Boolean))) {
    const report = validateReport(readJson(path.join(dir, `${sha}.json`)), sha)
    artifacts.set(sha, report)
  }
  return artifacts
}

function percent(value) {
  return `${value >= 0 ? '+' : ''}${value.toFixed(1)}%`
}

function milliseconds(value) {
  if (value === null || value === undefined) return 'n/a'
  if (value < 1) return `${(value * 1000).toFixed(1)} µs`
  if (value < 10) return `${value.toFixed(2)} ms`
  if (value < 100) return `${value.toFixed(1)} ms`
  return `${Math.round(value).toLocaleString('en-US')} ms`
}

function escapeText(value) {
  return String(value).replace(/\|/gu, '\\|').replace(/[\r\n]+/gu, ' ')
}

function regression(label, after, reference) {
  if (!reference) return null
  const bound = reference.worst
  const ownBound = after.worst
  const margin = Math.max(
    bound * FAIL_MARGIN,
    bound - reference.best,
    ownBound - after.best,
    MIN_MARGIN_MS
  )
  if (after.best > bound + margin)
    return {
      status: 'fail',
      note: `${label} ${milliseconds(after.best)} > reference worst ${milliseconds(bound)} by more than ${milliseconds(margin)}`,
    }
  if (after.best > bound + margin / 2)
    return {
      status: 'warn',
      note: `${label} ${milliseconds(after.best)} > reference worst ${milliseconds(bound)}`,
    }
  return null
}

function referenceFor(name, floor, base, meta, variants) {
  const record = floor.metrics[name]
  const sha = meta.floor[name]
  if (record && sha) {
    // The ceiling is re-measured in this session. Stored times are only used to
    // decide what to record, never to gate a PR on a different runner.
    const measured = variants.get(sha)?.results.find((entry) => entry.name === name)
    if (!measured) return null
    return {
      kind: 'ceiling',
      sha,
      best: measured.medianMs,
      worst: measured.worstMedianMs,
      p95: measured.p95Ms,
      worstP95: measured.worstP95Ms,
      p99: measured.p99Ms,
      worstP99: measured.worstP99Ms,
      budgetMs: record.budgetMs,
    }
  }
  const result = base?.results.find((entry) => entry.name === name)
  if (!result) return null
  return {
    kind: 'base',
    sha: meta.baseSha,
    best: result.medianMs,
    worst: result.worstMedianMs,
    p95: result.p95Ms,
    worstP95: result.worstP95Ms,
    p99: result.p99Ms,
    worstP99: result.worstP99Ms,
    budgetMs: result.budgetMs,
  }
}

function assess(name, after, reference, budgets) {
  const findings = []
  if (!after) return { status: 'fail', findings: [{ status: 'fail', note: 'not measured in this PR' }] }
  if (!after.criterion) findings.push({ status: 'fail', note: 'no criterion declared' })

  const trusted = budgets[name] ?? reference?.budgetMs ?? null
  const budget = trusted === null ? after.budgetMs : Math.min(trusted, after.budgetMs)
  if (trusted !== null && after.budgetMs > trusted)
    findings.push({
      status: 'fail',
      note: `budget raised from ${milliseconds(trusted)} to ${milliseconds(after.budgetMs)}`,
    })
  else if (after.medianMs > budget * (1 + FAIL_MARGIN))
    findings.push({
      status: reference && reference.best > budget ? 'warn' : 'fail',
      note: `median ${milliseconds(after.medianMs)} > budget ${milliseconds(budget)}`,
    })

  if (reference) {
    for (const finding of [
      regression('median', { best: after.medianMs, worst: after.worstMedianMs }, {
        best: reference.best,
        worst: reference.worst,
      }),
      regression('p95', { best: after.p95Ms, worst: after.worstP95Ms }, {
        best: reference.p95,
        worst: reference.worstP95,
      }),
      regression('p99', { best: after.p99Ms, worst: after.worstP99Ms }, {
        best: reference.p99,
        worst: reference.worstP99,
      }),
    ])
      if (finding) findings.push(finding)
  }

  const status = findings.some((finding) => finding.status === 'fail')
    ? 'fail'
    : findings.length > 0
      ? 'warn'
      : reference
        ? 'ok'
        : 'new'
  return { status, findings, budget }
}

function compare({ meta, floor, head, base, variants, budgets = {} }) {
  const artifacts = new Map(variants ?? [])
  if (!artifacts.has(meta.headSha)) artifacts.set(meta.headSha, head)
  if (base && !artifacts.has(meta.baseSha)) artifacts.set(meta.baseSha, base)
  const headByName = new Map(head.results.map((result) => [result.name, result]))
  const names = [
    ...new Set([...Object.keys(floor.metrics), ...(base?.results ?? []).map((r) => r.name), ...headByName.keys()]),
  ].sort()
  const rows = names.map((name) => {
    const after = headByName.get(name) ?? null
    const reference = referenceFor(name, floor, base, meta, artifacts)
    const assessment = assess(name, after, reference, budgets)
    const change = reference && after ? (after.medianMs / reference.best - 1) * 100 : null
    return {
      name,
      criterion: after?.criterion ?? '',
      budgetMs: after?.budgetMs ?? null,
      reference,
      after,
      change,
      status: assessment.status,
      findings: assessment.findings,
    }
  })
  return { rows, failed: rows.some((row) => row.status === 'fail') }
}

function markdown({ rows, failed }) {
  const icons = { ok: '✅', warn: '⚠️', fail: '❌', new: '🆕' }
  const failing = rows.filter((row) => row.status === 'fail').length
  const warnings = rows.filter((row) => row.status === 'warn').length
  const measured = rows.filter((row) => row.after).length
  const summary = failed
    ? `❌ ${failing} of ${rows.length} workloads failed.`
    : warnings
      ? `⚠️ No workload failed, but ${warnings} of ${measured} has a warning above the reference.`
      : `✅ All ${measured} workloads are within budget and at or below the reference.`

  const table = rows.map((row) => {
    const status = row.findings.length
      ? `${icons[row.status]} ${row.findings.map((finding) => finding.note).join('; ')}`
      : icons[row.status]
    const reference = row.reference
      ? `${row.reference.kind} \`${row.reference.sha.slice(0, 7)}\``
      : 'none'
    const tail = row.after
      ? `${milliseconds(row.after.p95Ms)} / ${milliseconds(row.after.p99Ms)}`
      : 'n/a'
    return `| \`${row.name}\`<br><sub>${escapeText(row.criterion)}</sub> | ${milliseconds(row.budgetMs)} | ${reference} ${row.reference ? milliseconds(row.reference.best) : ''} | ${row.after ? milliseconds(row.after.medianMs) : 'n/a'} | ${tail} | ${row.change === null ? 'n/a' : percent(row.change)} | ${status} |`
  })

  const ceilings = new Map()
  for (const row of rows)
    if (row.reference?.kind === 'ceiling')
      ceilings.set(row.reference.sha, (ceilings.get(row.reference.sha) ?? 0) + 1)
  const ceilingLine = ceilings.size
    ? `Ceilings: ${[...ceilings].map(([sha, count]) => `\`${sha.slice(0, 7)}\` (${count})`).join(', ')}.`
    : 'No ceiling is recorded yet, so workloads without one are compared with main.'

  return `${MARKER}
### Performance audit

${summary}

| Workload | Budget | Reference | This PR | p95 / p99 | vs reference | Status |
| :------- | -----: | :-------- | ------: | --------: | -----------: | :----- |
${table.join('\n')}

${ceilingLine}

All workloads run three times on a release build in the same CI session, and the best round is used. The reference is a ceiling commit re-measured in this session when one exists, otherwise main from this session. A workload fails when its median, p95, or p99 is more than 10% slower than the reference worst round, or by more than either side's spread between rounds, whichever is larger, with a floor of 2 µs. It warns at half that margin. It also fails when its median is more than 10% over budget while the reference is within budget. Budgets can only be tightened. Benchmarks use generated fixtures and never read a Noctalia config.`
}

function parseArgs(argv) {
  const args = {}
  for (let index = 0; index < argv.length; index += 2) {
    const key = argv[index]
    if (!key.startsWith('--')) fail(`Unknown argument ${key}`)
    args[key.slice(2)] = argv[index + 1]
  }
  return args
}

function runPlan(args) {
  if (!args.head || !SHA.test(args.head)) fail('--head must be a commit SHA')
  if (args.base && !SHA.test(args.base)) fail('--base must be a commit SHA')
  if (args.kind !== 'pull_request' && args.kind !== 'push') fail('--kind must be pull_request or push')
  if (!args.meta) fail('--meta is required')
  const floor = readFloor(args.floor)
  const meta = {
    kind: args.kind,
    headSha: args.head,
    baseSha: args.base || null,
    floor: mapping(floor),
    budgets: budgetMap(floor),
  }
  fs.writeFileSync(args.meta, `${JSON.stringify(meta, null, 2)}\n`)
  fs.writeFileSync(
    path.join(path.dirname(args.meta), 'floor.json'),
    `${JSON.stringify(floor, null, 2)}\n`
  )
  for (const sha of [...new Set([meta.baseSha, ...Object.values(meta.floor)])]
    .filter((sha) => sha && sha !== meta.headSha)
    .sort())
    console.log(sha)
}

function runReport(args) {
  if (!args.dir || !args.out || !args.result) fail('--dir, --out, and --result are required')
  const meta = validateMeta(readJson(path.join(args.dir, 'meta.json')))
  const floor = readFloor(args.floor)
  const variants = loadArtifacts(args.dir, [meta.headSha, meta.baseSha, ...Object.values(meta.floor)])
  const head = variants.get(meta.headSha)
  if (!head) fail(`No measurement for ${meta.headSha}`)
  const base = meta.baseSha ? variants.get(meta.baseSha) : null
  const compared = compare({
    meta,
    floor,
    head,
    base,
    variants,
    budgets: meta.budgets ?? {},
  })
  const body = markdown(compared)
  fs.writeFileSync(args.out, `${body}\n`)
  fs.writeFileSync(args.result, `${compared.failed ? 'fail' : 'pass'}\n`)
  console.log(body)
}

function runFloor(args) {
  if (!args.meta || !args.dir || !args.out) fail('--meta, --dir, and --out are required')
  const meta = validateMeta(readJson(args.meta))
  if (meta.kind !== 'push') fail('Ceilings only update from a main run.')
  const floor = readFloor(args.floor)
  const variants = loadArtifacts(args.dir, [meta.headSha, ...Object.values(meta.floor)])
  const head = variants.get(meta.headSha)
  if (!head) fail(`No measurement for ${meta.headSha}`)
  const now = args.now || new Date().toISOString()
  const next = { version: VERSION, method: METHOD, metrics: { ...floor.metrics } }
  const changes = []
  const recordFor = (result, budgetMs) => ({
    sha: meta.headSha,
    method: METHOD,
    medianMs: result.medianMs,
    worstMedianMs: result.worstMedianMs,
    p95Ms: result.p95Ms,
    worstP95Ms: result.worstP95Ms,
    p99Ms: result.p99Ms,
    worstP99Ms: result.worstP99Ms,
    budgetMs,
    updatedAt: now,
  })

  for (const result of head.results) {
    finite(result.worstMedianMs, `${result.name}.worstMedianMs`)
    if (result.worstMedianMs < result.medianMs)
      fail(`${result.name}.worstMedianMs must not be below medianMs`)

    const declared = Math.min((meta.budgets ?? {})[result.name] ?? result.budgetMs, result.budgetMs)
    const current = floor.metrics[result.name]
    if (!current) {
      next.metrics[result.name] = recordFor(result, declared)
      changes.push(result.name)
      continue
    }

    // Another run may have moved the ceiling while this one was queued.
    const measuredSha = meta.floor[result.name]
    if (measuredSha !== current.sha) continue
    const anchor = variants.get(measuredSha)?.results.find((entry) => entry.name === result.name)
    if (!anchor) continue

    const budgetMs = Math.min(declared, current.budgetMs)
    if (result.worstMedianMs >= anchor.medianMs) {
      if (budgetMs !== current.budgetMs) {
        next.metrics[result.name] = { ...current, budgetMs }
        changes.push(result.name)
      }
      continue
    }
    next.metrics[result.name] = recordFor(result, budgetMs)
    changes.push(result.name)
  }

  validateFloor(next, args.out)
  fs.writeFileSync(args.out, `${JSON.stringify(next, null, 2)}\n`)
  if (args.marker) fs.writeFileSync(args.marker, `${changes.length ? 'changed' : 'unchanged'}\n`)
  for (const name of changes) console.log(name)
}

function main() {
  const [command, ...rest] = process.argv.slice(2)
  const args = parseArgs(rest)
  if (command === 'plan') return runPlan(args)
  if (command === 'report') return runReport(args)
  if (command === 'floor') return runFloor(args)
  fail('Use plan, report, or floor.')
}

module.exports = {
  METHOD,
  MARKER,
  emptyFloor,
  validateReport,
  validateFloor,
  validateMeta,
  compare,
  markdown,
  updateFloor: runFloor,
  plan: runPlan,
  report: runReport,
}

if (require.main === module) {
  try {
    main()
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exit(1)
  }
}
