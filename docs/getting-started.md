# Getting started

## Cargo setup

Add the dependencies to `Cargo.toml`:

```toml
[dependencies]
noctalia-rs = "0.4"
serde_json = "1.0"

[build-dependencies]
noctalia-rs-build = "0.4"
```

For Rust handlers, the dependency, `build.rs`, and `noctalia.toml` shown below
are enough. Entries with a hand-written `.luau` script and no `handler` also
need `noctalia-rs-luau = "0.4"` in `[dependencies]`.

Package names and Rust library names match. Plugin code imports `noctalia_rs::`,
and `build.rs` calls `noctalia_rs_build::generate()`.

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

[[setting]]
key = "refresh_ms"
type = "int"
label_key = "settings.refresh_ms.label"
default = 1000

[[service]]
id = "service"
entry = "service.luau"
handler = "service"
```

Each entry names a handler function in your crate. The build finds that function
in the crate source. The generated plugin takes its version from `Cargo.toml`.

Declare non-code files with `assets`. Shared Luau modules go in `luau_sources`.
Entries without `handler` embed the script named in `entry` automatically. Only
declared files are copied into the plugin directory.
Both are root-level keys, so put them before the first `[[...]]` table.

```toml
assets = ["icons/clock.svg"]
luau_sources = ["lib/util.luau"]
```

Put the strings for `label_key` and `description_key` in
`translations/en.json` or another language file.

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

## Typed config

Declare a struct with `#[derive(Config)]` to read settings by field instead of by
key. Each field needs a literal default that matches the `[[setting]]` default in
`noctalia.toml`. The build checks the key, setting type, and default before it
compiles the plugin.
Declare those settings at plugin level so every strict entry receives them.

```rust
use noctalia_rs::{Config, Context, Op, Result};

#[derive(Config)]
struct Settings {
    #[config(key = "message", default = "Hello")]
    message: String,
    #[config(key = "refresh_ms", default = 1000)]
    refresh_ms: u32,
}

fn service(ctx: &Context) -> Result<Vec<Op>> {
    let settings = Settings::from_ctx(ctx);
    Ok(vec![Op::set_interval(1000), Op::log(format!("{} {}", settings.message, settings.refresh_ms))])
}
```

Supported field types are `String`, `bool`, `i32`, `u32`, and `f64`. Strict
builds read the generated `CONFIG` table directly. Auto and off builds keep the
host config path.

## Build and install

```sh
cargo build --release
mkdir dist
cp target/release/my-plugin dist/my-helper
./target/release/my-plugin emit dist my-helper
```

Add `dist` as a local plugin source in Noctalia and enable the plugin id.
Replace `my-plugin` with your crate name.

The `template/` crate in this workspace is a complete minimal example.
