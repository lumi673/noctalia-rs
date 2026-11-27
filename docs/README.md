# Documentation

- [Getting started](getting-started.md): dependencies, manifest, handlers, assets, translations, and building a plugin.
- [Compile modes](compile-modes.md): `strict`, `auto`, and `off`, plus compiler coverage.
- [Native functions](native-functions.md): the native split, attributes, signatures, context, calling behavior, and caching.
- [Nix packaging](nix-packaging.md): optional Nix build and helper placement.
- [Parity checks](parity-checks.md): API and manifest coverage tests.
- [Performance harness](performance.md): benchmarks and the CI performance audit.

## Capabilities

- `noctalia.toml` as the source of truth for metadata, settings, and entries
- Compiled Luau entries by default, with per-function native calls for arbitrary Rust and bridge fallback in `auto` mode
- Version taken from `Cargo.toml` through `CARGO_PKG_VERSION`
- No Nix requirement; Nix packaging stays optional
- All Noctalia setting types and entry kinds
- Plugin-wide and entry-scoped settings
- Entry metadata for panels, launchers, widgets, and layers
- Assets and extra translation files emitted into the plugin directory
- Typed ops for common host calls and a generic `HostCall` escape hatch
- Typed UI props (`Props`) that keep keys and static values borrowed instead of building a `serde_json` map per node
- Host reads through `Context`
- Event-driven spool dispatch, so idle helpers sleep instead of polling

## Parity contract

When a Luau plugin is translated to Rust, the Rust handler must preserve the
original algorithm exactly, including its complexity and habits. Performance
gains come from the generated Luau, bridge, and runtime, not from changing what
a plugin computes.
