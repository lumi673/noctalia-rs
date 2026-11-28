use std::path::PathBuf;

use noctalia_rs::Context;
use noctalia_rs::compat::Parts;
use noctalia_rs_compiler::{
    EntryKind, NativeMode, Options, StrictOptions, compile,
};
use serde_json::{Value, json};

const CONFIG: &str = "local CONFIG = {}\n";
const TRANSLATIONS: &str = "local LANGUAGES = {}\nlocal TRANSLATIONS = {}\n";

const TIMERS: &str = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "load" {
        ctx.set_timeout(1500, "finish");
        ctx.set_interval(250, "tick");
        return Ok(vec![Op::set_interval(1000)]);
    }
    if ctx.event == "host_callback" {
        let id = ctx.payload_str("id").unwrap_or_default();
        return Ok(vec![Op::log(id)]);
    }
    Ok(Vec::new())
}
"#;

const ZERO_CANCEL: &str = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "load" {
        let handle = ctx.set_timeout(0, "now");
        ctx.cancel_timeout(handle);
        return Ok(Vec::new());
    }
    if ctx.event == "host_callback" {
        let id = ctx.payload_str("id").unwrap_or_default();
        return Ok(vec![Op::log(id)]);
    }
    Ok(Vec::new())
}
"#;

const DYNAMIC_OVERFLOW: &str = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        let delay = 18446744073709551615u64;
        ctx.set_timeout(delay, "late");
    }
    if ctx.event == "host_callback" {
        return Ok(vec![Op::log("callback")]);
    }
    Ok(Vec::new())
}
"#;

const CANCEL: &str = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "load" {
        let handle = ctx.set_timeout(500, "late");
        ctx.cancel_timeout(handle);
        return Ok(Vec::new());
    }
    if ctx.event == "host_callback" {
        let id = ctx.payload_str("id").unwrap_or_default();
        return Ok(vec![Op::log(id)]);
    }
    Ok(Vec::new())
}
"#;

fn compiled(handler: &str, strict: bool) -> String {
    let source =
        format!("use noctalia_rs::{{Context, Op, Result}};\n\n{handler}\n");
    let assets: Vec<(&str, u32)> = Vec::new();
    compile(
        &source,
        &Options {
            handler: "handler",
            entry: "timers",
            state_key: "timers.state",
            kind: EntryKind::Service,
            strings: &[],
            settings: &[],
            module: "main",
            modules: &[],
            native: NativeMode::Deny,
            strict: strict.then_some(StrictOptions {
                config: CONFIG,
                translations: TRANSLATIONS,
                assets: &assets,
            }),
        },
    )
    .expect("compile")
    .source
}
#[expect(
    clippy::too_many_arguments,
    reason = "one dispatch carries the source, mode, event and clock"
)]
fn run(source: &str, strict: bool, event: &str, now_ms: i64) -> Vec<Value> {
    let context = Context::new(
        Parts {
            entry: "timers".to_owned(),
            event: event.to_owned(),
            payload: json!({}),
            config: json!({}),
            state: json!({}),
            host: json!({ "now_ms": now_ms }),
            data_dir: PathBuf::from("/tmp/noctalia-timers"),
        },
        json!({}),
    );
    let result = if strict {
        noctalia_rs_luau::dispatch_strict(&context, &[("timers", source)])
    } else {
        noctalia_rs_luau::dispatch(&context, &[("timers", source)])
    };
    result
        .unwrap_or_else(|error| panic!("dispatch {event}: {error}"))
        .iter()
        .map(|op| serde_json::to_value(op).expect("serialize op"))
        .collect()
}

fn tick(source: &str, strict: bool, now_ms: i64) -> Vec<Value> {
    run(source, strict, "update", now_ms)
}

fn messages(values: &[Value]) -> Vec<String> {
    values
        .iter()
        .filter(|value| value["op"] == json!("log"))
        .map(|value| value["message"].as_str().unwrap_or_default().to_owned())
        .collect()
}

fn intervals(values: &[Value]) -> Vec<u64> {
    values
        .iter()
        .filter(|value| value["op"] == json!("set_interval"))
        .map(|value| value["ms"].as_u64().unwrap_or_default())
        .collect()
}

#[test]
fn strict_timers_fire_once_and_repeat_from_the_update_tick() {
    let source = compiled(TIMERS, true);
    let startup = run(&source, true, "update", 1000);
    assert_eq!(intervals(&startup), [250], "{startup:?}");
    assert!(messages(&startup).is_empty(), "{startup:?}");

    let before = tick(&source, true, 1100);
    assert!(messages(&before).is_empty(), "{before:?}");

    let first_tick = tick(&source, true, 1300);
    assert_eq!(messages(&first_tick), ["tick"], "{first_tick:?}");

    let both = tick(&source, true, 2600);
    assert_eq!(messages(&both), ["finish", "tick"], "{both:?}");
}

#[test]
fn auto_timers_match_strict_callbacks_and_ticks() {
    let strict = compiled(TIMERS, true);
    let auto = compiled(TIMERS, false);
    let strict_start = run(&strict, true, "load", 1000);
    let auto_start = run(&auto, false, "load", 1000);
    assert_eq!(messages(&strict_start), messages(&auto_start), "startup");
    assert_eq!(intervals(&strict_start), intervals(&auto_start), "startup");
    for now in [1100, 1300, 2600] {
        let strict_ops = tick(&strict, true, now);
        let auto_ops = tick(&auto, false, now);
        assert_eq!(messages(&strict_ops), messages(&auto_ops), "at {now}");
        assert_eq!(intervals(&strict_ops), intervals(&auto_ops), "at {now}");
    }
}

#[test]
fn a_cancelled_timeout_never_fires() {
    for strict in [true, false] {
        let source = compiled(CANCEL, strict);
        let startup = run(&source, strict, "update", 1000);
        assert!(messages(&startup).is_empty(), "{startup:?}");
        let later = tick(&source, strict, 2000);
        assert!(messages(&later).is_empty(), "{later:?}");
    }
}

#[test]
fn a_zero_delay_timeout_can_be_cancelled_before_the_next_tick() {
    for strict in [true, false] {
        let source = compiled(ZERO_CANCEL, strict);
        let startup = run(&source, strict, "load", 1000);
        assert!(messages(&startup).is_empty(), "{startup:?}");
        let later = tick(&source, strict, 1001);
        assert!(messages(&later).is_empty(), "{later:?}");
    }
}

#[test]
fn a_dynamic_delay_failure_names_the_source_span() {
    let source = compiled(DYNAMIC_OVERFLOW, true);
    let error = noctalia_rs_luau::dispatch_strict(
        &Context::new(
            Parts {
                entry: "timers".to_owned(),
                event: "update".to_owned(),
                payload: json!({}),
                config: json!({}),
                state: json!({}),
                host: json!({ "now_ms": 1000 }),
                data_dir: PathBuf::from("/tmp/noctalia-timers-overflow"),
            },
            json!({}),
        ),
        &[("timers", source.as_str())],
    )
    .expect_err("dynamic overflow");
    let error = error.to_string();
    assert!(
        error.contains("timer delay is outside the exact Luau integer range"),
        "{error}"
    );
    assert!(error.contains("main:"), "{error}");
}
