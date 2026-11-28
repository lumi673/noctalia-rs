use crate::{EntryKind, StrictOptions, compile};

use super::{options, source, strict_options};

const HANDLER: &str = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        let timeout = ctx.set_timeout(1500, "finish");
        let interval = ctx.set_interval(250, "tick");
        if ctx.event == "cancel" {
            ctx.cancel_timeout(timeout);
            ctx.cancel_interval(interval);
        }
        return Ok(vec![Op::set_interval(1000)]);
    }
    if ctx.event == "host_callback" {
        let id = ctx.payload_str("id").unwrap_or_default();
        return Ok(vec![Op::log(id)]);
    }
    Ok(Vec::new())
}
"#;

fn compile_mode(handler: &str, strict: bool) -> Result<String, String> {
    let assets: Vec<(&str, u32)> = Vec::new();
    let options = if strict {
        strict_options(
            "handler",
            EntryKind::Service,
            StrictOptions {
                config: "local CONFIG = {}\n",
                translations: "local LANGUAGES = {}\nlocal TRANSLATIONS = {}\n",
                assets: &assets,
            },
        )
    } else {
        options("handler", EntryKind::Service)
    };
    compile(&source(handler), &options)
        .map(|compiled| compiled.source)
        .map_err(|error| error.render("timers.rs"))
}

#[test]
fn strict_timers_use_real_callbacks_and_a_local_tick() {
    let output = compile_mode(HANDLER, true).expect("strict compile");
    assert!(output.contains("local function __noctalia_timer_tick(force)"));
    assert!(output.contains("noctalia.setUpdateInterval(target)"));
    assert!(output.contains("__noctalia_timer_base(1000)"));
    assert!(output.contains("function update() __noctalia_timer_tick(false);"));
    assert!(output.contains("item[2]()"));
    assert!(
        output.contains(
            "__noctalia_timer_start(__t1, (function(__captured) return function(...) __strict_callback(__captured, ...) end end)(__t2), false, \"main:7:27\")"
        ),
        "{output}"
    );
    assert!(output.contains("local __t2 = \"finish\""), "{output}");
    for forbidden in [
        "onHostCallback",
        "__host_callback",
        "__call =",
        "lib/bridge",
    ] {
        assert!(!output.contains(forbidden), "{forbidden} in {output}");
    }
}

#[test]
fn auto_timers_dispatch_by_callback_name() {
    let output = compile_mode(HANDLER, false).expect("auto compile");
    assert!(
        output.contains(
            "__noctalia_timer_start(__t1, __t2, false, \"main:7:27\")"
        )
    );
    assert!(output.contains("dispatch({ id = item[2], args = {} })"));
    assert!(output.contains("function onHostCallback(payload)"));
    assert!(!output.contains("__strict_callback"));
}

#[test]
fn strict_timers_reject_a_large_literal_at_the_span() {
    let handler = HANDLER.replace(
        "ctx.set_timeout(1500, \"finish\")",
        "ctx.set_timeout(18446744073709551615u64, \"finish\")",
    );
    let error = compile_mode(&handler, true).expect_err("large delay");
    assert!(error.contains("timers.rs:"), "{error}");
    assert!(error.contains("exact Luau integer range"), "{error}");
}

#[test]
fn strict_timers_reject_a_callback_marker() {
    let handler = HANDLER.replace(
        "ctx.set_timeout(1500, \"finish\")",
        "ctx.set_timeout(1500, noctalia_rs::ui::callback(\"finish\"))",
    );
    let error = compile_mode(&handler, true).expect_err("marker callback");
    assert!(error.contains("timers.rs:"), "{error}");
    assert!(error.contains("pass the callback id string"), "{error}");
}

#[test]
fn strict_timers_need_a_host_callback_branch() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "update" {
        ctx.set_timeout(1500, "finish");
    }
    Ok(Vec::new())
}
"#;
    let error = compile_mode(handler, true).expect_err("missing branch");
    assert!(error.contains("timers.rs:"), "{error}");
    assert!(error.contains("host_callback branch"), "{error}");
}

#[test]
fn strict_rejects_a_zero_interval_period() {
    let handler = HANDLER.replace(
        "ctx.set_interval(250, \"tick\")",
        "ctx.set_interval(0, \"tick\")",
    );
    let error = compile_mode(&handler, true).expect_err("zero interval");
    assert!(error.contains("timers.rs:"), "{error}");
    assert!(error.contains("greater than zero"), "{error}");
}
