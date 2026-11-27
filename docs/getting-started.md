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
