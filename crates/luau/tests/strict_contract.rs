//! Contract checks for the strict Luau emitter.
//!
//! Strict emit has no bridge and no helper, so it must pass real closures
//! where plugin API 57 takes a function, and it must not define
//! `onHostCallback`. Auto keeps the marker tables that `bridge.luau`
//! materializes.

use noctalia_rs_compiler::{EntryKind, NativeMode, Options, compile};

fn strict_source(handler: &str) -> String {
    let source = format!(
        "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::json;\n\n{handler}\n"
    );
    compile(
        &source,
        &Options {
            handler: "handler",
            entry: "strict",
            state_key: "strict.state",
            kind: EntryKind::Service,
            strings: &[],
            settings: &[],
            module: "main",
            modules: &[],
            native: NativeMode::Deny,
        },
    )
    .expect("compile")
    .source
}

fn assert_strict_contract(source: &str) {
    for forbidden in [
        "__host_callback",
        "__call",
        "onHostCallback",
        "bridge.luau",
        "lib/bridge",
    ] {
        assert!(
            !source.contains(forbidden),
            "strict emit contains `{forbidden}`:\n{source}"
        );
    }
}

#[test]
#[ignore = "phase B: remove when strict emission stops using marker tables"]
fn strict_callback_emit_uses_real_closures() {
    let source = strict_source(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        ctx.http_start(json!({ "url": "https://example.invalid/" }), "onResponse");
    }
    Ok(Vec::new())
}
"#,
    );
    assert_strict_contract(&source);
}

#[test]
#[ignore = "phase B: remove when strict emission stops using marker tables"]
fn strict_op_emit_uses_no_marker_tables() {
    let source = strict_source(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::run_stream(json!(["tail", "-f"]), "onLine")])
}
"#,
    );
    assert_strict_contract(&source);
}
