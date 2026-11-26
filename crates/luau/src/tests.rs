use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::callback::{callback_args, callback_for_event};
use crate::dispatch;
use crate::dispatch_strict;
use crate::host::Host;
use noctalia_rs::{Context, Parts, api};
use serde_json::{Value, json};

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
fn missing_state_keys_query_the_global_state_service() {
    let dir = QueryDir::new();
    dir.respond("noctalia_rs_luau_1", json!({ "shared": [1, 2] }));
    let ctx = context_in(empty_fixture("update"), dir.path());
    let source = r#"
local value = state.get("shared.key")
assert(type(value) == "table")
assert(value.shared[2] == 2)
"#;
    let values = dispatch(&ctx, &[("entry", source)])
        .expect("dispatch")
        .into_iter()
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect::<Vec<_>>();
    let query = find(&values, "query");
    assert_eq!(query["path"], json!(["state", "get"]));
    assert_eq!(query["args"][0], "shared.key");
}

#[test]
fn plugin_data_dir_preserves_a_nil_host_result() {
    let dir = QueryDir::new();
    dir.respond("noctalia_rs_luau_1", json!(Value::Null));
    let ctx = context_in(empty_fixture("update"), dir.path());
    let source = r#"assert(noctalia.pluginDataDir() == nil)"#;
    assert!(dispatch(&ctx, &[("entry", source)]).is_ok());
}

#[test]
fn query_results_decode_byte_markers() {
    let dir = QueryDir::new();
    dir.respond(
        "noctalia_rs_luau_1",
        json!({ "__noctalia_rs_bytes": "89504e470d0a1a0a" }),
    );
    let ctx = context_in(empty_fixture("update"), dir.path());
    let source = r#"
local bytes = noctalia.appIconPath("app")
assert(string.byte(bytes, 1) == 137)
assert(string.byte(bytes, 2) == 80)
"#;
    assert!(dispatch(&ctx, &[("entry", source)]).is_ok());
}

#[test]
fn seeded_rng_matches_embedded_luau_vm() {
    use mlua::Lua;
    use noctalia_rs::LuauRng;

    let lua = Lua::new();
    let floating: Vec<f64> = lua
        .load(
            "math.randomseed(42); local t = {} \
             for i = 1, 8 do t[i] = math.random() end return t",
        )
        .eval()
        .expect("luau floats");
    let integers: Vec<i64> = lua
        .load(
            "math.randomseed(42); local t = {} \
             for i = 1, 8 do t[i] = math.random(10) end return t",
        )
        .eval()
        .expect("luau integers");
    let ranges: Vec<i64> = lua
        .load(
            "math.randomseed(7); local t = {} \
             for i = 1, 8 do t[i] = math.random(-3, 3) end return t",
        )
        .eval()
        .expect("luau ranges");

    let mut rng = LuauRng::seed(42);
    let ported_floats: Vec<f64> = (0..8).map(|_| rng.next_f64()).collect();
    let mut rng = LuauRng::seed(42);
    let ported_integers: Vec<i64> = (0..8)
        .map(|_| rng.below(10).expect("integer range"))
        .collect();
    let mut rng = LuauRng::seed(7);
    let ported_ranges: Vec<i64> = (0..8)
        .map(|_| rng.range(-3, 3).expect("inclusive range"))
        .collect();

    assert_eq!(ported_floats, floating);
    assert_eq!(ported_integers, integers);
    assert_eq!(ported_ranges, ranges);
}
#[test]
fn registered_member_kinds_drive_luau_dispatch() {
    let value = api::find("noctalia", "appIconPath").expect("appIconPath");
    assert_eq!(value.kind, api::MemberKind::Value);
    let void =
        api::find("noctalia", "setWallpaperEnabled").expect("void member");
    assert_eq!(void.kind, api::MemberKind::Void);

    let dir = QueryDir::new();
    dir.respond("noctalia_rs_luau_1", json!("icon.png"));
    let ctx = context_in(empty_fixture("update"), dir.path());
    let source = r#"
assert(noctalia["appIconPath"]("app") == "icon.png")
assert(noctalia["setWallpaperEnabled"]("DP-1", true) == nil)
"#;
    let values = dispatch(&ctx, &[("entry", source)])
        .expect("dispatch")
        .into_iter()
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect::<Vec<_>>();
    assert!(
        values.iter().any(|value| value["op"] == "query"
            && value["path"] == json!(["appIconPath"])),
        "value member did not query the host: {values:?}"
    );
    let calls = values
        .iter()
        .filter(|value| {
            value["op"] == "host_call"
                && value["path"] == json!(["setWallpaperEnabled"])
        })
        .count();
    assert_eq!(calls, 1, "void member did not emit one host call");
}

#[test]
fn binary_lua_strings_cross_as_byte_markers() {
    let values =
        run(r#"noctalia.copyToClipboard(string.char(255, 254), "image/png")"#);
    let call = values
        .iter()
        .find(|value| value["op"] == "host_call")
        .expect("host call");
    assert_eq!(call["args"][0], json!({ "__noctalia_rs_bytes": "fffe" }));
}

#[test]
fn ordinary_tables_that_carry_the_marker_key_stay_tables() {
    let values = run(
        r#"noctalia.log(json.encode({ __noctalia_rs_bytes = "zz", other = 1 }))"#,
    );
    let log = find(&values, "log");
    let message = log["message"].as_str().expect("message");
    assert!(message.contains("other"), "{message}");
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
noctalia.layer.show("badge")
assert(type(layer.render) == "function")
assert(layer.show == nil)
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
noctalia.layer.configure("osk", { output = "all" })
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
noctalia.unknownFutureApi({ mode = "fast" }, 7)
noctalia.osd.show("bell", "done", { progress = 0.5 })
shortcut.setActive("main", true)
"#);
    let unknown = find(&values, "host_call");
    assert_eq!(unknown["root"], "noctalia");
    assert_eq!(unknown["path"], json!(["unknownFutureApi"]));
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

#[test]
fn generated_callbacks_have_runtime_mappings() {
    for callback in noctalia_rs::codegen::CALLBACKS {
        assert_eq!(
            callback_for_event(callback.event),
            Some(callback.function),
            "{} has no runtime callback",
            callback.event
        );
    }

    for event in ["init", "update", "render", "frame_tick"] {
        assert!(
            callback_for_event(event).is_some(),
            "{event} has no runtime callback"
        );
    }
}

#[test]
fn file_and_path_members_answer_with_the_fixture_shapes() {
    let dir = QueryDir::new();
    dir.queue(vec![
        json!(["/tmp/a.txt", "/tmp/b"]),
        json!({ "size": 3, "mtime": 10, "isDir": false }),
        json!("hello"),
        json!(true),
        json!(true),
    ]);
    let ctx = context_in(empty_fixture("update"), dir.path());
    let source = r#"
local entries, list_error = noctalia.listDir("/tmp")
assert(list_error == nil and entries[1] == "/tmp/a.txt")
local info, info_error = noctalia.fileInfo("/tmp/a.txt")
assert(info_error == nil and info.size == 3 and info.mtime == 10)
assert(info.isDir == false)
local text, read_error = noctalia.readFile("/tmp/a.txt")
assert(read_error == nil and text == "hello")
local written, write_error = noctalia.writeFile("/tmp/b.txt", text .. "!")
assert(written == true and write_error == nil)
assert(noctalia.fileExists("/tmp/a.txt") == true)
assert(type(noctalia.mkdirAll) == "function")
assert(type(noctalia.removeFile) == "function")
assert(type(noctalia.renameFile) == "function")
"#;
    let values = dispatch(&ctx, &[("entry", source)])
        .expect("dispatch")
        .into_iter()
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect::<Vec<_>>();
    let query = |path: Value| {
        values
            .iter()
            .find(|value| value["op"] == "query" && value["path"] == path)
            .unwrap_or_else(|| panic!("missing query {path}: {values:?}"))
    };
    assert_eq!(query(json!(["listDir"]))["args"][0], "/tmp");
    assert_eq!(query(json!(["fileInfo"]))["args"][0], "/tmp/a.txt");
    assert_eq!(query(json!(["readFile"]))["args"][0], "/tmp/a.txt");
    assert_eq!(
        query(json!(["writeFile"]))["args"],
        json!(["/tmp/b.txt", "hello!"])
    );
    assert_eq!(query(json!(["fileExists"]))["args"][0], "/tmp/a.txt");
}

#[test]
fn embedded_bridge_source_parses() {
    let lua = mlua::Lua::new();
    let source = noctalia_rs::bridge::SOURCE
        .replace("{{HELPER}}", "helper")
        .replace("{{PREFIX}}", "prefix");
    lua.load(&source)
        .into_function()
        .expect("bridge.luau must parse");
}

const BRIDGE_STUBS: &str = r#"
local on_line_sink = nil
local fail_next_write = false
noctalia.mkdirAll = function() return true end
noctalia.runStream = function(_, on_line)
  on_line_sink = on_line
  on_line('{"control":"ready","boot":"test","spool":"/spool"}')
  return true
end
noctalia.writeFile = function(_, contents)
  if fail_next_write then
    fail_next_write = false
    return false, "bridge down"
  end
  noctalia.log(contents)
  return true
end
noctalia.renameFile = function() return true end
"#;

fn bridge_requests_from(body: &str) -> Vec<Value> {
    let bridge = noctalia_rs::bridge::SOURCE
        .replace("{{HELPER}}", "helper")
        .replace("{{PREFIX}}", "prefix");
    let source = format!(
        "{BRIDGE_STUBS}\nlocal bridge = (function()\n{bridge}\nend)()\n\
         bridge.setup(\"service\", {{\"name\"}})\n\
         function update()\n{body}end\n"
    );
    let ctx = context(Fixture {
        event: "update",
        payload: json!({}),
        config: json!({ "name": "bench" }),
        state: json!({}),
        host: json!({
            "now_ms": 1_700_000_000_i64,
            "setting": { "shell.time_format": "time" },
            "colors": { "primary": "#fff" },
        }),
    });
    let ops = dispatch(&ctx, &[("entry", source.as_str())]).expect("dispatch");
    let values = ops
        .iter()
        .map(|op| serde_json::to_value(op).expect("serialize op"))
        .collect::<Vec<_>>();
    values
        .iter()
        .filter(|value| value.get("op").and_then(Value::as_str) == Some("log"))
        .filter_map(|value| value.get("message").and_then(Value::as_str))
        .filter_map(|message| serde_json::from_str(message).ok())
        .collect()
}

fn bridge_requests(events: &[&str]) -> Vec<Value> {
    let body = events
        .iter()
        .map(|event| format!("  bridge.call(\"{event}\", {{}})\n"))
        .collect::<String>();
    bridge_requests_from(&body)
}

#[test]
fn bridged_requests_resend_the_fixed_host_only_when_it_changes() {
    let requests = bridge_requests(&["render", "render"]);
    assert_eq!(requests.len(), 2, "{requests:?}");

    let first = &requests[0];
    let second = &requests[1];
    assert_eq!(first["event"], json!("render"));
    assert_eq!(
        first["host_fixed"]["setting"]["shell.time_format"],
        json!("time")
    );
    assert_eq!(first["host_fixed"]["colors"]["primary"], json!("#fff"));
    assert_eq!(first["host"]["now_ms"], json!(1_700_000_000_i64));

    assert_eq!(second["event"], json!("render"));
    assert!(second.get("host_fixed").is_none(), "{second}");
    assert_eq!(second["host"]["now_ms"], json!(1_700_000_000_i64));
    assert!(second["host"].get("setting").is_none());
}

#[test]
fn bridged_requests_resend_the_fixed_host_after_a_failed_write() {
    let requests = bridge_requests_from(
        "  fail_next_write = true\n\
         bridge.call(\"render\", {})\n\
         bridge.call(\"render\", {})\n",
    );
    assert_eq!(requests.len(), 1, "{requests:?}");
    assert!(
        requests[0].get("host_fixed").is_some(),
        "a request whose write failed never reached the helper: {}",
        requests[0]
    );
}

#[test]
fn bridge_host_carries_the_fixed_snapshot_after_setup() {
    bridge_requests_from(
        "  assert(bridge.host.setting[\"shell.time_format\"] == \"time\")\n\
         assert(bridge.host.colors.primary == \"#fff\")\n",
    );
}

#[test]
fn bridged_requests_resend_the_fixed_host_after_a_rejected_request() {
    let requests = bridge_requests_from(
        "  bridge.call(\"render\", {})\n\
         on_line_sink('{\"control\":\"error\",\"message\":\"rejected\"}')\n\
         bridge.call(\"render\", {})\n",
    );
    assert_eq!(requests.len(), 2, "{requests:?}");
    assert!(requests[0].get("host_fixed").is_some());
    assert!(
        requests[1].get("host_fixed").is_some(),
        "a rejected request must send the fixed host again: {}",
        requests[1]
    );
}

const OFFICIAL_CALLBACKS: &str =
    include_str!("../../lib/tests/fixtures/callbacks.txt");

struct QueryDir(PathBuf);

impl QueryDir {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("noctalia-query-{nanos}"));
        fs::create_dir_all(root.join(".noctalia-rs/spool")).expect("spool");
        fs::create_dir_all(root.join(".noctalia-rs/query")).expect("query");
        Self(root)
    }

    fn path(&self) -> &PathBuf {
        &self.0
    }

    fn respond(&self, id: &str, value: Value) {
        let body = json!({ "ok": true, "value": value });
        let path = self.0.join(".noctalia-rs/query").join(format!("{id}.json"));
        fs::write(path, body.to_string()).expect("response");
    }

    /// Answers the first queries in order, for a script that calls more than
    /// one host member.
    fn queue(&self, values: Vec<Value>) {
        for (index, value) in values.into_iter().enumerate() {
            self.respond(&format!("noctalia_rs_luau_{}", index + 1), value);
        }
    }
}

impl Drop for QueryDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn context_in(fixture: Fixture<'_>, dir: &Path) -> Context {
    let mut ctx = context(fixture);
    ctx.data_dir = dir.to_path_buf();
    ctx
}

#[test]
fn registry_members_are_reachable_in_the_embedded_runtime() {
    use std::fmt::Write as _;
    let mut source = String::new();
    for member in api::members() {
        let mut path = member.root.to_owned();
        for part in member.name.split('.') {
            path.push_str("[\"");
            path.push_str(part);
            path.push_str("\"]");
        }
        let kind = match member.kind {
            api::MemberKind::Namespace => "table",
            _ => "function",
        };
        let label = format!("{}.{}", member.root, member.name);
        let _ =
            writeln!(source, "assert(type({path}) == \"{kind}\", \"{label}\")");
    }
    let ctx = context(empty_fixture("update"));
    dispatch(&ctx, &[("entry", source.as_str())]).expect("dispatch");
}

#[test]
fn official_callbacks_have_runtime_mappings() {
    for line in OFFICIAL_CALLBACKS
        .lines()
        .filter(|line| !line.starts_with('#'))
    {
        let parts: Vec<_> = line.split_whitespace().collect();
        let (event, function, arity) = (parts[0], parts[1], parts[2]);
        assert_eq!(
            callback_for_event(event),
            Some(function),
            "{event} has no runtime mapping"
        );
        assert!(arity.parse::<usize>().expect("arity") <= 3);
    }

    let host = Host::new(
        &context(Fixture {
            event: "key",
            payload: json!({
                "chord": "space",
                "pressed": true,
                "repeated": true
            }),
            config: json!({}),
            state: json!({}),
            host: json!({}),
        }),
        &[],
    );
    assert_eq!(
        callback_args(&host.snapshot, "onKey"),
        vec![json!("space"), json!(true), json!(true)]
    );

    let host = Host::new(
        &context(Fixture {
            event: "audio_spectrum",
            payload: json!({ "values_csv": "1,2", "state_csv": "ok" }),
            config: json!({}),
            state: json!({}),
            host: json!({}),
        }),
        &[],
    );
    assert_eq!(
        callback_args(&host.snapshot, "onAudioSpectrum"),
        vec![json!("1,2"), json!("ok")]
    );
}

#[test]
fn queries_use_the_host_result() {
    let dir = QueryDir::new();
    dir.respond("noctalia_rs_luau_1", json!("/icons/app.png"));
    let ctx = context_in(empty_fixture("update"), dir.path());
    let source = r#"assert(noctalia.appIconPath("app") == "/icons/app.png")"#;
    let values = dispatch(&ctx, &[("entry", source)])
        .expect("dispatch")
        .into_iter()
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect::<Vec<_>>();
    let query = find(&values, "query");
    assert_eq!(query["path"], json!(["appIconPath"]));
    assert_eq!(query["args"][0], "app");
}

#[test]
fn unwatch_returns_the_host_result() {
    let dir = QueryDir::new();
    dir.respond("noctalia_rs_luau_1", json!(false));
    let ctx = context_in(empty_fixture("update"), dir.path());
    let source = r#"noctalia.log(tostring(noctalia.idle.unwatch(9)))"#;
    let values = dispatch(&ctx, &[("entry", source)])
        .expect("dispatch")
        .into_iter()
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect::<Vec<_>>();
    assert_eq!(find(&values, "log")["message"], "false");
}

#[test]
fn http_stream_handle_forwards_stop() {
    let dir = QueryDir::new();
    for id in 1..=5 {
        dir.respond(&format!("noctalia_rs_luau_{id}"), json!("stream-1"));
    }
    let ctx = context_in(empty_fixture("update"), dir.path());
    let source = r#"
local handle = noctalia.httpStream({}, function() end, function() end)
assert(type(handle) == "table")
assert(type(handle.stop) == "function")
handle.stop()
"#;
    let values = dispatch(&ctx, &[("entry", source)])
        .expect("dispatch")
        .into_iter()
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect::<Vec<_>>();
    let query = find(&values, "query");
    assert_eq!(query["path"], json!(["httpStream"]));
    let stop = find(&values, "host_call");
    assert_eq!(stop["root"], "__noctalia_rs");
    assert_eq!(stop["path"], json!(["httpStreamStop"]));
    assert_eq!(stop["args"][0], "stream-1");
}

#[test]
fn typed_wrappers_keep_every_argument() {
    let values = run(r#"
noctalia.notify("T", "B", {
  actions = { { id = "open", label = "Open" } },
  onAction = "onAction",
  context = "file",
})
noctalia.notifyError("E", nil, { onAction = "onError" })
noctalia.copyToClipboard("uri", "text/uri-list")
noctalia.runAsync({ "echo", "hi" }, function() end, 250)
noctalia.setWallpaper("DP-1", "/wall.png")
noctalia.revealPath("/file")
noctalia.runInTerminal("htop")
"#);
    let notice = find(&values, "notify");
    assert_eq!(notice["options"]["actions"][0]["id"], "open");
    assert_eq!(notice["options"]["onAction"], "onAction");
    assert_eq!(notice["options"]["context"], "file");
    assert_eq!(
        find(&values, "notify_error")["options"]["onAction"],
        "onError"
    );
    let copy = values
        .iter()
        .find(|value| value["path"] == json!(["copyToClipboard"]))
        .expect("copy op");
    assert_eq!(copy["args"][1], "text/uri-list");
    let run = values
        .iter()
        .find(|value| value["path"] == json!(["runAsync"]))
        .expect("run op");
    assert_eq!(run["args"][2].as_f64(), Some(250.0));
    let wallpaper = values
        .iter()
        .find(|value| value["path"] == json!(["setWallpaper"]))
        .expect("wallpaper op");
    assert_eq!(wallpaper["args"], json!(["DP-1", "/wall.png"]));
    assert!(
        values
            .iter()
            .any(|value| value["path"] == json!(["revealPath"]))
    );
    assert!(
        values
            .iter()
            .any(|value| value["path"] == json!(["runInTerminal"]))
    );
}

#[test]
fn host_callback_invokes_the_registered_function() {
    const SOURCE: &str = r#"
noctalia.http({ url = "https://example.invalid/" }, function(response)
    noctalia.log(response.body)
end)
"#;
    let ctx = context(empty_fixture("update"));
    let ops = dispatch(&ctx, &[("entry", SOURCE)]).expect("first dispatch");
    let value = serde_json::to_value(&ops[0]).expect("serialize");
    let id = value["args"][1]["__host_callback"]
        .as_str()
        .expect("callback id");

    let mut callback = empty_fixture("host_callback");
    callback.payload = json!({ "id": id, "args": [{ "body": "ok" }] });
    let ctx = context(callback);
    let ops = dispatch(&ctx, &[("entry", SOURCE)]).expect("callback dispatch");
    assert_eq!(ops.len(), 1);
    assert_eq!(
        serde_json::to_value(&ops[0]).expect("serialize")["message"],
        "ok"
    );
}

#[test]
fn named_host_callbacks_reach_the_script_handler() {
    const SOURCE: &str = r#"
function update()
  noctalia.runStream({ "tail", "-f" }, { __host_callback = "watch" })
end
function onHostCallback(payload)
  if payload.id == "watch" then
    noctalia.log(payload.args[1])
  end
end
"#;
    let ctx = context(empty_fixture("update"));
    let ops = dispatch(&ctx, &[("entry", SOURCE)]).expect("first dispatch");
    let value = serde_json::to_value(&ops[0]).expect("serialize");
    assert_eq!(value["path"], json!(["runStream"]));
    assert_eq!(value["args"][1]["__host_callback"], "watch");

    let mut callback = empty_fixture("host_callback");
    callback.payload = json!({ "id": "watch", "args": ["line"] });
    let ctx = context(callback);
    let ops = dispatch(&ctx, &[("entry", SOURCE)]).expect("callback dispatch");
    let log = serde_json::to_value(&ops[0]).expect("serialize");
    assert_eq!(log["op"], "log");
    assert_eq!(log["message"], "line");
}

#[test]
fn launcher_rows_treat_an_empty_table_as_no_rows() {
    let values = run(r#"
function update()
  launcher.setResults("none", {})
end
"#);
    let results = find(&values, "launcher_results");
    assert_eq!(results["rows"], json!([]));
}

fn strict_error(source: &str) -> String {
    let ctx = context(empty_fixture("update"));
    dispatch_strict(&ctx, &[("entry", source)])
        .expect_err("strict dispatch fails")
        .to_string()
}

#[test]
fn strict_emit_rejects_a_marker_table_where_a_function_is_required() {
    let error = strict_error(
        r#"
noctalia.http({ url = "https://example.invalid/" }, { __host_callback = "onResponse" })
"#,
    );
    assert!(error.contains("API 57 takes a function"), "{error}");
}

#[test]
fn strict_emit_rejects_the_compiler_marker_table() {
    let error = strict_error(
        r#"
noctalia.runAsync({ "echo", "hi" }, { __call = "onDone" })
"#,
    );
    assert!(error.contains("API 57 takes a function"), "{error}");
}

#[test]
fn strict_emit_rejects_a_marker_table_on_a_generic_member() {
    let error = strict_error(
        r#"
noctalia.idle.watch("idle", { __host_callback = "onIdle" })
"#,
    );
    assert!(error.contains("marker table"), "{error}");
}

#[test]
fn strict_emit_rejects_on_host_callback() {
    let error = strict_error(
        r#"
function onHostCallback(payload)
  noctalia.log(payload.id)
end
"#,
    );
    assert!(error.contains("onHostCallback"), "{error}");
}

#[test]
fn auto_keeps_accepting_marker_tables() {
    let values = run(r#"
noctalia.runStream({ "tail", "-f" }, { __host_callback = "watch" })
"#);
    let stream = find(&values, "host_call");
    assert_eq!(stream["args"][1]["__host_callback"], "watch");
}

#[test]
fn strict_accepts_a_real_closure_and_the_later_event_reaches_it() {
    const SOURCE: &str = r#"
noctalia.http({ url = "https://example.invalid/" }, function(response)
    noctalia.log(response.body)
end)
"#;
    let ctx = context(empty_fixture("update"));
    let ops =
        dispatch_strict(&ctx, &[("entry", SOURCE)]).expect("strict dispatch");
    let value = serde_json::to_value(&ops[0]).expect("serialize");
    let id = value["args"][1]["__host_callback"]
        .as_str()
        .expect("callback id")
        .to_owned();

    let mut callback = empty_fixture("host_callback");
    callback.payload = json!({ "id": id, "args": [{ "body": "later" }] });
    let ctx = context(callback);
    let ops =
        dispatch_strict(&ctx, &[("entry", SOURCE)]).expect("callback dispatch");
    let log = serde_json::to_value(&ops[0]).expect("serialize");
    assert_eq!(log["op"], "log");
    assert_eq!(log["message"], "later");
}

#[test]
fn strict_response_registry_serves_read_and_remove() {
    const SOURCE: &str = r#"
local path = "/data/entry/reply.json"
assert(noctalia.writeFile(path, "{\"ok\":true}"))
local text = noctalia.readFile(path)
assert(text == "{\"ok\":true}", tostring(text))
assert(noctalia.removeFile(path))
assert(noctalia.readFile(path) == nil)
"#;
    let ctx = context(empty_fixture("update"));
    dispatch_strict(&ctx, &[("entry", SOURCE)]).expect("strict dispatch");
}

#[test]
fn auto_does_not_use_the_strict_response_registry() {
    const SOURCE: &str = r#"
local path = "/data/entry/reply.json"
local ok = noctalia.writeFile(path, "body")
assert(ok == nil, tostring(ok))
assert(noctalia.readFile(path) == nil)
"#;
    let ctx = context(empty_fixture("update"));
    dispatch(&ctx, &[("entry", SOURCE)]).expect("auto dispatch");
}

#[test]
fn http_reports_offline_mode_and_emits_no_request() {
    let mut fixture = empty_fixture("update");
    fixture.host = json!({ "setting": { "shell.offline_mode": true } });
    let ctx = context(fixture);
    let ops = dispatch_strict(
        &ctx,
        &[(
            "entry",
            r#"assert(noctalia.http({ url = "https://example.invalid/" }, function() end) == false)"#,
        )],
    )
    .expect("strict dispatch");
    assert!(ops.is_empty(), "{ops:?}");
}

#[test]
fn run_start_rejects_a_command_it_cannot_describe() {
    let ctx = context(empty_fixture("update"));
    let ops = dispatch_strict(
        &ctx,
        &[(
            "entry",
            r#"
assert(noctalia.runAsync({}) == false)
assert(noctalia.runAsync({ "echo", 3 }) == false)
assert(noctalia.runStream("", function() end) == false)
"#,
        )],
    )
    .expect("strict dispatch");
    assert!(ops.is_empty(), "{ops:?}");
}

#[test]
fn strict_response_registry_handles_rename_and_file_exists() {
    const SOURCE: &str = r#"
local from = "/data/entry/a.json"
local to = "/data/entry/b.json"
assert(noctalia.writeFile(from, "body"))
assert(noctalia.fileExists(to) == nil)
assert(noctalia.renameFile(from, to))
assert(noctalia.fileExists(to) == true)
assert(noctalia.readFile(from) == nil)
assert(noctalia.readFile(to) == "body")
"#;
    let ctx = context(empty_fixture("update"));
    dispatch_strict(&ctx, &[("entry", SOURCE)]).expect("strict dispatch");
}

#[test]
fn auto_keeps_http_and_run_calls_unchanged() {
    let mut fixture = empty_fixture("update");
    fixture.host = json!({ "setting": { "shell.offline_mode": true } });
    let ctx = context(fixture);
    let ops = dispatch(
        &ctx,
        &[(
            "entry",
            r#"
assert(noctalia.http({ url = "https://example.invalid/" }, function() end) == true)
assert(noctalia.runAsync({}) == true)
assert(noctalia.runStream("", function() end) == true)
"#,
        )],
    )
    .expect("dispatch");
    assert_eq!(ops.len(), 3, "{ops:?}");
}

#[test]
fn strict_write_file_forwards_when_a_host_is_attached() {
    let dir = QueryDir::new();
    dir.respond("noctalia_rs_luau_1", json!(true));
    let ctx = context_in(empty_fixture("update"), dir.path());
    let ops = dispatch_strict(
        &ctx,
        &[(
            "entry",
            r#"
assert(noctalia.writeFile("/sys/led", "1") == true)
assert(noctalia.readFile("/sys/led") == nil)
"#,
        )],
    )
    .expect("strict dispatch");
    let values = ops
        .iter()
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect::<Vec<_>>();
    let query = values
        .iter()
        .find(|value| value["op"] == json!("query"))
        .expect("write query");
    assert_eq!(query["path"], json!(["writeFile"]));
}
