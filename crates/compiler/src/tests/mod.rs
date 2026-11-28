use crate::{EntryKind, Module, NativeMode, Options, compile};
mod calls;
mod constants;
mod expressions;
mod files;
mod inline;
mod macros;
mod methods;
mod native;
mod ops;
mod statements;
mod strict;
mod timers;
mod types;

fn options<'a>(handler: &'a str, kind: EntryKind) -> Options<'a> {
    Options {
        handler,
        entry: "widget",
        state_key: "hello.state",
        kind,
        strings: &["message", "name"],
        settings: &["message", "name"],
        module: "main",
        modules: &[],
        native: NativeMode::Deny,
        strict: None,
    }
}

fn source(handler: &str) -> String {
    format!(
        "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::json;\n\n{handler}\n"
    )
}

fn module_options<'a>(
    handler: &'a str,
    kind: EntryKind,
    modules: &'a [Module<'a>],
) -> Options<'a> {
    Options {
        handler,
        entry: "widget",
        state_key: "hello.state",
        kind,
        strings: &["message", "name"],
        settings: &["message", "name"],
        module: "main",
        modules,
        native: NativeMode::Deny,
        strict: None,
    }
}

fn strict_options<'a>(
    handler: &'a str,
    kind: EntryKind,
    strict: crate::StrictOptions<'a>,
) -> Options<'a> {
    Options {
        handler,
        entry: "widget",
        state_key: "hello.state",
        kind,
        strings: &["message", "name"],
        settings: &["message", "name"],
        module: "main",
        modules: &[],
        native: NativeMode::Deny,
        strict: Some(strict),
    }
}

fn native_options<'a>(handler: &'a str, kind: EntryKind) -> Options<'a> {
    Options {
        handler,
        entry: "widget",
        state_key: "hello.state",
        kind,
        strings: &["message", "name"],
        settings: &["message", "name"],
        module: "main",
        modules: &[],
        native: NativeMode::Allow,
        strict: None,
    }
}

const NATIVE_HELPER: &str = r#"
use noctalia_rs::{Context, Op, Result};

fn load_rows() -> Result<Vec<String>> {
    let text = std::fs::read_link("/tmp/rows")?.to_string_lossy().into_owned();
    Ok(vec![text])
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let rows = load_rows()?;
    Ok(vec![Op::log(rows.join(","))])
}
"#;

fn native_options_with<'a>(
    handler: &'a str,
    kind: EntryKind,
    modules: &'a [Module<'a>],
) -> Options<'a> {
    Options {
        handler,
        entry: "widget",
        state_key: "hello.state",
        kind,
        strings: &["message", "name"],
        settings: &["message", "name"],
        module: "main",
        modules,
        native: NativeMode::Allow,
        strict: None,
    }
}
