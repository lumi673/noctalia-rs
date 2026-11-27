# Parity checks

`crates/lib/tests/parity.rs` checks that the generated API covers the setting
types, entry kinds, and manifest fields used by community plugins. Without
configuration it uses an embedded fixture. Set `NOCTALIA_PARITY_FIXTURES` to a
checkout of a plugin collection to check every real manifest.
