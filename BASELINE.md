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
