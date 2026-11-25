//! The native boundary as emitted into a real plugin script.
//!
//! These tests compile a hybrid handler and drive the generated Luau through
//! `noctalia_rs_luau::dispatch`, standing in for the host: the ready line and
//! the native result are delivered to the registered stream callback, exactly
//! as Noctalia delivers them.

use std::path::PathBuf;

use noctalia_rs::{Context, Parts};
use noctalia_rs_compiler::{
    EntryKind, NativeMode, Options, compile as compile_rust,
};
use serde_json::{Value, json};

fn context(entry: &str, event: &str, payload: Value) -> Context {
    Context::new(
        Parts {
            entry: entry.to_owned(),
            event: event.to_owned(),
            payload,
            config: json!({}),
            state: json!({}),
            host: json!({}),
            data_dir: PathBuf::from("/nonexistent-noctalia-native"),
        },
        json!({}),
    )
}

fn compile(entry: &str, handler: &str) -> String {
    compile_kind(entry, EntryKind::Widget, handler)
}

fn compile_kind(entry: &str, kind: EntryKind, handler: &str) -> String {
    let source = format!(
        "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::json;\n\n{handler}\n"
    );
    let compiled = compile_rust(
        &source,
        &Options {
            handler: "handler",
            entry,
            state_key: "native.state",
            kind,
            strings: &[],
            settings: &[],
            module: "main",
            modules: &[],
            native: NativeMode::Allow,
        },
    )
    .expect("compile")
    .source;
    assert!(compiled.contains("__native.call"), "{compiled}");
    compiled.replace("{{HELPER}}", "noctalia-native-helper")
}

#[allow(clippy::too_many_arguments)]
fn dispatch(
    entry: &str,
    source: &str,
    event: &str,
    payload: Value,
) -> Vec<Value> {
    noctalia_rs_luau::dispatch(
        &context(entry, event, payload),
        &[(entry, source)],
    )
    .expect("dispatch")
    .iter()
    .map(|op| serde_json::to_value(op).expect("serialize"))
    .collect()
}

fn stream_id(values: &[Value]) -> String {
    values
        .iter()
        .find(|value| {
            value.get("op").and_then(Value::as_str) == Some("host_call")
                && value["path"][0].as_str() == Some("runStream")
        })
        .expect("the script starts the helper stream")["args"][1]["__host_callback"]
        .as_str()
        .expect("callback id")
        .to_owned()
}

fn line(stream: &str, message: Value) -> Value {
    // The host delivers stream lines as JSON text, not as a table.
    json!({ "id": stream, "args": [message.to_string()] })
}

fn spool_writes(values: &[Value]) -> usize {
    values
        .iter()
        .filter(|value| {
            value.get("op").and_then(Value::as_str) == Some("log")
                && value.get("message").and_then(Value::as_str).is_some_and(
                    |message| message.contains("spool write failed"),
                )
        })
        .count()
}

fn bar_text(values: &[Value]) -> Option<String> {
    values.iter().find_map(|value| {
        (value.get("op").and_then(Value::as_str) == Some("bar_text")).then(
            || {
                value
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            },
        )
    })
}

const TWO_CACHED_CALLS: &str = r#"
#[native(cache_ms = 60_000)]
fn rows() -> Result<Vec<String>> {
    let text = std::fs::read_link("/tmp/rows")?.to_string_lossy().into_owned();
    Ok(vec![text])
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let first = rows()?;
    let second = rows()?;
    Ok(vec![Op::BarText { text: format!("{}|{}", first.join("-"), second.join("-")) }])
}
"#;

#[test]
fn a_cached_native_call_resumes_in_one_round_trip() {
    // The same arguments twice: the second call reads the in-script cache, so a
    // single native result is enough to finish the handler. If the cache missed,
    // the body would yield again and no bar text would be emitted yet.
    let source = compile("cached", TWO_CACHED_CALLS);

    let parked = dispatch("cached", &source, "render", json!({}));
    assert!(bar_text(&parked).is_none(), "{parked:?}");
    let stream = stream_id(&parked);

    let ready = dispatch(
        "cached",
        &source,
        "host_callback",
        line(
            &stream,
            json!({ "control": "ready", "boot": "b1", "spool": "/tmp/x" }),
        ),
    );
    assert!(
        ready
            .iter()
            .all(|value| bar_text(std::slice::from_ref(value)).is_none()),
        "{ready:?}"
    );
    // The load-time invocation parked before the helper answered, so the ready
    // line flushes exactly one request. A second write would run the function
    // twice for every hybrid entry's first call.
    assert_eq!(spool_writes(&ready), 1, "{ready:?}");

    let resumed = dispatch(
        "cached",
        &source,
        "host_callback",
        line(
            &stream,
            json!({ "control": "native_result", "id": "000000000001", "value": ["a", "b"], "error": null }),
        ),
    );
    assert_eq!(
        bar_text(&resumed).as_deref(),
        Some("a-b|a-b"),
        "{resumed:?}"
    );
}

const TWO_UNCACHED_CALLS: &str = r#"
#[native]
fn rows() -> Result<Vec<String>> {
    let text = std::fs::read_link("/tmp/rows")?.to_string_lossy().into_owned();
    Ok(vec![text])
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let first = rows()?;
    let second = rows()?;
    Ok(vec![Op::BarText { text: format!("{}|{}", first.join("-"), second.join("-")) }])
}
"#;

#[test]
fn an_uncached_native_call_always_takes_a_round_trip() {
    let source = compile("uncached", TWO_UNCACHED_CALLS);

    let parked = dispatch("uncached", &source, "render", json!({}));
    let stream = stream_id(&parked);
    dispatch(
        "uncached",
        &source,
        "host_callback",
        line(
            &stream,
            json!({ "control": "ready", "boot": "b1", "spool": "/tmp/x" }),
        ),
    );

    let first = dispatch(
        "uncached",
        &source,
        "host_callback",
        line(
            &stream,
            json!({ "control": "native_result", "id": "000000000001", "value": ["a"], "error": null }),
        ),
    );
    assert!(bar_text(&first).is_none(), "{first:?}");

    let second = dispatch(
        "uncached",
        &source,
        "host_callback",
        line(
            &stream,
            json!({ "control": "native_result", "id": "000000000002", "value": ["b"], "error": null }),
        ),
    );
    assert_eq!(bar_text(&second).as_deref(), Some("a|b"), "{second:?}");
}

const FAILING_CALL: &str = r#"
#[native]
fn rows() -> Result<Vec<String>> {
    let text = std::fs::read_link("/tmp/rows")?.to_string_lossy().into_owned();
    Ok(vec![text])
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let first = rows()?;
    Ok(vec![Op::BarText { text: first.join("-") }])
}
"#;

#[test]
fn a_native_error_surfaces_as_a_lua_error() {
    let source = compile("failing", FAILING_CALL);
    let parked = dispatch("failing", &source, "render", json!({}));
    let stream = stream_id(&parked);

    let error = noctalia_rs_luau::dispatch(
        &context(
            "failing",
            "host_callback",
            line(
                &stream,
                json!({
                    "control": "native_result",
                    "id": "000000000001",
                    "value": null,
                    "error": "permission denied"
                }),
            ),
        ),
        &[("failing", source.as_str())],
    )
    .expect_err("the native error must reach the caller");
    assert!(error.to_string().contains("permission denied"), "{error}");
}

const QUEUED_EVENT: &str = r#"
#[native]
fn rows() -> Result<Vec<String>> {
    let text = std::fs::read_link("/tmp/rows")?.to_string_lossy().into_owned();
    Ok(vec![text])
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let first = rows()?;
    if ctx.event == "click" {
        Ok(vec![Op::BarText { text: format!("click|{}", first.join("-")) }])
    } else {
        Ok(vec![Op::BarText { text: format!("render|{}", first.join("-")) }])
    }
}
"#;

#[test]
fn one_event_while_parked_is_queued_and_drained_in_order() {
    let source = compile_kind("queued", EntryKind::Service, QUEUED_EVENT);
    let parked = dispatch("queued", &source, "update", json!({}));
    let stream = stream_id(&parked);
    dispatch(
        "queued",
        &source,
        "host_callback",
        line(
            &stream,
            json!({ "control": "ready", "boot": "b1", "spool": "/tmp/x" }),
        ),
    );

    // A click arrives while the render invocation is parked.
    let queued = dispatch("queued", &source, "click", json!({}));
    assert!(bar_text(&queued).is_none(), "{queued:?}");

    // A service renders once on load, so the first result resumes that
    // invocation, the second resumes the queued update, and the third the click.
    let load = dispatch(
        "queued",
        &source,
        "host_callback",
        line(
            &stream,
            json!({ "control": "native_result", "id": "000000000001", "value": ["load"], "error": null }),
        ),
    );
    assert_eq!(bar_text(&load).as_deref(), Some("render|load"), "{load:?}");

    let update = dispatch(
        "queued",
        &source,
        "host_callback",
        line(
            &stream,
            json!({ "control": "native_result", "id": "000000000002", "value": ["update"], "error": null }),
        ),
    );
    assert_eq!(
        bar_text(&update).as_deref(),
        Some("render|update"),
        "{update:?}"
    );

    let click = dispatch(
        "queued",
        &source,
        "host_callback",
        line(
            &stream,
            json!({ "control": "native_result", "id": "000000000003", "value": ["click"], "error": null }),
        ),
    );
    assert_eq!(
        bar_text(&click).as_deref(),
        Some("click|click"),
        "{click:?}"
    );
}

const CONTEXT_CALL: &str = r#"
fn describe(ctx: &Context) -> Result<String> {
    let text = std::fs::read_link("/tmp/rows")?.to_string_lossy().into_owned();
    Ok(format!("{}:{text}", ctx.event))
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::BarText { text: describe(ctx)? }])
}
"#;

#[test]
fn a_native_function_that_takes_the_context_resumes() {
    let source = compile("context", CONTEXT_CALL);
    assert!(
        source.contains("__native.call(\"main::describe\", {}, 0, true, true)"),
        "{source}"
    );
    assert!(
        source.contains("local function snapshot(descriptor)"),
        "{source}"
    );

    let parked = dispatch("context", &source, "render", json!({}));
    let stream = stream_id(&parked);
    let ready = dispatch(
        "context",
        &source,
        "host_callback",
        line(
            &stream,
            json!({ "control": "ready", "boot": "b1", "spool": "/tmp/x" }),
        ),
    );
    // The request write goes through the host, which the test does not provide,
    // but building the context snapshot must not raise.
    assert!(bar_text(&ready).is_none(), "{ready:?}");

    let resumed = dispatch(
        "context",
        &source,
        "host_callback",
        line(
            &stream,
            json!({ "control": "native_result", "id": "000000000001", "value": "render:rows", "error": null }),
        ),
    );
    assert_eq!(
        bar_text(&resumed).as_deref(),
        Some("render:rows"),
        "{resumed:?}"
    );
}
