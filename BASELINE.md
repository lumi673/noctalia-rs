# noctalia-rs performance baseline

Branch `bench/api57-baseline`, commit `40c0111` (benchmark changes are local and uncommitted). This is the pre-parity baseline for comparing native Rust handlers with equivalent raw Luau.

Each pair runs the same algorithm and API call order on both sides. The Luau side is executed through `noctalia_rs_luau::dispatch` with a warm cached VM, so dispatch, snapshot update, callback lookup, and op collection are included. The Rust side calls the equivalent handler directly. Contexts and payloads are built from deterministic fixtures; no user Noctalia config is read. Speedup is `Luau median / Rust median`.

## Community plugin baseline

Branch `bench/plugin-speedup`, based on `feat/api57-full-parity`. Head `ae66290`; benchmark changes are local and uncommitted. Raw report: `plugin-baseline.json`.

The original Luau modules and the rewritten Rust modules are vendored unchanged in logic under `crates/bench/src/plugins/`. Luau sources came from `/home/user/.local/share/Trash/files/noctalia-rs/.tmp/community-plugins/`, Rust sources from `/home/user/projects/community-plugins-rs/`. The only edits to vendored Rust files are module paths (`crate::` to `super::`), the bundled translation path, and rustfmt normalization. No library behavior and no community checkout changed.

Each pair uses one deterministic fixture. The Luau side calls the original pure function from a wrapper through `noctalia_rs_luau::dispatch` with a warm cached VM. The Rust side calls the rewritten function directly. Both sides emit the same op sequence through `noctalia.log`. The harness canonicalizes both op outputs as JSON and fails before timing if they differ. Mihomo groups are sorted by name before emission because the original iterates Lua `pairs`.

### Exact commands

```sh
nix develop --command cargo run --release -p noctalia-bench --offline -- \
  --samples 200 --warmup 20 --report plugin-baseline.json

node .github/scripts/performance-aggregate.test.cjs
```

### Machine metadata

| Field | Value |
| :-- | :-- |
| Timestamp | 2026-11-23T18:45:10Z |
| OS | Linux 7.3.5 x86\_64 |
| CPU | AMD Ryzen 7 8845HS w/ Radeon 780M Graphics |
| Logical CPUs | 16 |
| Max frequency | 3,801,000 kHz |
| Memory | 96,582,428 KiB |
| rustc | 1.99.1 (58cf8f9c3 2026-10-13) |
| cargo | 1.99.1 (5f94df478 2026-08-27) |
| Profile | release, fat LTO, codegen-units 1 |
| Samples | 200 per surface, 20 warmup |
| Percentiles | nearest rank |
| Timer | `std::time::Instant` |

### Plugin results

Times are microseconds. Speedup is `Luau median / Rust median`.

| Workload | Function | Rust median | Rust p95 | Rust p99 | Luau median | Luau p95 | Luau p99 | Speedup median | Speedup p95 | Speedup p99 |
| :-- | :-- | --: | --: | --: | --: | --: | --: | --: | --: | --: |
| plugin\_case\_convert | `cases.convertAll` over 18 conversions | 24.136 | 33.032 | 38.011 | 224.361 | 366.937 | 863.909 | 9.30x | 11.11x | 22.73x |
| plugin\_color\_picker | RGB/HSL/HEX conversions and formatting over 128 colors | 191.043 | 248.877 | 325.330 | 401.272 | 502.141 | 710.421 | 2.10x | 2.02x | 2.18x |
| plugin\_vietnamese\_lunar\_calendar | `solarToLunar` over 730 dates | 1,442.328 | 1,945.836 | 2,870.870 | 8,868.475 | 11,087.497 | 11,784.704 | 6.15x | 5.70x | 4.10x |
| plugin\_als\_brightness | `compile` plus 256 `eval` samples | 29.335 | 40.617 | 45.615 | 241.267 | 290.694 | 341.710 | 8.22x | 7.16x | 7.49x |
| plugin\_mihomo\_control | `normalizeServersStore` over 400 servers and 40 group orders | 849.072 | 1,087.207 | 1,334.782 | 1,837.538 | 2,792.793 | 3,486.273 | 2.16x | 2.57x | 2.61x |
| plugin\_file\_search | `slices`, `entries`, and `commonDir` over a nested disk-usage index | 7.534 | 8.125 | 12.424 | 23.714 | 32.701 | 36.158 | 3.15x | 4.02x | 2.91x |

All six clear 2x at the median, p95, and p99. None clears 10x at the median; `plugin_case_convert` reaches 11.11x at p95 and 22.73x at p99. The closest are `plugin_case_convert` at 9.30x and `plugin_als_brightness` at 8.22x. The three lowest ratios are limited by the work itself rather than dispatch. Color and mihomo build many small JSON values on both sides, so allocation dominates. File search sorts a small index where fixed dispatch cost is a large share. The lunar calendar calls native Luau math for every date, which narrows the gap despite a large fixture.

### Excluded plugins

`token-monitor` was tried and skipped. Its Rust rewrites diverge from the original output, and the community checkout was left untouched:

- `analytics.normalize` returns `models: []` and `sessions: []` for unavailable periods, while the Luau original returns empty tables, which encode as `{}`.
- `heatmap.build` returns `value: null` for future and outside cells, while the Luau original omits the `value` key.

Other candidates were not selected because their heavy work is outside the plugin algorithm: `calculator-plus` delegates evaluation to the external `qalc` process, `mathsym` delegates search to `fzf` in `search.sh`, and `calculator`, `bitdjinn`, `arch-updater`, and `file-search` launcher paths are dominated by host I/O or UI construction.

## Exact commands

```sh
nix develop --command cargo run --release -p noctalia-bench -- \
  --samples 200 --warmup 20 --report baseline.json

sha=$(git rev-parse HEAD)
mkdir -p performance/raw
for run in 1 2 3; do
  nix develop --command cargo run --release -p noctalia-bench -- \
    --samples 60 --warmup 6 --build-samples 2 --build-warmup 1 \
    --report "performance/raw/$sha-$run.json"
done
node .github/scripts/performance-aggregate.cjs performance/raw performance 3
```

## Machine metadata

| Field | Value |
| :-- | :-- |
| Timestamp | 2026-11-23T14:53:59Z |
| OS | Linux 7.3.5 x86\_64 |
| CPU | AMD Ryzen 7 8845HS w/ Radeon 780M Graphics |
| Logical CPUs | 16 |
| Max frequency | 3,801,000 kHz |
| Memory | 96,582,428 KiB |
| rustc | 1.99.1 (58cf8f9c3 2026-10-13) |
| cargo | 1.99.1 (5f94df478 2026-08-27) |
| Profile | release, fat LTO, codegen-units 1 |
| Samples | 200 per surface, 20 warmup |
| Percentiles | nearest rank |
| Timer | `std::time::Instant` |

## Matched workload results

Times are microseconds. Raw report: `baseline.json`.

| Workload | Rust median | Rust p95 | Rust p99 | Luau median | Luau p95 | Luau p99 | Speedup | 3x met |
| :-- | --: | --: | --: | --: | --: | --: | --: | :-- |
| json\_roundtrip | 0.631 | 0.671 | 0.702 | 5.760 | 8.015 | 8.707 | 9.13x | yes |
| snapshot\_reads | 0.100 | 0.120 | 0.130 | 3.536 | 3.897 | 6.001 | 35.36x | yes |
| state\_ops | 0.270 | 0.281 | 0.291 | 6.553 | 8.696 | 9.969 | 24.27x | yes |
| ui\_tree | 0.651 | 0.721 | 0.742 | 17.012 | 23.614 | 26.019 | 26.13x | yes |
| callback\_dispatch | 0.060 | 0.061 | 0.061 | 1.914 | 1.993 | 2.054 | 31.90x | yes |
| service\_update | 0.231 | 0.261 | 0.271 | 9.077 | 12.002 | 14.828 | 39.29x | yes |

Speedup at each percentile:

| Workload | Median | p95 | p99 |
| :-- | --: | --: | --: |
| json\_roundtrip | 9.13x | 11.94x | 12.40x |
| snapshot\_reads | 35.36x | 32.48x | 46.16x |
| state\_ops | 24.27x | 30.95x | 34.26x |
| ui\_tree | 26.13x | 32.75x | 35.07x |
| callback\_dispatch | 31.90x | 32.67x | 33.67x |
| service\_update | 39.29x | 45.98x | 54.72x |

All six pairs exceed the 3x aspiration at the median, p95, and p99.

## CI-style rounds

Three rounds used the CI sample plan (60 samples, 6 warmup, 2 build samples, 1 build warmup). CI publishes the raw runs and the best-of-three aggregate under `performance/`; they are not committed.

| Workload | Speedup, round 1 | Round 2 | Round 3 |
| :-- | --: | --: | --: |
| json\_roundtrip | 9.32x | 9.03x | 9.32x |
| snapshot\_reads | 35.67x | 35.82x | 36.07x |
| state\_ops | 24.58x | 24.42x | 25.47x |
| ui\_tree | 27.40x | 31.27x | 27.69x |
| callback\_dispatch | 32.90x | 32.07x | 32.23x |
| service\_update | 40.60x | 41.53x | 39.62x |

## Slowest workloads relative to 3x

No pair misses the goal. Closest to it:

1. `json_roundtrip`, 9.13x, still 3.04 times the goal. It is dominated by `json.encode` and `json.decode` through the Luau globals.
2. `state_ops`, 24.27x.
3. `ui_tree`, 26.13x. It also has the highest absolute Luau median at 17.012 us.
4. `callback_dispatch`, 31.90x, with the lowest absolute times on both sides.
5. `snapshot_reads`, 35.36x.
6. `service_update`, 39.29x, the widest gap but also the largest matched algorithm.

By absolute Luau median the slowest are `ui_tree` (17.012 us), `service_update` (9.077 us), `state_ops` (6.553 us), and `json_roundtrip` (5.760 us).

Sanity check against the main-session numbers: this run measured `json_encode_rust` at 0.285 us and `luau_runtime_dispatch` at 4.328 us, in line with the reported 0.291 us and 4.238 us.

## Post-parity verification

After API 57 parity landed, the same release harness was run again with `nix develop --command cargo run --release -p noctalia-bench -- --samples 200 --warmup 20`. The matched pairs stayed above the 3x goal. This run is on a busier machine than the earlier table, so most medians are higher on both sides; `snapshot_reads` improved because a host read no longer stats the spool directory on every call.

| Workload | Rust median | Luau median | Speedup | 3x met |
| :-- | --: | --: | --: | :-- |
| json\_roundtrip | 0.641 | 8.636 | 13.47x | yes |
| snapshot\_reads | 0.126 | 3.717 | 29.62x | yes |
| state\_ops | 0.281 | 9.413 | 33.50x | yes |
| ui\_tree | 0.637 | 19.997 | 31.42x | yes |
| callback\_dispatch | 0.060 | 2.019 | 33.65x | yes |
| service\_update | 0.371 | 12.724 | 34.30x | yes |

The parity work therefore kept the baseline and all six matched workloads remain above 3x.

## Compiled Luau entries

The compiler turns one Rust handler into a complete Luau entry. This section
measures that output against hand-written Luau doing the same work.

Each fixture uses one deterministic `Context`. The compiled side is generated
by `noctalia-rs-compiler` from the Rust handler in `crates/bench/src/generated.rs`.
The hand-written side is the equivalent plugin a person would write, including
the same load render, state watch, and `panel.setWantsSecondTicks` startup.
Both run through `noctalia_rs_luau::dispatch` with a warm cached VM. Op output is
compared before timing, including the startup render and the measured event,
and must match after filtering startup `host_call` ops that the Rust handler
does not perform.

Run three rounds of 300 samples. Speedup is `hand-written / compiled`, so 1.0
is parity. Times are microseconds. The ratio column is the median of the three
round ratios, so it does not always equal the quotient of the two medians below
it when the host is noisy.

| Workload | Compiled medians r1/r2/r3 | Hand medians r1/r2/r3 | Speedups r1/r2/r3 | Median speedup | Source bytes compiled / hand-written |
| :-- | :-- | :-- | :-- | --: | :-- |
| generated\_hello\_bar | 1.934 / 1.894 / 1.893 | 2.054 / 2.004 / 1.953 | 1.01 / 1.01 / 0.98 | 1.01x | 2405 / 467 |
| generated\_ui\_panel | 9.889 / 10.008 / 9.969 | 17.021 / 16.551 / 16.390 | 1.72 / 1.65 / 1.64 | 1.64x | 2938 / 637 |
| generated\_event | 3.637 / 3.702 / 3.606 | 3.651 / 4.348 / 4.078 | 1.00 / 0.98 / 0.99 | 0.99x | 3174 / 447 |

The UI tree clears parity in every round. The two small fixtures sit on the
same roughly 2 us dispatch floor, so their ratios move with host load. Treat
`generated_hello_bar` and `generated_event` as parity within measurement noise.
The compiled scripts carry every callback a host may invoke, so they are larger
than the hand-written mirrors here; the win on UI trees comes from emitting
node tables directly instead of calling `ui.row` and `ui.label`.

The 3x target is not measured against hand-written Luau. Both sides pay the
same dispatch floor, so 3x is not reachable there. It is measured against the
bridge path this replaces. This run includes the idle helper CPU change from
`main`, so `spool_dispatch` is 77 / 77 / 80 us across the three rounds, with a
median of 77 us. Compiled renders are 1.9 us to 10.0 us, or 8x to 41x faster
than the bridge.

### Correctness fixes behind this run

The first compiler pass had four defects found by audit. This run includes the
fixes:

- a const-folded `if` with an early `return` now wraps the taken branch in
  `do ... end`, so the following statements stay parseable;
- launcher entries no longer emit an invented load render, and their state
  rerender passes `last_query` instead of dropping the query text;
- `unwrap_or` on a bool uses `bool_or`, so `false` is not replaced by the
  default;
- `config_str` and `payload_str` coerce numbers the way `Context` does. String
  config keys declared in `noctalia.toml` skip that coercion.
- `payload_str` keys that match a callback parameter are passed as parameters,
  so an `activate` handler takes `id` directly instead of building and reading
  a payload table.

### Template output

The same release build emitted the template before and after opting both
entries into `luau = "src/main.rs"`. Compiled entries now declare every host
callback, which is why the generated script is close in size to the bridge stub
even though it has no helper or IPC.

| File | Bridge stub | Compiled |
| :-- | --: | --: |
| `widget.luau` | 2,255 B | 380 B |
| `service.luau` | 2,175 B | 270 B |
| `lib/bridge.luau` | 437 lines | not emitted |
| total lines in the plugin directory | 545 | 61 |

### Exact command

```sh
for i in 1 2 3; do
  nix develop --command cargo run --release -p noctalia-bench --offline -- \
    --samples 300 --warmup 30 --build-samples 1 --build-warmup 0 \
    --report ".tmp/round-$i.json"
done
```

### Machine metadata

| Field | Value |
| :-- | :-- |
| Timestamp | 2026-11-24T17:05:00Z |
| OS | Linux 7.3.5 x86\_64 |
| CPU | AMD Ryzen 7 8845HS w/ Radeon 780M Graphics |
| Logical CPUs | 16 |
| Max frequency | 3,801,000 kHz |
| Memory | 96,582,424 KiB |
| rustc | 1.99.1 (58cf8f9c3 2026-10-13) |
| cargo | 1.99.1 (5f94df478 2026-08-27) |
| Profile | release, fat LTO, codegen-units 1 |
| Samples | 300 per surface, 30 warmup, 3 rounds |
| Percentiles | nearest rank |
| Timer | `std::time::Instant` |

## Compiler adoption after stage 3

The compiler coverage scan compiles every entry in the 366-plugin community
corpus from `/home/user/projects/community-plugins-rs`. It uses the same
`noctalia_rs_compiler::compile` call the build uses, with the entry id, handler,
kind and `<last id segment>.state`. A handler that fails still builds through
the bridge in `auto` mode; this scan only measures how many entries can skip it.

| Stage | Compiled entries | Adoption |
| :-- | --: | --: |
| single-file compiler | 8 / 1,067 | 0.75% |
| stage 1, whole-crate helpers | 14 / 1,067 | 1.31% |
| stage 2, closures and statics | 30 / 1,067 | 2.81% |
| stage 3, JSON and broader syntax | 77 / 1,067 | 7.22% |
| stage 4, host callbacks and richer syntax | 185 / 1,067 | 17.34% |

After stage 3: widget 44/319, shortcut 28/80, service 2/265, desktop widget
1/36, launcher provider 2/64, panel 0/299, layer 0/4. The largest remaining
blockers are unresolved names (190), unsupported methods (168), unresolved
calls (82), helpers that return no value (72), path expressions (46), nested
match patterns (45), callback-id ops (30), and format specifiers (24). Each one
fails the build with a position in strict mode.

This is a measurement of compiler reach, not of the bridge path. The benchmark
above still measures runtime performance. Three rounds of 300 samples after
stage 3 measured compiled medians of 1.934 us, 10.204 us and 3.697 us against
hand-written medians of 2.014 us, 17.007 us and 3.878 us. Median ratios were
0.99x, 1.67x and 1.00x, so the small fixtures remain at parity within host
noise and the UI tree keeps its advantage. `spool_dispatch` was 79 us in the
median round.

### Stage 4

Stage 4 turns callback-id ops into named host callbacks. `Op::run`,
`Op::run_stream`, `Op::http`, and `Op::state_watch` emit the marker table the
bridge already uses, and the Luau runtime hands that name to the entry's
`onHostCallback(payload)` global when no registered function matches the id.
A compiled `host_callback` branch therefore runs with the same payload and
emits the same ops as the Rust handler.

After stage 4: widget 112/319, shortcut 41/80, panel 15/299, service 7/265,
desktop widget 5/36, launcher provider 5/64, layer 0/4. The largest remaining
blockers are unsupported methods (219), unresolved names (78), unresolved calls
(63), unsupported expressions (52), host-start methods whose acceptance result
is read (43), `vec!` of ops outside `Ok(...)` or a `let` (32), and tuple loop
bindings over values (31). Each one fails the build with a position in strict
mode.

All 185 compiled scripts load in the Luau VM. A parse check over the whole set
found eight shapes that emitted invalid Luau before this stage: an `else if`
whose condition emits statements, an inlined helper body used as a statement, a
table literal followed by an index or a method call, `drop(x)` in statement
position, a unit closure passed to `get_or_init`, `vec!` elements that emit
statements, a tuple pattern over a table literal, and a character class
containing control characters.

Two rounds of 300 samples measured compiled medians of 1.944 us and 1.924 us
against hand-written medians of 1.923 us and 1.884 us for the bar, 11.075 us
and 9.758 us against 16.602 us and 16.696 us for the UI panel, and 3.707 us and
3.607 us against 3.657 us and 3.636 us for the event fixture. Ratios were
0.99x, 1.50x and 0.99x, then 0.98x, 1.71x and 1.01x. Compiled source bytes are
unchanged at 2,405, 2,938 and 3,174 for the three fixtures, so the generated
code is byte-identical to stage 3 and the ratios move with host noise.

### Exact command

```sh
nix develop --command cargo run --release -p noctalia-bench --offline -- \
  --samples 300 --warmup 30 --build-samples 1 --build-warmup 0 \
  --report .tmp/stage3-round-1.json
```

The corpus is treated as read-only; the scan and its scratch files live in
`.tmp` and are deleted after each run.

## Compiler adoption after stage 5

Stage 5 stays inside the compiler. It maps iterator, string, numeric and container
operations with exact Luau equivalents, broadens tuple loops and closure patterns,
adds transparent newtypes and finite JSON numbers, and rejects Luau keyword
identifiers instead of emitting invalid scripts. No Rust API, bridge, compile mode
or host global changed.

| Stage | Compiled entries | Adoption |
| :-- | --: | --: |
| stage 4 | 185 / 1,067 | 17.34% |
| stage 5, group 1 (method and call mappings) | 188 / 1,067 | 17.62% |
| stage 5, group 2 (syntax) | 194 / 1,067 | 18.18% |
| stage 5, group 3 (newtypes and finite numbers) | 198 / 1,067 | 18.56% |
| stage 5, final after the keyword bail | 197 / 1,067 | 18.46% |

Group 4 (context field and method gaps) found no remaining gap with an exact
existing global. The 43-entry blocking query group and the 22-entry JSON null group
were deliberately left alone.

Final per-kind adoption: widget 119/319, shortcut 42/80, panel 18/299, service
7/265, desktop widget 6/36, launcher provider 5/64, layer 0/4.

The largest final blockers, counted by distinct entries, are unsupported
expressions (53), blocking host-start acceptance results (36), `vec!` of ops
outside `Ok(...)` or a let (32), `&mut` helper parameters other than
`&mut Vec<Op>` (23), unsupported path expressions (23), JSON null inside an object
(22), Luau keyword identifiers (20), unsupported `Ok` match patterns (20),
unparsable Rust syntax (19), and unsupported literal arguments (18). Each one
still fails with `file:line:column` in strict mode.

Examples that now compile end to end include `audio-switcher/launcher`,
`cider/now-playing`, `cider/osd` and `containers/bar`, all of which use finite
JSON numbers from `Number::from_f64`. A Luau parse check loaded all 197 final
compiled scripts with zero failures.

Full workspace tests, `clippy -D warnings`, `fmt --check` and tarpaulin all pass.
Coverage is 80.87% (7,972 / 9,858 lines). Three rounds of 300 samples measured
compiled medians of 1.883 / 1.934 / 1.903 us for `generated_hello_bar`,
9.933 / 9.949 / 10.399 us for `generated_ui_panel` and 3.666 / 3.737 / 3.677 us
for `generated_event`. Against hand-written Luau the ratios were 0.98x / 0.98x /
1.01x, 1.72x / 1.68x / 1.62x and 0.98x / 0.96x / 1.24x. The small fixtures stay
at parity inside host noise and the UI panel keeps its advantage. Generated source
sizes are unchanged at 2,405, 2,938 and 3,174 bytes.

100% is impossible under these constraints at the first entry that needs JSON null
as an object value or a blocking query result. A Luau table cannot distinguish an
absent key from a null value without a representation change, and compiled entries
have no synchronous host response path without a new host API. Both would require
changes outside the compiler.

## Hybrid entries after stage 6

Stage 6 splits the helper boundary at the function instead of the entry. A
handler the compiler cannot translate as a whole can now keep the untranslatable
functions in the plugin binary and compile the rest to Luau. Three tiers exist:

- fully compiled: every reached function translates, no helper, byte-identical
  to stage 5 (generated sizes stay 2,405 / 2,938 / 3,174 bytes),
- hybrid: only the untranslatable functions cross, one round trip per uncached
  call,
- bridged: the handler body itself cannot be split, or a native candidate has a
  signature that cannot cross JSON, so the entry keeps the whole-entry bridge.

`strict` denies the boundary and reports the same errors as stage 5. `auto`
allows it. `off` is unchanged.

### Corpus adoption

The scan runs the same `noctalia_rs_compiler::compile` call the build uses over
all 1,067 entries in 366 plugins. A second scan builds every plugin with the new
build script against a local path dependency and restores the corpus.

| Stage | Compiled | Hybrid | No whole-entry bridge | Bridged |
| :-- | --: | --: | --: | --: |
| stage 5 record | 197 | 0 | 197 / 1,067 (18.46%) | 870 |
| base `346b3de`, re-measured | 195 | 0 | 195 / 1,067 (18.27%) | 872 |
| stage 6 | 195 | 41 | 236 / 1,067 (22.12%) | 831 |

The same scan runs against base `346b3de` and stage 6: all 195 base scripts are
byte-identical by md5, and stage 6 adds 41 hybrid scripts. Nothing that
compiled before changed.

No base entry moved to bridged, and no base entry moved to hybrid. The 41
hybrid entries were bridged before.

The 366-plugin build scan has one failure, `github-prs` at
`src/main.rs:1034` E0283. It fails on base `main` too and is unrelated to this
stage. Every other plugin builds with the generated `native.rs`.

The 60% no-whole-entry-bridge target is not reachable inside the decided
design. The measured blockers, counted by the first error in each bridged entry:

| Blocker | Entries |
| :-- | --: |
| native candidate is not visible from the crate root (`pub(crate)` needed) | 109 |
| native candidate takes `&mut`, whose writes would be lost | 78 |
| native candidate returns a type that cannot be serialized | 76 |
| unsupported method in the handler body itself | 66 |
| blocking host-start acceptance read inside the handler body | 60 |
| unknown local inside the handler body | 60 |
| native candidate takes a type that cannot be deserialized | 43 |
| unresolved call inside the handler body | 32 |
| `vec!` of ops outside `Ok(...)` or a let | 31 |
| unsupported expression in the handler body | 31 |

The first three are signature gates, not missing translations. Supporting
`&mut` would silently drop the writes. A `&Context` parameter is supported, but
109 candidates sit in modules and are private, so the generated dispatch at the
crate root cannot call them. The rest fail inside the handler body, where no
function-level split can help.

### Native call cost

The native round trip was measured against a real hybrid helper: a small plugin
with one `#[native] fn add(u64) -> u64`, driven by a request writer that mirrors
the Luau side (write, rename, wait for the `native_result` line).

| Surface | Round 1 | Round 2 | Round 3 |
| :-- | --: | --: | --: |
| uncached native call, median | 51.44 us | 49.12 us | 50.32 us |
| uncached native call, p95 | 84.19 us | 71.88 us | 79.78 us |
| helper cache hit (`cache_ms = 1000`), median | 52.50 us | 52.50 us | 52.50 us |

A cached call on the Luau side performs no round trip at all. That is proven by
`crates/luau/tests/native_boundary.rs`: two identical calls with `cache_ms`
resume the handler from a single `native_result`. The helper cache still pays
the transport when the Luau cache misses; it only avoids recomputing the
function.

For comparison, `spool_dispatch` (a whole bridged entry) measured 81.0 us and
82.0 us in the same session. A hybrid handler with one uncached call therefore
costs about 50 us on top of the compiled path, and a hybrid handler whose calls
are cached runs at the fully compiled cost.

### Runtime benchmark

Three rounds of 300 samples, 30 warmups, same harness as stage 5.

| Surface | Round 1 | Round 2 | Round 3 | Hand-written |
| :-- | --: | --: | --: | --: |
| `generated_hello_bar` compiled | 1.924 us | 1.873 us | 1.894 us | 1.883 / 1.873 / 1.894 us |
| `generated_ui_panel` compiled | 10.800 us | 9.873 us | 9.744 us | 17.067 / 17.052 / 16.952 us |
| `generated_event` compiled | 3.657 us | 3.647 us | 3.697 us | 3.922 / 3.627 / 3.577 us |
| `spool_dispatch` | 82.0 us | 82.0 us | 81.0 us | |

Ratios are 0.98x-1.00x / 1.58x-1.74x / 0.97x-1.07x. Generated source sizes are
unchanged from stage 5, so the fully compiled path is byte-identical and the
movement is host noise.

### Exact commands

```sh
nix develop --command cargo run --release -p noctalia-bench --offline -- \
  --samples 300 --warmup 30 --build-samples 1 --build-warmup 0 \
  --report .tmp/stage6-round-1.json
```

The corpus is treated as read-only. The scan and its scratch files live in
`.tmp`, and the corpus manifest, lock file, and every tracked file are restored
and checked by md5 before the scan ends.

## 0.4.0 final baseline

Main `c9ad4a5` contains S0 through S9 and S7's bridge work. Plugin API stays at
57\. The corpus has 366 plugin crates and 1,067 handler entries. The release is
additive: every mode, public API, helper feature, and bridge mechanism stays.
The only behavior change is that strict translates more entries and emits no
bridge for them.

### Final adoption by mode

| Mode | Entries OK | Total | Valid scripts | Output |
| :-- | --: | --: | --: | :-- |
| strict | 316 | 1,067 | 316 / 316 | Standalone API 57 Luau. No bridge, helper, markers, `onHostCallback`, or response files. |
| auto | 1,067 | 1,067 | 332 / 332 | Default. Fully compiled, hybrid, and bridged entries; helper, bridge, spool, and response files stay. |
| off | 1,067 | 1,067 | 0 | No compiler. Every entry uses the bridge. |

### Adoption by merge stage

| Stage | Merge commit | Strict OK / total | Adoption | Auto valid scripts | Off |
| :-- | :-- | --: | --: | --: | :-- |
| S0 emit split | `e4a6593` | 195 / 1,067 | 18.28% | 234 / 236 | 1,067 / 1,067 |
| S7 bridge speed | `976307e` | 195 / 1,067 | 18.28% | 234 / 236 | 1,067 / 1,067 |
| S1 strict ABI | `d87f877` | 206 / 1,067 | 19.31% | 236 / 236 | 1,067 / 1,067 |
| S2 `serde_json` | `8a6e087` | 221 / 1,067 | 20.71% | 246 / 246 | 1,067 / 1,067 |
| S3 `std::fs` and `std::path` | `57796e4` | 221 / 1,067 | 20.71% | 247 / 247 | 1,067 / 1,067 |
| S4 methods and iterators | `4bc71c4` | 232 / 1,067 | 21.74% | 252 / 252 | 1,067 / 1,067 |
| S5 scoping and syntax | `1420b22` | 308 / 1,067 | 28.87% | 324 / 324 | 1,067 / 1,067 |
| S6 mutable helpers | `b09a559` | 313 / 1,067 | 29.33% | 328 / 328 | 1,067 / 1,067 |
| S9 exact integers | `c9ad4a5` | 316 / 1,067 | 29.62% | 332 / 332 | 1,067 / 1,067 |

Auto and off reach 1,067 / 1,067 at every stage. These are separate mode
outcomes. In `strict`, an entry that cannot be translated stops the build; it
never silently switches to auto. In `auto`, a function whose signature can cross
JSON can run as a native call in the helper while the rest of the handler stays
in Luau. A handler that cannot be split keeps the whole-entry bridge through
`bridge.luau`. `off` bridges every entry. Every strict script is parsed in the
Luau VM. The final 316 and 332 counts come from S9's scan at `c9ad4a5`. Earlier
counts come from the stage scans recorded in git history and T1's baselines,
re-recorded whenever the compared SHA moved.

### Bridge performance, S7 before and after

Best of three paired runs, release build, 200 samples and 20 warmups per
surface. Times are in microseconds.

| Surface | Median before | Median after | p95 before | p95 after | p99 before | p99 after |
| :-- | --: | --: | --: | --: | --: | --: |
| `spool_dispatch` | 78.03 | 41.30 | 97.26 | 54.80 | 105.77 | 69.44 |
| `bridge_event` | 119.38 | 92.53 | 130.26 | 102.33 | 132.57 | 107.38 |
| `generated_hello_bar` compiled | 1.89 | 1.92 | 1.99 | 2.02 | 2.07 | 2.07 |
| `generated_ui_panel` compiled | 9.88 | 9.70 | 14.28 | 12.98 | 15.67 | 15.36 |

The compiled surfaces stay at about 1.9 us and 10 us. S7 did not touch the
compiled path. Strict entries use neither `spool_dispatch` nor `bridge_event`.

### Accepted source-change classes

The corpus was not edited. These classes are plugin-side work a maintainer can
choose to do. No host API, global, permission, or `plugin_api` value changes.

| Class | Measured count | Recommendation |
| :-- | --: | :-- |
| JSON null inside a container | 22 entries at the first-error baseline | Edit those plugins to avoid computed nulls in tables and arrays. The compiler fails at `file:line:column` rather than dropping a key. Keep the entry on auto until it is edited. |
| `Context::Parts` and `set_op_sink` | 5 plugins: `keymap`, `mullvad`, `pixel-shift`, `bangumi-progress`, `wallpaper-deck`. 14 files use `Context::Parts`, 3 use `set_op_sink`. | Rewrite to a normal `Context` and direct `Op` emission for strict. Modeling a cloned context and stdout op sink in Luau would not be faithful. |
| Cooperative coroutine helpers | 4 plugins: `terminal-widgets`, `keymap`, `pixel-shift`, `wallpaper-deck`. `yield_now` in 10 files, `Coroutine::spawn` in 9, `ClockBudget` in 4. | Use callback-driven handlers for strict, or keep the entry on auto with its helper. Strict uses real acceptance booleans and closures, not coroutine parking. |
| Byte exactness through the host | 31 plugins use byte APIs. | `ByteBuf` maps to Lua byte strings and `__noctalia_rs_bytes` stays for JSON crossings. Check non-UTF-8 values against a real Noctalia host before relying on strict. No host change is assumed. |
| Integer edge methods | 2 entries failed faithfully at S4. Exact limb lowering in S9 recovers 3 entries in total. 0 integer-edge first errors remain at `c9ad4a5`. | No source edit is needed. Fail closed when width, signedness, or exactness is unproven; lower exactly with 32-bit limbs otherwise. Auto and off keep native Rust arithmetic. |

The recovered integer entries are `ccf-deadlines/widget[0]`,
`hyprland-minimize/widget[0]`, and `currency-exchange/panel[0]`.

### Gates and coverage

`cargo fmt --check`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and both performance script tests pass on the
docs branch. Tarpaulin was recorded at 81.59% (13,930 / 17,074 lines) on S9.
Docs-only changes do not rerun coverage.
