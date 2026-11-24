'use strict'

const fs = require('node:fs')
const path = require('node:path')

const { METHOD } = require('./performance.cjs')

const RUN = /^([0-9a-f]{40})-(\d+)\.json$/u

const WORKLOADS = {
  fresh_process_dispatch: ['dispatch through a newly started helper process', 20],
  spool_dispatch: ['dispatch through a persistent spool', 10],
  luau_vm_creation: ['create a Luau VM for a cold directory', 5],
  luau_vm_cache_reuse: ['reuse a cached Luau VM', 1],
  luau_runtime_dispatch: ['dispatch an on-demand Luau runtime', 2],
  json_roundtrip_luau: ['encode and decode JSON in Luau', 2],
  json_roundtrip_rust: ['encode and decode JSON in Rust', 1],
  bridge_snapshot_reads: ['read the generated snapshot through the bridge', 5],
  json_encode_rust: ['encode a nested JSON value in Rust', 1],
  json_decode_rust: ['decode a nested JSON value in Rust', 1],
  snapshot_reads_rust: ['read config, state, and host values in Rust', 1],
  snapshot_reads_luau: ['read config, state, and host values in Luau', 2],
  state_ops_rust: ['read, set, and watch state in Rust', 1],
  state_ops_luau: ['read, set, and watch state in Luau', 5],
  ui_tree_rust: ['build and render a UI tree in Rust', 1],
  ui_tree_luau: ['build and render a UI tree in Luau', 5],
  callback_dispatch_rust: ['dispatch a callback handler in Rust', 1],
  callback_dispatch_luau: ['dispatch a callback handler in Luau', 5],
  service_update_rust: ['run a plugin-style service update in Rust', 2],
  service_update_luau: ['run a plugin-style service update in Luau', 10],
  plugin_case_convert_luau: ['normalize and format text with the case-convert Luau module', 2],
  plugin_case_convert_rust: ['normalize and format text with the case-convert Rust module', 0.5],
  plugin_color_picker_luau: ['convert and format colors with the color picker Luau module', 3],
  plugin_color_picker_rust: ['convert and format colors with the color picker Rust module', 2],
  plugin_vietnamese_lunar_calendar_luau: ['convert solar dates with the Vietnamese lunar Luau module', 60],
  plugin_vietnamese_lunar_calendar_rust: ['convert solar dates with the Vietnamese lunar Rust module', 10],
  plugin_als_brightness_luau: ['compile and sample a brightness curve in Luau', 2],
  plugin_als_brightness_rust: ['compile and sample a brightness curve in Rust', 0.5],
  plugin_mihomo_control_luau: ['normalize a proxy server store in Luau', 8],
  plugin_mihomo_control_rust: ['normalize a proxy server store in Rust', 4],
  plugin_file_search_luau: ['sort disk usage slices with the file-search Luau module', 0.2],
  plugin_file_search_rust: ['sort disk usage slices with the file-search Rust module', 0.1],
  generated_hello_bar_compiled: ['render a compiled hello bar Luau entry', 0.1],
  generated_hello_bar_handwritten: ['render a hand-written hello bar Luau entry', 0.1],
  generated_ui_panel_compiled: ['render a compiled UI panel Luau entry', 0.1],
  generated_ui_panel_handwritten: ['render a hand-written UI panel Luau entry', 0.1],
  generated_event_compiled: ['run a compiled service event Luau entry', 0.1],
  generated_event_handwritten: ['run a hand-written service event Luau entry', 0.1],
  codegen_generate: ['generate a plugin from a manifest', 10],
  build_incremental_release: ['build the generated fixture after one source change', 120000],
}

function fail(message) {
  throw new Error(message)
}

function microsToMs(value) {
  return value / 1000
}

function toGateReport(raw) {
  if (!raw?.metadata || !Array.isArray(raw.results) || raw.results.length === 0)
    fail('benchmark report has no results')
  const results = raw.results.map((result) => {
    const workload = WORKLOADS[result.surface]
    if (!workload) fail(`no criterion or budget for ${result.surface}`)
    for (const key of ['median_us', 'p95_us', 'p99_us'])
      if (typeof result[key] !== 'number' || !Number.isFinite(result[key]))
        fail(`${result.surface}.${key} must be a finite number`)
    const [criterion, budgetMs] = workload
    return {
      name: result.surface,
      criterion,
      budgetMs,
      samples: result.samples,
      warmup: result.warmup,
      medianMs: microsToMs(result.median_us),
      worstMedianMs: microsToMs(result.median_us),
      p95Ms: microsToMs(result.p95_us),
      worstP95Ms: microsToMs(result.p95_us),
      p99Ms: microsToMs(result.p99_us),
      worstP99Ms: microsToMs(result.p99_us),
    }
  })
  return {
    method: METHOD,
    runtime: raw.metadata.rustc,
    os: raw.metadata.os,
    cpu: raw.metadata.cpu_model,
    results,
  }
}

function combine(reports) {
  const first = reports[0]
  const expected = first.results.map((result) => result.name).sort()
  for (const report of reports) {
    const names = report.results.map((result) => result.name).sort()
    if (names.length !== expected.length || names.some((name, index) => name !== expected[index]))
      fail('benchmark runs do not share the same workloads')
  }
  const results = first.results.map((result) => {
    const values = reports.map((report) => {
      const value = report.results.find((entry) => entry.name === result.name)
      if (!value) fail(`a run is missing ${result.name}`)
      if (value.budgetMs !== result.budgetMs) fail(`${result.name} changed budget between runs`)
      return value
    })
    const medians = values.map((value) => value.medianMs)
    const p95s = values.map((value) => value.p95Ms)
    const p99s = values.map((value) => value.p99Ms)
    const best = Math.min(...medians)
    const worst = Math.max(...medians)
    return {
      ...result,
      medianMs: best,
      worstMedianMs: worst,
      p95Ms: Math.min(...p95s),
      worstP95Ms: Math.max(...p95s),
      p99Ms: Math.min(...p99s),
      worstP99Ms: Math.max(...p99s),
      noisePercent: (worst / best - 1) * 100,
    }
  })
  return { method: first.method, runtime: first.runtime, os: first.os, cpu: first.cpu, results }
}

function main() {
  const [input = 'performance/raw', output = 'performance', roundsArg = '3'] = process.argv.slice(2)
  const rounds = Number(roundsArg)
  if (!Number.isSafeInteger(rounds) || rounds < 1) fail('--rounds must be a positive integer')

  const variants = new Map()
  for (const file of fs.readdirSync(input)) {
    const match = RUN.exec(file)
    if (!match) continue
    const round = Number(match[2])
    if (round < 1 || round > rounds) fail(`${file} has a round outside 1..${rounds}`)
    const runs = variants.get(match[1]) ?? new Map()
    runs.set(round, path.join(input, file))
    variants.set(match[1], runs)
  }
  if (variants.size === 0) fail(`no benchmark runs found in ${input}`)

  fs.mkdirSync(output, { recursive: true })
  for (const [sha, runs] of variants) {
    if (runs.size !== rounds) fail(`${sha} has ${runs.size} of ${rounds} runs`)
    const reports = []
    for (let round = 1; round <= rounds; round++) {
      const file = runs.get(round)
      if (!file) fail(`${sha} has no run ${round}`)
      reports.push(toGateReport(JSON.parse(fs.readFileSync(file, 'utf8'))))
    }
    const combined = combine(reports)
    fs.writeFileSync(path.join(output, `${sha}.json`), JSON.stringify(combined))
  }
  console.log(`Wrote best of ${rounds} runs for ${variants.size} commits.`)
}

module.exports = { WORKLOADS, toGateReport, combine }

if (require.main === module) {
  try {
    main()
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error))
    process.exit(1)
  }
}
