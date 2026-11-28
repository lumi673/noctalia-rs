use crate::{EntryKind, NativeMode, Options, StrictOptions, compile};

use super::source;

fn compile_config(handler: &str, strict: bool) -> Result<String, crate::Error> {
    let config = "local CONFIG = {\n  [\"message\"] = nil,\n  [\"enabled\"] = nil,\n  [\"count\"] = nil,\n  [\"scale\"] = nil,\n}\n";
    let translations = "local LANGUAGES = {}\nlocal TRANSLATIONS = {}\n";
    let assets = Vec::new();
    let keys = ["message", "enabled", "count", "scale"];
    let opts = Options {
        handler: "handler",
        entry: "widget",
        state_key: "hello.state",
        kind: EntryKind::Widget,
        strings: &keys,
        settings: &keys,
        module: "main",
        modules: &[],
        native: NativeMode::Deny,
        strict: strict.then_some(StrictOptions {
            config,
            translations,
            assets: &assets,
        }),
    };
    compile(&source(handler), &opts).map(|compiled| compiled.source)
}

const DERIVE: &str = r#"
use noctalia_rs::Config;

#[derive(Config)]
struct Settings {
    #[config(key = "message", default = "hello")]
    message: String,
    #[config(key = "enabled", default = false)]
    enabled: bool,
    #[config(key = "count", default = 3)]
    count: u32,
    #[config(key = "scale", default = 1.5)]
    scale: f64,
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let settings = Settings::from_ctx(ctx);
    Ok(vec![Op::log(settings.message)])
}
"#;

#[test]
fn config_derive_reads_the_strict_table_in_field_order() {
    let output = compile_config(DERIVE, true).expect("strict compile");

    assert!(!output.contains("noctalia.getConfig"), "{output}");
    assert!(output.contains("CONFIG[\"message\"]"), "{output}");
    assert!(output.contains("CONFIG[\"enabled\"]"), "{output}");
    assert!(output.contains("CONFIG[\"count\"]"), "{output}");
    assert!(output.contains("CONFIG[\"scale\"]"), "{output}");
    let keys = [
        "CONFIG[\"message\"]",
        "CONFIG[\"enabled\"]",
        "CONFIG[\"count\"]",
        "CONFIG[\"scale\"]",
    ];
    let positions = keys
        .iter()
        .map(|key| {
            output
                .find(key)
                .unwrap_or_else(|| panic!("missing {key} in {output}"))
        })
        .collect::<Vec<_>>();
    assert!(
        positions.windows(2).all(|pair| pair[0] < pair[1]),
        "config keys read out of order: {positions:?}"
    );
    assert!(
        output.contains("type(") && output.contains("enabled = false"),
        "{output}"
    );
}

#[test]
fn config_derive_checks_signed_integer_range() {
    let handler = DERIVE.replace("count: u32", "count: i32").replace(
        "#[config(key = \"count\", default = 3)]",
        "#[config(key = \"count\", default = -3)]",
    );
    let output = compile_config(&handler, true).expect("strict compile");
    assert!(output.contains("-2147483648"), "{output}");
    assert!(output.contains("2147483647"), "{output}");
    assert!(output.contains("count = -3"), "{output}");
}

#[test]
fn config_derive_checks_integer_width_and_signedness() {
    let output = compile_config(DERIVE, true).expect("strict compile");
    assert!(output.contains("4294967295"), "{output}");
    assert!(output.contains("< 0"), "{output}");
    assert!(output.contains("% 1 ~= 0"), "{output}");
    assert!(
        output.contains("tonumber(") && output.contains("CONFIG[\"scale\"]"),
        "{output}"
    );
}

#[test]
fn config_derive_uses_the_host_config_outside_strict() {
    let output = compile_config(DERIVE, false).expect("auto compile");
    assert!(
        output.contains("noctalia.getConfig(\"message\")"),
        "{output}"
    );
    assert!(!output.contains("CONFIG[\"message\"]"), "{output}");
}

#[test]
fn config_derive_rejects_an_unsupported_integer_width() {
    let handler = DERIVE.replace("count: u32", "count: i64");
    let error = compile_config(&handler, true).expect_err("i64 is not exact");
    let rendered = error.render("main.rs");
    assert!(rendered.starts_with("main.rs:"), "{rendered}");
    assert!(rendered.contains("unsupported config type"), "{rendered}");
}

#[test]
fn config_derive_rejects_a_missing_default() {
    let handler = DERIVE.replace(
        "#[config(key = \"count\", default = 3)]\n    count: u32,",
        "count: u32,",
    );
    let error = compile_config(&handler, true).expect_err("missing default");
    let rendered = error.render("main.rs");
    assert!(rendered.starts_with("main.rs:"), "{rendered}");
    assert!(rendered.contains("needs #[config(default"), "{rendered}");
}

#[test]
fn config_derive_requires_the_handler_context() {
    let handler = DERIVE
        .replace("Settings::from_ctx(ctx)", "Settings::from_ctx(&ctx.config)");
    let error = compile_config(&handler, true).expect_err("not a context");
    let rendered = error.render("main.rs");
    assert!(rendered.starts_with("main.rs:"), "{rendered}");
    assert!(
        rendered.contains("takes the handler's &Context"),
        "{rendered}"
    );
}

#[test]
fn config_derive_default_matches_the_declared_values() {
    let handler = DERIVE.replace(
        "let settings = Settings::from_ctx(ctx);",
        "let settings = Settings::default();",
    );
    let output = compile_config(&handler, true).expect("default compile");
    assert!(output.contains("message = \"hello\""), "{output}");
    assert!(output.contains("enabled = false"), "{output}");
    assert!(output.contains("count = 3"), "{output}");
    assert!(output.contains("scale = 1.5"), "{output}");
}

#[test]
fn strict_config_reads_use_the_build_table() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let message = ctx.config_str("message").unwrap_or_default();
    let enabled = ctx.config_bool("enabled", true);
    let value = ctx.config.get("count");
    let indexed = ctx.config["scale"];
    let _ = (value, indexed);
    Ok(vec![Op::log(format!("{message}{enabled}"))])
}
"#;
    let output = compile_config(handler, true).expect("strict compile");
    assert!(!output.contains("noctalia.getConfig"), "{output}");
    assert!(output.contains("CONFIG[\"message\"]"), "{output}");
    assert!(
        output.contains("bool_or(CONFIG[\"enabled\"], true)"),
        "{output}"
    );
    assert!(output.contains("CONFIG[\"count\"]"), "{output}");
    assert!(output.contains("CONFIG[\"scale\"]"), "{output}");
}

#[test]
fn auto_config_reads_keep_the_host_call() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let message = ctx.config_str("message").unwrap_or_default();
    Ok(vec![Op::log(message)])
}
"#;
    let output = compile_config(handler, false).expect("auto compile");
    assert!(
        output.contains("noctalia.getConfig(\"message\")"),
        "{output}"
    );
    assert!(!output.contains("CONFIG[\"message\"]"), "{output}");
}

#[test]
fn strict_config_reads_reject_dynamic_keys() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let key = "message";
    let value = ctx.config.get(key);
    let _ = value;
    Ok(Vec::new())
}
"#;
    let error = compile_config(handler, true).expect_err("dynamic key");
    let rendered = error.render("main.rs");
    assert!(rendered.starts_with("main.rs:"), "{rendered}");
    assert!(rendered.contains("strict config keys"), "{rendered}");
}
