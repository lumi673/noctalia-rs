# Compile modes

`compile` selects how Rust handlers become Luau entries. It defaults to `auto`:

```toml
compile = "auto"
```

| Mode | What ships |
| :-- | :-- |
| `strict` | Standalone API 57 Luau for every translated entry. No `lib/bridge.luau`, helper binary, host-callback markers, `onHostCallback`, or native stubs, and no bridge response-file plumbing. A strict `Op::query` still writes its response envelope with the API 57 `writeFile` member. An entry that cannot be translated fails the build with `file:line:column`. Entries without a handler are rejected. `#[native]` is ignored; `#[luau]` still requires Luau. |
| `auto` (default) | Compiles each handler it can, splits functions it cannot translate into native calls through the helper, and keeps the whole-entry bridge for the rest. |
| `off` | Skips the compiler and bridges every entry. |

`auto` and `off` keep the full bridge machinery. `bridge.luau` turns callback and
action markers into real API 57 functions before the host sees them. Native
stubs cross the helper boundary as JSON requests over the spool, and
`Op::Query` responses use response files. `crates/luau` enforces the API 57
contract per path: a strict script that uses a marker or defines
`onHostCallback` fails a test, while an auto script still passes through the
bridge.

In `auto`, the build prints one `cargo:warning` per function that crossed the
helper boundary and one per entry that kept the bridge stub, each with the entry
id and the reason. Strict stops on the first failure, and `off` skips the
compiler.

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

Each entry kind has a typed event enum in `noctalia_rs::events`: `ServiceEvent`,
`WidgetEvent`, `ShortcutEvent`, `LauncherEvent`, `PanelEvent`, `LayerEvent`, and
`DesktopWidgetEvent`. Read it with the matching `Context` method, such as
`ctx.widget_event()`, and match its variants instead of comparing
`ctx.event.as_str()` to strings. A variant with a payload binds the decoded
payload table, so `WidgetEvent::Scroll(scroll)` gives access to the same fields
`ctx.payload_str("axis")` reads. The compiler folds the match into the existing
per-event Luau branches in `strict` and `auto`; `off` decodes it in the native
handler. Unknown host events stay
in the `Unknown(String)` variant. A typed match without an arm for a delivered
event, an unrecognized variant, or a guard fails the build with
`file:line:column`.

JSON parsing and `serde_json::Value`, nested assignments, common iterators,
string and numeric methods, format specifiers, character-set trimming, string
slices, blocks and loops are translated. Match guards read their pattern
bindings, nested `or` patterns and constant patterns compare directly, and a
crate function passed as a callback value, such as `map(lua_text)`, is inlined
at the call.

Host callbacks use stable names. In `auto` and `off`, `Op::run`, `Op::run_stream`,
`Op::http`, and `Op::state_watch` emit `{ __host_callback = "name" }`, and the
runtime delivers that name to the entry's `onHostCallback(payload)` global, so a
compiled `host_callback` branch sees the same payload the Rust handler does. In
`strict`, those ops emit real closures and named plugin globals. The script
contains no marker table and no `onHostCallback` global.

`ctx.run_async_start`, `ctx.run_stream_start`, and `ctx.http_start` compile in
`auto` when their acceptance result is dropped, because compiled Luau starts the
call without blocking. In `strict` the real boolean result can be used in any
expression position.

Arbitrary trait iterators, dynamic dispatch, and trait methods without a Luau
equivalent are never approximated. In `auto`, a function whose signature can
cross JSON runs as a native call in the helper and the rest of the handler still
compiles. A function whose signature cannot cross JSON, or a handler body that
cannot be split, keeps the whole-entry bridge. In `strict`, those constructs
fail the build with `file:line:column`.

In `strict`, `Op::query` is resolved statically. A literal host root and path
become a direct API 57 call, and the handler's response path receives the result
envelope through `noctalia.writeFile`. A dynamic root, a dynamic path, or an
unknown member fails the build with `file:line:column`. `auto` rejects a compiled
`Op::query`, so that function stays native or the entry keeps its bridge. `off`
skips the compiler.

At 0.4.0 the 1,067-entry community corpus has 316 strict entries with 316/316
scripts valid, 1,067 auto entries with 332/332 scripts valid, and 1,067 off
entries. `auto` mixes fully compiled, hybrid, and bridged entries. `strict`
emits no bridge for any script it translates. [BASELINE.md](../BASELINE.md)
records the per-stage adoption, the performance table, and the source-change
classes that still need a plugin edit.
