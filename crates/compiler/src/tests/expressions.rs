use super::*;

#[test]
fn compiles_a_hello_bar_handler() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    if ctx.event == "render" || ctx.event == "click" {
        let message = ctx.config_str("message")
            .unwrap_or_default()
            .into_owned();
        Ok(vec![Op::BarText { text: message }])
    } else {
        Ok(Vec::new())
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");

    assert!(compiled.source.contains("local function body_"));
    assert!(compiled.source.contains("function update() body_"));
    assert!(compiled.source.contains("noctalia.getConfig(\"message\")"));
    assert!(compiled.source.contains("== nil then __t"));
    assert!(compiled.source.contains("barWidget.setText(message)"));
    assert!(compiled.source.contains("function onClick() body_"));
    assert!(!compiled.source.contains("require("));
    assert!(!compiled.source.contains("bridge."));
    assert_eq!(compiled.events, vec!["click", "render"]);
    assert!(
        compiled.source.len() < 3_000,
        "hello world should stay small: {} bytes",
        compiled.source.len()
    );
}

#[test]
fn maps_host_reads_to_lua_globals() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let exists = ctx.file_exists("~/.config");
    let command = ctx.command_exists("git");
    let contents = ctx.read_file("~/file").unwrap_or_default();
    let _ = ctx.write_file("~/out", "text");
    let home = ctx.env("HOME").unwrap_or_default();
    let path = ctx.expand_path("~/.cache").to_string_lossy().to_string();
    let entries = ctx.list_dir("~/.cache");
    let _ = ctx.mkdir_all("~/.cache/plugin");
    let _ = ctx.remove_file("~/.cache/plugin/x");
    let clock = ctx.format_time("{:%H:%M}");
    let stamp = ctx.format_time_at("%Y", Some(0), Some("UTC"));
    let now = ctx.os_time();
    let _data = ctx.plugin_data_dir_result();
    let _dir = ctx.plugin_dir_result();
    let _ = entries;
    Ok(vec![Op::log(format!("{} {} {} {} {} {} {} {}", exists, command, contents, home, path, clock, stamp, now))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.fileExists("));
    assert!(compiled.source.contains("noctalia.commandExists("));
    assert!(compiled.source.contains("noctalia.readFile("));
    assert!(compiled.source.contains("noctalia.writeFile("));
    assert!(compiled.source.contains("noctalia.getenv("));
    assert!(compiled.source.contains("noctalia.expandPath("));
    assert!(compiled.source.contains("noctalia.listDir("));
    assert!(compiled.source.contains("noctalia.mkdirAll("));
    assert!(compiled.source.contains("noctalia.removeFile("));
    assert!(compiled.source.contains("noctalia.formatTime("));
    assert!(
        compiled
            .source
            .contains("noctalia.formatTime(\"%Y\", (0), (\"UTC\"))")
    );
    assert!(compiled.source.contains("os.time()"));
    assert!(compiled.source.contains("noctalia.pluginDataDir()"));
    assert!(compiled.source.contains("noctalia.pluginDir()"));
}

#[test]
fn keeps_source_indentation_and_escapes() {
    let handler = r#"
fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![
        Op::BarText { text: "quote \" and \\ slash".to_owned() },
        Op::log("line\nbreak"),
    ])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains(r#"barWidget.setText("quote \" and \\ slash")"#)
    );
    assert!(compiled.source.contains(r#"noctalia.log("line\nbreak")"#));
}

#[test]
fn compiles_ranges_remainders_and_unwrap() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let rows = ctx.payload_arg(0).unwrap();
    let mut total = 0;
    for index in 0..3 {
        total += index % 2;
    }
    for index in 0..=3 {
        total += rows.get(index).as_i64().unwrap_or(0);
    }
    let text = format!("{total:x}");
    Ok(vec![Op::log(text)])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("format spec is rejected");
    assert!(
        error.message.contains("format specifier"),
        "{}",
        error.message
    );

    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let rows = ctx.payload_arg(0).unwrap();
    let mut total = 0;
    for index in 0..3 {
        total += index % 2;
    }
    for index in 0..=3 {
        total += rows.get(index).as_i64().unwrap_or(0);
    }
    Ok(vec![Op::log(format!("done {}", total))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("for index = 0, (3) - 1 do"));
    assert!(compiled.source.contains("for index = 0, 3 do"));
    assert!(compiled.source.contains("irem(index, 2)"));
    assert!(compiled.source.contains("local function irem(a, b)"));
    assert!(compiled.source.contains("unwrap(payload.args[(0) + 1])"));
    assert!(compiled.source.contains("local function unwrap(value)"));
}

#[test]
fn compiles_expressions_and_rejects_untranslatable_ones() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let trimmed = "  x  ".trim().to_uppercase();
    let lowered = trimmed.to_lowercase();
    let hit = lowered.contains("x");
    let length = lowered.len();
    let empty = lowered.is_empty();
    let first = lowered.as_str();
    let option = ctx.state_value::<String>("missing");
    let present = option.is_some();
    let absent = option.is_none();
    let cloned = option.clone();
    let cast = length as f64;
    let chosen = if hit { "yes" } else { "no" };
    Ok(vec![Op::log(format!("{} {} {} {} {} {} {} {}", first, present, absent, cloned.unwrap_or_default(), cast, empty, chosen, ctx.state_value::<String>("x").unwrap()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("string.trim(\"  x  \")"));
    assert!(compiled.source.contains("string.upper("));
    assert!(compiled.source.contains("string.lower("));
    assert!(compiled.source.contains("string.find("));
    assert!(compiled.source.contains("#lowered"));
    assert!(compiled.source.contains("local __t1"));
    assert!(compiled.source.contains("if hit then"));
}

#[test]
fn lowers_question_mark_on_options_and_results() {
    let handler = r#"
fn option(ctx: &Context, ops: &mut Vec<Op>) {
    let value = ctx.config_str("message")?;
    ops.push(Op::log(value));
}
fn result(ctx: &Context) -> Result<i64> { Ok(ctx.now_ms) }
fn value(ctx: &Context, ops: &mut Vec<Op>) {
    let now = result(ctx)?;
    ops.push(Op::log(format!("{}", now)));
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut ops = Vec::new();
    option(ctx, &mut ops);
    value(ctx, &mut ops);
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("== nil then return end"));
}

#[test]
fn covers_expression_subset_additions() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let (first, _, third) = ("a", "b", "c");
    let map = std::collections::HashMap::new();
    let set = std::collections::BTreeSet::new();
    let null = Value::Null;
    let boolean = Value::Bool(true);
    let number = Value::Number(1.0);
    let tree = ui::node("box", json!({ "id": "root" }), vec![]);
    let key = "message";
    let from_config = ctx.config.get(key);
    let indexed = ctx.config["message"];
    let number_value = from_config.and_then(Value::as_f64).unwrap_or(0.0);
    if let Some(text) = ctx.config_str("message") {
        let wrapped = &text;
        let value = *wrapped;
        Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}", first, third, map.len(), set.len(), number, boolean, tree.kind, number_value + value.len() as f64))])
    } else {
        let Some(fallback) = indexed.and_then(Value::as_str) else {
            return Ok(vec![Op::log("fallback")]);
        };
        Ok(vec![Op::log(fallback)])
    }
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.getConfig"));
}

#[test]
fn covers_additional_bail_and_expression_branches() {
    for handler in [
        "fn widget(ctx: &Context) -> Result<Vec<Op>> { let value = ctx.config_str(\"message\").map(|a: i64, b: i64| a + b); let _ = value; Ok(vec![]) }",
        "fn widget(ctx: &Context) -> Result<Vec<Op>> { static X: std::sync::OnceLock<String> = std::sync::OnceLock::new(); X.compare_exchange(1, 2); Ok(vec![]) }",
        "fn widget(ctx: &Context) -> Result<Vec<Op>> { let f = |(a, b): (i64, i64)| a + b; let _ = f; Ok(vec![]) }",
    ] {
        let error =
            compile(&source(handler), &options("widget", EntryKind::Widget))
                .expect_err("bail");
        assert!(!error.message.is_empty());
    }
    let handler = r#"
struct State { count: i64 }
impl State {
    fn new() -> State { State { count: 1 } }
    fn empty(&self) -> bool { self.count == 0 }
}
fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    let state = State::new();
    let empty = state.empty();
    let (a, b) = (1, 2);
    if let value = Some(a) {
        let Some(text) = Some("x") else { return Ok(vec![]); };
        return Ok(vec![Op::log(format!("{}{}{}{}", empty, a, b, text))]);
    }
    Ok(vec![])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("local a, b ="));
}

#[test]
fn emits_closures_as_lua_functions() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let double = |value: i64| value * 2;
    let add = |left: i64, right: i64| { left + right };
    let first = double(2);
    let second = add(1, 2);
    let ignored = |_value: i64| 0;
    let third = ignored(1);
    Ok(vec![Op::log(ctx.tr("key", &[("first", json!(first)), ("second", json!(second)), ("third", json!(third))]))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("function(value)"));
    assert!(compiled.source.contains("function(left, right)"));
    assert!(compiled.source.contains("double(2)"));
}

#[test]
fn covers_remaining_expression_and_error_paths() {
    let handler = r#"
use std::sync::OnceLock;
struct State { count: i64 }
impl State {
    fn new(value: i64) -> State { State { count: value } }
    fn bump(&mut self, amount: i64, extra: i64) { self.count += amount + extra; }
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let state = State::new(1);
    let mut mutable = state;
    mut_helper(&mut mutable, ctx);
    let cell: OnceLock<String> = OnceLock::new();
    let _ = cell.get();
    let value = ctx.config.get("message");
    let indexed = ctx.config["name"];
    let object = value.and_then(Value::as_object);
    let number = value.and_then(Value::as_f64);
    let flag = matches!(value, Value::Null | Some(Value::Bool(false)));
    let tuple = match value { Some(text) => (text.to_string(), 1), None => ("none".to_owned(), 0) };
    let tree = ui::node("box", json!({}), vec![]);
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}", mutable.count, object.is_some(), number.is_some(), flag, tuple.0, tuple.1, tree.kind))])
}
fn mut_helper(state: &mut State, _ctx: &Context) {
    state.bump(1, 2);
}
"#;
    for handler in [
        "struct State { count: i64 } impl State { fn new() -> State { State { count: 1 } } } fn widget(_ctx: &Context) -> Result<Vec<Op>> { let s = State::new(2); let _ = s; Ok(vec![]) }",
        "struct State { count: i64 } impl State { fn bump(&self, a: i64, b: i64) { let _ = (a, b); } } fn widget(_ctx: &Context) -> Result<Vec<Op>> { let s = State { count: 1 }; s.bump(1); Ok(vec![]) }",
    ] {
        let error =
            compile(&source(handler), &options("widget", EntryKind::Widget))
                .expect_err("bail");
        assert!(error.line > 0);
    }

    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.getConfig"));
}

#[test]
fn covers_arithmetic_host_reads_and_pattern_edges() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let a = 10 - 3;
    let b = 10 / 3;
    let c = 10.0 / 3.0;
    let d = 10 % 3;
    let e = 10.0 % 3.0;
    let now = ctx.host("now_ms").and_then(Value::as_i64).unwrap_or(0);
    let dark = ctx.host("dark").and_then(Value::as_bool).unwrap_or(false);
    let outputs = ctx.host("outputs").unwrap_or(json!([]));
    let focused = ctx.host("focused_output_name").and_then(Value::as_str).unwrap_or("");
    let key = "message";
    let from_key = ctx.config.get(key);
    let pair = match from_key { (first, second) => first, _ => "none" };
    let closure = |text: &str| { let upper = text.to_uppercase(); upper };
    let upper = closure("x");
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}{}{}", a, b, c, d, e, now, dark, outputs, focused, pair.len() + upper.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.nowMs()"));
}

#[test]
fn compiles_closure_argument_shapes() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let wild = ctx.config_str("message").map(|_| 1);
    let block = ctx.config_str("message").map(|value| { let size = value.len(); size });
    let empty = ctx.config_str("message").map(|_| {});
    let fold = ctx.config_str("message").map_or_else(|| { let fallback = 1; fallback }, |value| { value.len() });
    Ok(vec![Op::log(format!("{}{}{}{}", wild.unwrap_or_default(), block.unwrap_or_default(), empty.unwrap_or_default(), fold))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("if __t1 ~= nil then"));
    assert!(compiled.source.contains("nil"));
}

#[test]
fn compiles_transparent_tuple_structs() {
    let handler = r#"
struct Score(pub i64);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let score = Score(3);
    let value = score.0;
    Ok(vec![Op::log(format!("{value}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("local score = 3"), "{generated}");
    assert!(generated.contains("local value = score"), "{generated}");
    assert!(!generated.contains("score[1]"), "{generated}");
}

#[test]
fn bails_on_array_initializers_and_comparators_with_positions() {
    {
        let handler = "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let values = std::array::from_fn(|_| 1); Ok(vec![]) }";
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("bail");
        assert!(error.line > 0 && error.column > 0, "{error:?}");
        assert!(
            error.render("main.rs").starts_with(&format!(
                "main.rs:{}:{}:",
                error.line, error.column
            )),
            "{error:?}"
        );
        assert!(error.message.contains("from_fn"), "{}", error.message);
    }
}

#[test]
fn renames_luau_keyword_identifiers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let end = 1;
    Ok(vec![Op::log(format!("{end}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("local __kw_end = 1"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("noctalia.log(__kw_end)"),
        "{}",
        compiled.source
    );
}

#[test]
fn compiles_byte_literals() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let marker = b'#';
    let label = b"ready";
    Ok(vec![Op::log(format!("{marker}{label}"))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("byte string");
    assert!(error.message.contains("byte strings"), "{}", error.message);
}
#[test]
fn compiles_std_wrappers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let boxed = Box::new(2);
    let cloned = Arc::clone(&boxed);
    let now = std::time::SystemTime::now();
    let larger = f64::max(1.0, 2.0);
    let path = PathBuf::new();
    Ok(vec![Op::log(format!("{cloned}{now}{larger}{path}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("math.max"), "{}", compiled.source);
    assert!(
        compiled.source.contains("noctalia.nowMs()"),
        "{}",
        compiled.source
    );
}
#[test]
fn compiles_std_function_references() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = Some(serde_json::Value::Null);
    let empty = value.map_or(true, Value::is_null);
    let floored = Some(1.5_f64).map(f64::floor);
    Ok(vec![Op::log(format!("{empty}{floored}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("math.floor(__value)"),
        "{}",
        compiled.source
    );
}
#[test]
fn compiles_collect_string_and_ordering_paths() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let text = "abc".chars().take(2).collect::<String>();
    let _order = std::sync::atomic::Ordering::Relaxed;
    Ok(vec![Op::log(text)])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("table.concat"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("\"Relaxed\""),
        "{}",
        compiled.source
    );
}
#[test]
fn compiles_imported_ui_helpers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let id = callback("on_click");
    let tree = node("label", json!({ "text": "ok" }), Vec::new());
    Ok(vec![Op::BarRender { tree }, Op::state_watch("k", id)])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("__host_callback"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("kind = \"label\""),
        "{}",
        compiled.source
    );
}
#[test]
fn compiles_remaining_std_wrappers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let smaller = i64::min(3, 4);
    let bytes = std::str::from_utf8(&[104, 105]).unwrap_or("");
    let buffer = ByteBuf::new(vec![104u8]);
    let options = OpenOptions::new();
    let _ = options;
    Ok(vec![Op::log(format!("{smaller}{bytes}{buffer}"))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("unsupported UTF-8 conversion");
    assert!(
        error.message.contains("UTF-8 conversion"),
        "{}",
        error.message
    );
}

#[test]
fn rejects_let_chain_binding_shadowing() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let outer = 10;
    let value = if let Some(outer) = Some(5) && let Some(extra) = Some(1) {
        outer
    } else {
        outer
    };
    Ok(vec![Op::log(format!("{value}"))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("shadowing");
    assert!(error.message.contains("cannot shadow"), "{}", error.message);
}

#[test]
fn void_if_branches_do_not_emit_empty_assignments() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let flag = ctx.event == "never";
    let _picked = if flag {
        Op::log("a")
    } else {
        Op::log("b")
    };
    Ok(vec![])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Widget))
            .expect("compiles");
    assert!(
        !compiled.source.lines().any(|line| {
            let line = line.trim();
            line.starts_with("__t") && line.ends_with('=')
        }),
        "{}",
        compiled.source
    );
}
