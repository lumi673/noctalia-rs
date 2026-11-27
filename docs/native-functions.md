# Native functions

A handler compiles in one of three tiers. The helper boundary exists only in
`auto` and `off`. A strict build never writes a native request, starts a helper,
or loads `bridge.luau`.

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

    entry widget: native `git::fetch_prs` (cache 0 ms): call to `reqwest::blocking::get` is not supported by the Luau compiler

A native function must be visible from the crate root, because the generated
dispatch table lives there. Functions at the crate root can stay private.
A helper inside `src/other.rs` or a module needs `pub(crate)`. `auto` bridges
the entry and names the function when it is not; `#[native]` fails the build
with the same message.

## Attributes

```rust
use noctalia_rs::{Context, Op, Result, native, luau};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize)]
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

## Signature requirement

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

## Handler context

A native function may take `&Context` at any position. Only calls to functions
that declare it pay for it. Before the request is written, the compiled script
rebuilds the same snapshot `bridge.luau` sends: entry, event, payload, every
declared config key, the entry's state, and the fixed host reads (time, theme
colors, bar, clipboard, media, stats, nightlight, and so on). The helper turns
that back into a real `Context`. Two values can differ from a bridge request:
`now_ms` is sampled when the native call happens, and state written earlier in
the same handler invocation is already visible. The context is part of the cache
key, so a call from a click never reuses a render result.

## Calling semantics

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

## Caching

`#[native]` always calls the helper. `#[native(cache_ms = 30_000)]` caches the
result for the key `(function name, encoded arguments)`, plus the invocation
context when the function takes `&Context`, for 30 seconds. The
compiled script keeps a bounded in-memory cache (128 entries, oldest first) and
the helper keeps the same bound as a backstop shared by every Luau session. A
cached call performs no round trip at all.

A native function argument that contains a large table is part of the cache
key, so change the arguments only when the result should change.
