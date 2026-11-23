use std::path::PathBuf;

use noctalia::{Context, Parts};
use serde_json::{Value, json};

use crate::callback::{callback_args, callback_for_event};
use crate::dispatch;
use crate::host::Host;

struct Fixture<'a> {
    event: &'a str,
    payload: Value,
    config: Value,
    state: Value,
    host: Value,
}

fn context(fixture: Fixture<'_>) -> Context {
    Context::new(
        Parts {
            entry: "entry".to_owned(),
            event: fixture.event.to_owned(),
            payload: fixture.payload,
            config: fixture.config,
            state: fixture.state,
            host: fixture.host,
            data_dir: PathBuf::from("/tmp/noctalia"),
        },
        json!({
            "greet": { "label": "Hello {name}" }
        }),
    )
}

fn empty_fixture(event: &str) -> Fixture<'_> {
    Fixture {
        event,
        payload: json!({}),
        config: json!({}),
        state: json!({}),
        host: json!({}),
    }
}

fn run(source: &str) -> Vec<Value> {
    let ctx = context(empty_fixture("update"));
    let ops = dispatch(&ctx, &[("entry", source)]).expect("dispatch");
    ops.iter()
        .map(|op| serde_json::to_value(op).expect("serialize op"))
        .collect()
}

fn find<'a>(values: &'a [Value], name: &str) -> &'a Value {
    values
        .iter()
        .find(|value| value.get("op").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("missing op {name}"))
}

#[test]
fn json_encode_and_decode_round_trip() {
    let source = r#"
local encoded = json.encode({ a = 1, b = { "x", "y" }, c = true })
assert(encoded == "{\"a\":1,\"b\":[\"x\",\"y\"],\"c\":true}", encoded)
local value = json.decode(encoded)
assert(value.a == 1)
assert(value.b[2] == "y")
assert(value.c == true)
local ok, err = noctalia.json.decode("{")
assert(ok == nil and type(err) == "string")
"#;
    assert!(run(source).is_empty());
}

#[test]
fn config_and_state_reads_use_context_snapshot() {
    let mut fixture = empty_fixture("update");
    fixture.config = json!({ "name": "Hello", "enabled": true, "count": 3 });
    fixture.state = json!({ "timers": [1, 2, 3], "label": "ready" });
    fixture.host = json!({
        "colors": { "primary": "#fff" },
        "setting": { "shell.font": "sans" }
    });
    let ctx = context(fixture);
    let source = r##"
assert(noctalia.getConfig("name") == "Hello")
assert(noctalia.getConfig("enabled") == true)
assert(noctalia.getConfig("count") == 3)
assert(noctalia.getConfig("missing") == nil)
assert(state.get("label") == "ready")
assert(state.get("timers")[3] == 3)
assert(state.get("missing") == nil)
assert(noctalia.getColor("primary") == "#fff")
assert(noctalia.getSetting("shell.font") == "sans")
assert(noctalia.tr("greet.label", { name = "Ada" }) == "Hello Ada")
"##;
    let ops = dispatch(&ctx, &[("entry", source)]).expect("dispatch");
    assert!(ops.is_empty());
}

#[test]
fn actions_emit_typed_ops_and_generic_host_calls() {
    let values = run(r#"
noctalia.log("hello")
noctalia.notify("Title", "Body")
noctalia.setUpdateInterval(1000)
state.set("answer", { value = 42 })
noctalia.futureApi({ mode = "fast" }, 7)
layer.show("badge")
"#);
    assert_eq!(find(&values, "log")["message"], "hello");
    assert_eq!(find(&values, "notify")["title"], "Title");
    assert_eq!(find(&values, "set_interval")["ms"], 1000);
    assert_eq!(find(&values, "state_set")["value"]["value"], 42);
    assert_eq!(find(&values, "host_call")["path"], json!(["futureApi"]));
    assert_eq!(find(&values, "host_call")["args"][0]["mode"], "fast");
    assert_eq!(find(&values, "layer_show")["id"], "badge");
}

#[test]
fn stream_start_reports_success() {
    let values = run(r#"
assert(noctalia.runStream("while true do end", "line") == true, "stream start")
"#);
    assert_eq!(find(&values, "host_call")["path"], json!(["runStream"]));
}

#[test]
fn require_loads_registered_local_sources() {
    let ctx = context(empty_fixture("update"));
    let sources = [
        (
            "entry",
            r#"
local util = require("./lib/util.luau")
assert(util.answer == 42)
noctalia.log(util.echo("ok"))
"#,
        ),
        (
            "lib/util.luau",
            "return { answer = 42, echo = function(v) return v end }",
        ),
    ];
    let ops = dispatch(&ctx, &sources).expect("dispatch");
    assert_eq!(ops.len(), 1);
    assert_eq!(
        serde_json::to_value(&ops[0]).expect("serialize")["message"],
        "ok"
    );
}

#[test]
fn callback_receives_payload_arguments() {
    let mut fixture = empty_fixture("scroll");
    fixture.payload =
        json!({ "axis": "y", "steps": 3, "starts_gesture": false });
    let ctx = context(fixture);
    let source = r#"
function onScroll(axis, steps, startsGesture)
    noctalia.log(axis .. ":" .. steps .. ":" .. tostring(startsGesture))
end
"#;
    let ops = dispatch(&ctx, &[("entry", source)]).expect("dispatch");
    assert_eq!(ops.len(), 1);
    assert_eq!(
        serde_json::to_value(&ops[0]).expect("serialize")["message"],
        "y:3:false"
    );
}

#[test]
fn ui_tree_render_serializes_kind_props_and_children() {
    let values = run(r#"
local tree = ui.column({ gap = 4, onClick = function() end }, {
    ui.label({ text = "hello" }),
    ui.row({ align = "center" }, {
        ui.glyph({ name = "sparkles" })
    })
})
panel.render(tree)
"#);
    let tree = &find(&values, "panel_render")["tree"];
    assert_eq!(tree["kind"], "column");
    assert_eq!(tree["props"]["gap"], 4);
    assert!(tree["props"]["onClick"]["__call"].is_string());
    assert_eq!(tree["children"][0]["kind"], "label");
    assert_eq!(tree["children"][0]["props"]["text"], "hello");
    assert_eq!(tree["children"][1]["kind"], "row");
    assert_eq!(tree["children"][1]["children"][0]["kind"], "glyph");
    assert_eq!(
        tree["children"][1]["children"][0]["props"]["name"],
        "sparkles"
    );
}

#[test]
fn typed_actions_cover_widgets_and_host_helpers() {
    let values = run(r#"
barWidget.setText("42")
barWidget.setTooltip({ { key = "k", value = "v" } })
barWidget.render(ui.label({ text = "bar" }))
desktopWidget.render(ui.label({ text = "desk" }))
launcher.setResults("q", { { id = "one" } })
layer.configure("osk", { output = "all" })
noctalia.copyToClipboard("clip")
noctalia.runAsync({ "echo", "hi" }, function(result) noctalia.log(result.stdout) end)
"#);
    assert_eq!(find(&values, "bar_text")["text"], "42");
    assert_eq!(find(&values, "bar_tooltip")["value"][0]["key"], "k");
    assert_eq!(find(&values, "bar_render")["tree"]["kind"], "label");
    assert_eq!(
        find(&values, "desktop_widget_render")["tree"]["kind"],
        "label"
    );
    assert_eq!(find(&values, "launcher_results")["rows"][0]["id"], "one");
    assert_eq!(find(&values, "layer_configure")["props"]["output"], "all");
    assert_eq!(
        find(&values, "host_call")["path"],
        json!(["copyToClipboard"])
    );
    let run = values
        .iter()
        .find(|value| value.get("path") == Some(&json!(["runAsync"])))
        .expect("run op");
    assert!(run["args"][1]["__host_callback"].is_string());
}

#[test]
fn unknown_globals_emit_host_calls_with_full_path() {
    let values = run(r#"
noctalia.compositor.subscribe({ events = { "windows" } }, function() end)
noctalia.osd.show("bell", "done", { progress = 0.5 })
shortcut.setActive("main", true)
"#);
    let subscribe = values
        .iter()
        .find(|value| {
            value.get("path") == Some(&json!(["compositor", "subscribe"]))
        })
        .expect("subscribe op");
    assert_eq!(subscribe["root"], "noctalia");
    assert_eq!(subscribe["args"][0]["events"][0], "windows");
    assert_eq!(find(&values, "host_call")["root"], "noctalia");
    let shortcut = values
        .iter()
        .find(|value| {
            value.get("root").and_then(Value::as_str) == Some("shortcut")
        })
        .expect("shortcut op");
    assert_eq!(shortcut["path"], json!(["setActive"]));
    let osd = values
        .iter()
        .find(|value| value.get("path") == Some(&json!(["osd", "show"])))
        .expect("osd op");
    assert_eq!(osd["args"][2]["progress"], 0.5);
}

#[test]
fn top_local_state_survives_repeated_dispatch() {
    const SOURCES: &[(&str, &str)] = &[(
        "entry",
        r#"
local count = 0

function update()
    count = count + 1
    noctalia.log("count=" .. count)
end
"#,
    )];
    let ctx = context(empty_fixture("update"));
    let first = dispatch(&ctx, SOURCES).expect("first dispatch");
    let second = dispatch(&ctx, SOURCES).expect("second dispatch");

    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(
        serde_json::to_value(&first[0]).expect("serialize")["message"],
        "count=1"
    );
    assert_eq!(
        serde_json::to_value(&second[0]).expect("serialize")["message"],
        "count=2"
    );
}

#[test]
fn required_module_state_survives_repeated_dispatch() {
    let sources = [
        (
            "entry",
            r#"
local counter = require("./lib/counter.luau")

function update()
    counter.bump()
    noctalia.log("value=" .. counter.value())
end
"#,
        ),
        (
            "lib/counter.luau",
            r#"
local value = 0

return {
    bump = function() value = value + 1 end,
    value = function() return value end
}
"#,
        ),
    ];
    let ctx = context(empty_fixture("update"));
    let first = dispatch(&ctx, &sources).expect("first dispatch");
    let second = dispatch(&ctx, &sources).expect("second dispatch");

    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(
        serde_json::to_value(&first[0]).expect("serialize")["message"],
        "value=1"
    );
    assert_eq!(
        serde_json::to_value(&second[0]).expect("serialize")["message"],
        "value=2"
    );
}

#[test]
fn dispatch_uses_current_snapshot_and_returns_only_request_ops() {
    let source = r#"
function onConfigChanged(key, value)
    noctalia.log(noctalia.getConfig("name") .. ":" .. key .. ":" .. tostring(value))
end
"#;
    let mut first_fixture = empty_fixture("config_changed");
    first_fixture.config = json!({ "name": "first" });
    first_fixture.payload = json!({ "key": "name", "value": "one" });
    let first_ctx = context(first_fixture);

    let mut second_fixture = empty_fixture("config_changed");
    second_fixture.config = json!({ "name": "second" });
    second_fixture.payload = json!({ "key": "name", "value": "two" });
    let second_ctx = context(second_fixture);

    let first =
        dispatch(&first_ctx, &[("entry", source)]).expect("first dispatch");
    let second =
        dispatch(&second_ctx, &[("entry", source)]).expect("second dispatch");

    assert_eq!(first.len(), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(
        serde_json::to_value(&first[0]).expect("serialize")["message"],
        "first:name:one"
    );
    assert_eq!(
        serde_json::to_value(&second[0]).expect("serialize")["message"],
        "second:name:two"
    );
}

#[test]
fn callback_mapping_covers_host_events() {
    let mappings = [
        ("update", "update"),
        ("render", "update"),
        ("init", "update"),
        ("click", "onClick"),
        ("right_click", "onRightClick"),
        ("middle_click", "onMiddleClick"),
        ("double_click", "onDoubleClick"),
        ("scroll", "onScroll"),
        ("hover", "onHover"),
        ("key", "onKey"),
        ("ipc", "onIpc"),
        ("config_changed", "onConfigChanged"),
        ("outputs_changed", "onOutputsChanged"),
        ("open", "onOpen"),
        ("close", "onClose"),
        ("close_clicked", "onCloseClicked"),
        ("refresh_clicked", "onRefreshClicked"),
        ("settings_clicked", "onSettingsClicked"),
        ("activate", "onActivate"),
        ("query", "onQuery"),
        ("frame", "onFrameTick"),
        ("frame_tick", "onFrameTick"),
        ("enable", "onEnable"),
        ("exit", "onExit"),
        ("suspend", "onSuspend"),
        ("resume", "onResume"),
        ("refresh", "onRefresh"),
        ("search", "onSearch"),
        ("search_change", "onSearchChange"),
    ];
    for (event, callback) in mappings {
        assert_eq!(callback_for_event(event), Some(callback));
    }
    assert_eq!(callback_for_event("unknown"), None);

    let host = Host::new(
        &context(Fixture {
            event: "update",
            payload: json!({ "args": [1, 2] }),
            config: json!({}),
            state: json!({}),
            host: json!({}),
        }),
        &[],
    );
    assert_eq!(
        callback_args(&host.snapshot, "onClick"),
        vec![json!(1), json!(2)]
    );
    host.snapshot.update(&context(Fixture {
        event: "update",
        payload: json!({ "key": "name", "value": "Rust" }),
        config: json!({}),
        state: json!({}),
        host: json!({}),
    }));
    assert_eq!(
        callback_args(&host.snapshot, "onConfigChanged"),
        vec![json!("name"), json!("Rust")]
    );
    host.snapshot.update(&context(Fixture {
        event: "update",
        payload: json!({ "text": "query" }),
        config: json!({}),
        state: json!({}),
        host: json!({}),
    }));
    assert_eq!(
        callback_args(&host.snapshot, "onQuery"),
        vec![json!("query")]
    );
}
