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
- Version taken from `Cargo.toml` through `CARGO_PKG_VERSION`
- No Nix requirement; Nix packaging stays optional
- All Noctalia setting types and entry kinds
- Plugin-wide and entry-scoped settings
- Entry metadata for panels, launchers, widgets, and layers
- Assets and extra translation files emitted into the plugin directory
- Typed ops for common host calls and a generic `HostCall` escape hatch
- Host reads through `Context`

## Cargo setup

```toml
[dependencies]
noctalia-rs = "0.1"
serde_json = "1.0"

[build-dependencies]
noctalia-rs-build = "0.1"
```

For entries that keep their logic in `.luau` files, add `noctalia-rs-luau = "0.1"` to `[dependencies]`. Rust handlers do not need it.
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

Each entry names a handler function in your crate. The generated plugin takes
its version from `Cargo.toml`.

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
`--helper` for an existing helper binary, and `--help` for all options. It uses
its own scratch files and never reads the Noctalia config.

CI runs the same harness three times on every pull request and publishes a
`Performance audit` check and comment. The check compares the best round with a
ceiling re-measured in the same session from the `floors` branch, and with each
workload's budget. A workload fails when its median, p95, or p99 is more than
10% slower than the reference worst round, or by more than either side's spread
between rounds, whichever is larger. It also fails when its median is more than
10% over budget while the reference is within budget. Budgets only tighten. Pushes
to main record a new ceiling. The benchmark scripts live in `.github/scripts`.
