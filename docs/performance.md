# Performance harness

Run all release benchmarks with:

```sh
cargo run --release -p noctalia-bench -- --samples 200 --warmup 20 --report bench.json
```

The harness builds the template helper, then measures fresh-process dispatch,
persistent spool dispatch, Luau VM creation and cache reuse, on-demand Luau
dispatch, JSON encode and decode, snapshot reads, code generation, and an
incremental release build. It prints machine metadata and median, p95, and p99
per surface. Use `--build-samples` and `--build-warmup` for the build surfaces,
`--helper` for an existing helper binary, and `--help` for all options. Set
`NOCTALIA_BENCH_FIXTURE_TARGET` to keep the fixture build directory between
runs, which is what makes a second run fast. It uses its own scratch files and
never reads the Noctalia config.

CI shards the harness across runners. Each workload and its reference are
measured together on one runner, three times on a release build, so runner speed
cancels out. The check compares the best round with a ceiling re-measured in that
session from the `floors` branch, and with each workload's budget. A workload
fails when its median or p95 is more than
10% slower than the reference worst round, or by more than either side's spread
between rounds, whichever is larger, with a floor of 2 µs so a few microseconds
of drift on the smallest workloads never fails on its own. A p99 failure also
needs 100 µs of absolute excess because a single delayed sample can move the
slowest sample on a busy runner. It also fails when its median is more than
10% over budget while the reference is within budget.
Budgets only tighten. Pushes to main record a new ceiling. The benchmark
scripts live in `.github/scripts`.
