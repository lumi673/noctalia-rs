# NoctaliaRs

Unofficial project. It is not maintained by or affiliated with the Noctalia team.
Source: https://github.com/lumi673/noctalia-rs

Write Noctalia plugins in Rust. `noctalia.toml` describes the plugin. Rust
handles behavior. The library emits `plugin.toml`, Luau entry stubs, the shared
bridge, translations, and assets.

When a Luau plugin is translated to Rust, the Rust handler must preserve the
original algorithm exactly, including its complexity and habits. Performance
gains come from the generated Luau, bridge, and runtime, not from changing what
a plugin computes.

## Features

- `noctalia.toml` as the source of truth for metadata, settings, and entries
- Compiled Luau entries by default, with per-function native calls for arbitrary Rust and bridge fallback in `auto` mode
- Version taken from `Cargo.toml` through `CARGO_PKG_VERSION`
- No Nix requirement; Nix packaging stays optional
- All Noctalia setting types and entry kinds
- Plugin-wide and entry-scoped settings
- Entry metadata for panels, launchers, widgets, and layers
- Assets and extra translation files emitted into the plugin directory
- Typed ops for common host calls and a generic `HostCall` escape hatch
- Typed UI props (`Props`) that keep keys and static values borrowed instead of
  building a `serde_json` map per node
- Host reads through `Context`
- Event-driven spool dispatch, so idle helpers sleep instead of polling

## Cargo setup

```toml
[dependencies]
noctalia-rs = "0.3"
serde_json = "1.0"

[build-dependencies]
noctalia-rs-build = "0.3"
```

For Rust handlers, the dependency, `build.rs`, and `noctalia.toml` shown below are enough. Entries with a hand-written `.luau` script and no `handler` also need `noctalia-rs-luau = "0.3"` in `[dependencies]`.
Package names and Rust library names now match. Plugin code imports `noctalia_rs::`, and `build.rs` calls `noctalia_rs_build::generate()`.

Add `build.rs`:

```rust
use std::error::Error;

fn main() -> Result<(), Box<dyn Error>> {
    noctalia_rs_build::generate()
}
```

Add `noctalia.toml`:

```toml
id = "me/hello"
name = "Hello"
plugin_api = 57
author = "Me"
license = "MIT"
description = "A Rust plugin"
tags = ["rust"]
icon = "sparkles"
dependencies = []

[[setting]]
key = "message"
type = "string"
label_key = "settings.message.label"
default = "Hello"

[[service]]
id = "service"
entry = "service.luau"
handler = "service"
```

Each entry names a handler function in your crate. The build finds that function
in the crate source. The generated plugin takes its version from `Cargo.toml`.

Declare non-code files with `assets`. Shared Luau modules go in `luau_sources`. Entries without `handler` embed the script named in `entry` automatically. Only declared files are copied into the plugin directory.

```toml
assets = ["icons/clock.svg"]
luau_sources = ["lib/util.luau"]
```

Put the strings for `label_key` and `description_key` in `translations/en.json` or another language file.

Write `src/main.rs`:

```rust
use std::process::ExitCode;

use noctalia_rs::{Context, Op, Result};

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

fn service(ctx: &Context) -> Result<Vec<Op>> {
    let message = ctx.config_str("message").unwrap_or_default().into_owned();
    let _ = serde_json::from_str::<serde_json::Value>("null")?;
    Ok(vec![Op::set_interval(1000), Op::log(message)])
}

fn main() -> ExitCode {
    noctalia_rs::run(&plugin())
}
```

## Compile modes

`compile` selects how Rust handlers become Luau entries. It defaults to `auto`:

```toml
compile = "auto"
```

`auto` compiles each handler it finds, splits the functions it cannot translate
into native calls, and bridges only the entries it cannot split. The build
prints one `cargo:warning` per function that crossed the boundary and one per
entry that kept the bridge stub, each with the entry id and the reason.
`strict` fails the build on the first function it cannot translate, ignores
`#[native]`, and emits no bridge, so it measures pure-Luau coverage. It also rejects entries without a
handler. `off` skips the compiler and bridges every entry.

The handler signature is `fn handler(ctx: &Context) -> Result<Vec<Op>>`. The
build walks the module tree from `src/main.rs` or `src/lib.rs`, follows `mod`
declarations and `#[path]`, and finds the function named by `handler`. No extra
manifest key is needed:

```toml
[[widget]]
id = "widget"
entry = "widget.luau"
handler = "widget"
```

Use `luau` only when the function lives in a file outside that module tree:

```toml
luau = "src/widget_handler.rs"
```

`NOCTALIA_RS_COMPILE` overrides the manifest so CI can gate a whole workspace:

```sh
NOCTALIA_RS_COMPILE=strict cargo build --release
```

Valid values are `strict`, `auto`, and `off`. An invalid value stops the build.

The compiler reads the whole crate. It resolves calls through `crate::`,
`self::`, `super::`, plain module paths, and simple `use` aliases. Helpers it
can translate are inlined: a `&Context` and `&mut Vec<Op>` helper at statement
position, a value helper in an expression, and a handler that delegates to
another module. Helpers with early `return`s or `?` run inside a Luau function
so those operators return from the helper, including inside nested loops.
`Option` `?` skips on nil. `Err(...)` raises `error(...)` at the point it is
constructed, so `?` on a `Result` needs no extra check.

Closures become Luau functions, including `map`, `and_then`, `filter`,
`is_some_and`, `unwrap_or_else`, `get_or_init`, and `call_once`. `Once`,
`OnceLock`, `LazyLock`, `Mutex`, `RwLock`, and atomics map to Lua locals. Those
locals live for one compiled script, not for the process, so a static shared by
several Rust entries becomes one value per entry. Simple structs, impl methods,
tuple bindings, `if let`, `let-else`, `matches!`, and pattern matches over
`Option` and `Value` also compile. Event checks are folded at compile time,
and the compiler covers config, payload, state, settings, `tr`, host reads,
common ops, and constants. Errors name the module when they sit in an inlined
helper.

JSON parsing and `serde_json::Value`, nested assignments, common iterators,
string and numeric methods, format specifiers, character-set trimming, string
slices, blocks and loops are translated. Match guards read their pattern
bindings, nested `or` patterns and constant patterns compare directly, and a
crate function passed as a callback value, such as `map(lua_text)`, is inlined
at the call.

Host callbacks use stable names. `Op::run`, `Op::run_stream`, `Op::http`, and
`Op::state_watch` emit `{ __host_callback = "name" }`, and the runtime delivers
that name to the entry's `onHostCallback(payload)` global, so a compiled
`host_callback` branch sees the same payload the Rust handler does.
`ctx.run_async_start`, `ctx.run_stream_start`, and `ctx.http_start` compile when
their acceptance result is dropped, because compiled Luau starts the call
without blocking.

Arbitrary trait iterators, dynamic dispatch, trait methods without a Luau
equivalent, and ops that need a response path are never approximated. In
`auto` they move the whole function to the plugin binary and the rest of the
handler still compiles; in `strict` they fail the build with
`file:line:column`. `BASELINE.md` has the per-stage adoption table for the community corpus: 195
fully compiled entries, 41 hybrid entries, and 831 that still bridge the whole
handler. The largest remaining blocker is that a candidate helper must be
`pub(crate)`; the next is `&mut` parameters, which cannot cross without
silently dropping the writes.

## Native functions

A handler compiles in one of three tiers:

- **Fully compiled**: every function reached from the handler translates to
  Luau. The script needs no helper and runs at hand-written Luau speed.
- **Hybrid**: the functions the compiler cannot translate keep running in the
  plugin binary, and the rest of the handler still compiles to Luau. Each
  uncached native call costs one round trip to the helper that is already
  running, then the handler continues in Luau.
- **Bridged**: the entry cannot be split, so the whole handler runs in the
  helper through `bridge.luau` as before. This happens when the handler body
  itself contains a construct with no faithful translation, when a module
  initializer needs native code, or when a helper that returns
  `Result<Vec<Op>>` is called in tail position. Move the untranslatable work
  into a value-returning helper in the last case.

The split point is always a whole function, never inside one. A helper that
calls a native function still compiles: the call becomes a call to the stub.
Recursion between native functions is fine, because the helper runs real Rust.
The build prints one line per function that crossed the boundary, for example:

```
entry widget: native `git::fetch_prs` (cache 0 ms): call to `reqwest::blocking::get` is not supported by the Luau compiler
```

A native function must be visible from the crate root, because the generated
dispatch table lives there. Functions at the crate root can stay private.
A helper inside `src/other.rs` or a module needs `pub(crate)`. `auto` bridges
the entry and names the function when it is not; `#[native]` fails the build
with the same message.

### Attributes

```rust
use noctalia_rs::{Context, Op, Result, native, luau};
use serde::Deserialize;

#[derive(Deserialize)]
struct Row {
    title: String,
}

/// Always runs in the plugin binary, even if the compiler could translate it.
#[native]
fn parse_rows(document: String) -> Result<Vec<Row>> {
    Ok(serde_json::from_str::<Vec<Row>>(&document)?)
}

/// Same, with a 30 second cache shared by the whole helper process.
#[native(cache_ms = 30_000)]
fn fetch_prs(limit: usize) -> Result<Vec<Row>> {
    let url = format!("https://api.github.com/repos/lumi673/noctalia-rs/pulls?per_page={limit}");
    let rows = reqwest::blocking::get(url)?.json::<Vec<Row>>()?;
    Ok(rows)
}

/// Requires Luau: the build fails with `file:line:column` instead of splitting.
#[luau]
fn label(count: usize) -> String {
    format!("{count} pull requests")
}

fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let document = ctx.config_str("document").unwrap_or_default().into_owned();
    let rows = parse_rows(document)?;
    let titles = rows.iter().map(|row| row.title.clone()).collect::<Vec<_>>();
    Ok(vec![Op::BarText { text: label(titles.len()) }])
}
```

`#[native]` and `#[luau]` are inert attribute macros: `rustc` expands them to
the unchanged function, and the build script reads them from the source text.
`#[native]` and `#[native(cache_ms = ...)]` skip the translation attempt
entirely. `#[luau]` turns a would-be split into a build error, even in `auto`.
`strict` has no boundary, so it ignores `#[native]` and compiles the function
instead; `#[luau]` keeps its meaning.

### Signature requirement

The arguments and the result cross as JSON. A native function must:

- take plain identifier parameters: owned values (`String`, `usize`,
  `Vec<Row>`, `serde_json::Value`), or borrowed forms the dispatcher can
  recreate from an owned value: `&str`, `&String`, `&Path`, `&PathBuf`,
  `&Vec<T>`, `&[T]`, `Option<&...>` of those, and `&T` or `Option<&T>` for a
  type that implements `Deserialize`. `&mut`, `self`, generics and
  `impl Trait` are rejected with `file:line:column`,
- implement `serde::de::DeserializeOwned` for every parameter type (the owned
  form of a borrowed parameter),
- return `T: serde::Serialize` (a returned reference is fine), or
  `Result<T, E>` where `T: Serialize` and `E: std::fmt::Display`.

`auto` only splits a function whose signature it can prove: parameters and the
success type must be scalars, strings, `serde_json::Value`, paths, standard
containers of those, or a local struct with the matching `Serialize` or
`Deserialize` derive. Anything else keeps the bridge and names the type. That
keeps `auto` from breaking a build on a type that cannot cross.

`#[native]` skips that proof and asks `rustc` instead: the build writes the
dispatch table to `$OUT_DIR/native.rs`, and a missing `Serialize`,
`Deserialize` or `Display` is reported on the exact type.

### Handler context

A native function may take `&Context` at any position. Only calls to functions
that declare it pay for it. Before the request is written, the compiled script
rebuilds the same snapshot `bridge.luau` sends: entry, event, payload, every
declared config key, the entry's state, and the fixed host reads (time, theme
colors, bar, clipboard, media, stats, nightlight, and so on). The helper turns
that back into a real `Context`. Two values can differ from a bridge request:
`now_ms` is sampled when the native call happens, and state written earlier in
the same handler invocation is already visible. The context is part of the cache
key, so a call from a click never reuses a render result.

### Calling semantics

Compiled Luau has no blocking. A hybrid handler runs inside a coroutine: when a
native stub is called, the handler yields, the runtime writes the request to
the helper's spool, and the host returns to its event loop. The helper answers
on the existing stdout stream and the coroutine resumes where it stopped.

- Ops emitted before the call apply immediately. Ops emitted after it apply
  when the result arrives.
- `?` on a native `Result` propagates the helper error as a Luau error, the
  same way `Err(...)` does in compiled Luau.
- One event per entry is handled at a time. If another event arrives while a
  coroutine is parked, it is queued and drained in arrival order.
- A native call inside a closure or callback cannot cross the boundary,
  because there is no coroutine to park. Move it into the handler body or a
  helper function, or mark that helper `#[native]` so the whole function runs
  in the helper.

### Caching

`#[native]` always calls the helper. `#[native(cache_ms = 30_000)]` caches the
result for the key `(function name, encoded arguments)`, plus the invocation
context when the function takes `&Context`, for 30 seconds. The
compiled script keeps a bounded in-memory cache (128 entries, oldest first) and
the helper keeps the same bound as a backstop shared by every Luau session. A
cached call performs no round trip at all.

A native function argument that contains a large table is part of the cache
key, so change the arguments only when the result should change.


Build and install:

```sh
cargo build --release
mkdir dist
cp target/release/my-plugin dist/my-helper
./target/release/my-plugin emit dist my-helper
```

Add `dist` as a local plugin source in Noctalia and enable the plugin id.

The `template/` crate in this workspace is a complete minimal example.

## Optional Nix packaging

```nix
{
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    noctalia-rs.url = "github:lumi673/noctalia-rs";
  };

  outputs = { self, nixpkgs, noctalia-rs }:
    let system = builtins.currentSystem;
    in {
      packages.${system}.default = noctalia-rs.mkPlugin.${system} {
        pname = "my-plugin";
        version = "0.1.0";
        src = ./.;
        cargoLock.lockFile = ./Cargo.lock;
        package = "my-plugin";
        helperName = "my-helper";
      };
    };
}
```

Run `nix build --out-link dist`. The output is a Noctalia local plugin source.
Nix only automates the build and helper placement. It is not required by the
library.

## Parity checks

`crates/lib/tests/parity.rs` checks that the generated API covers the setting
types, entry kinds, and manifest fields used by community plugins. Without
configuration it uses an embedded fixture. Set `NOCTALIA_PARITY_FIXTURES` to a
checkout of a plugin collection to check every real manifest.

## Performance harness

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

CI runs the same harness three times on every pull request and publishes a
`Performance audit` check and comment. The check compares the best round with a
ceiling re-measured in the same session from the `floors` branch, and with each
workload's budget. A workload fails when its median or p95 is more than
10% slower than the reference worst round, or by more than either side's spread
between rounds, whichever is larger, with a floor of 2 µs so a few microseconds
of drift on the smallest workloads never fails on its own. A p99 failure also
needs 100 µs of absolute excess because a single delayed sample can move the
slowest sample on a busy runner. It also fails when its median is more than
10% over budget while the reference is within budget.
Budgets only tighten. Pushes to main record a new ceiling. The benchmark
scripts live in `.github/scripts`.
