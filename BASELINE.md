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

After API 57 parity landed in the root checkout, the same release harness was run again with `nix develop --command cargo run --release -p noctalia-bench -- --samples 200 --warmup 20 --report .tmp/post-fix.json`. The matched pairs stayed above the 3x goal:

| Workload | Rust median | Luau median | Speedup | 3x met |
| :-- | --: | --: | --: | :-- |
| json\_roundtrip | 0.651 | 5.806 | 8.92x | yes |
| snapshot\_reads | 0.100 | 6.833 | 68.33x | yes |
| state\_ops | 0.291 | 6.692 | 23.00x | yes |
| ui\_tree | 0.661 | 17.293 | 26.16x | yes |
| callback\_dispatch | 0.060 | 1.884 | 31.40x | yes |
| service\_update | 0.231 | 8.627 | 37.34x | yes |

The parity work therefore kept the baseline and all six matched workloads remain above 3x.
