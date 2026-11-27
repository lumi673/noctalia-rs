# NoctaliaRs

Unofficial project. It is not maintained by or affiliated with the Noctalia team.
Source: https://github.com/lumi673/noctalia-rs

Write Noctalia plugins in Rust. `noctalia.toml` describes the plugin. The build
emits `plugin.toml`, Luau entries, translations, and assets for Noctalia.

Why use it:

- One manifest for metadata, settings, and entries.
- Rust handlers can compile to Luau, so entries can run without the helper bridge.
- Typed host calls, context reads, and UI props.
- Nix packaging is optional.

## Install

Add the dependencies to `Cargo.toml`:

```toml
[dependencies]
noctalia-rs = "0.4"
serde_json = "1.0"

[build-dependencies]
noctalia-rs-build = "0.4"
```

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
its version from `Cargo.toml`. See [getting started](docs/getting-started.md)
for a handler example.

Build and install:

```sh
cargo build --release
mkdir dist
cp target/release/my-plugin dist/my-helper
./target/release/my-plugin emit dist my-helper
```

Add `dist` as a local plugin source in Noctalia and enable the plugin id. Replace
`my-plugin` with your crate name.

## More documentation

- [Getting started](docs/getting-started.md)
- [Compile modes](docs/compile-modes.md)
- [Native functions](docs/native-functions.md)
- [Nix packaging](docs/nix-packaging.md)
- [Parity checks](docs/parity-checks.md)
- [Performance harness](docs/performance.md)
