use super::*;

#[test]
fn supports_additional_methods_and_match_guards() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.config_str("message").as_deref().unwrap_or_default().to_owned();
    let mut ops = Vec::new();
    match ctx.event.as_str() {
        "render" if ctx.now_ms > 0 => ops.push(Op::BarText { text }),
        _ => {}
    }
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("barWidget.setText(text)"));
    assert!(compiled.source.contains("and"));
}

#[test]
fn coerces_config_and_payload_strings_like_the_context() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let count = ctx.config_str("count").unwrap_or_default();
    let text = ctx.payload_str("text").unwrap_or_default();
    Ok(vec![Op::log(format!("{count}:{text}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(!compiled.source.contains("str_value"));
    assert!(compiled.source.contains("noctalia.getConfig(\"count\")"));
    assert!(compiled.source.contains("payload[\"text\"]"));
    assert!(compiled.source.contains("local __t"));
    assert!(compiled.source.contains("type("));
}

#[test]
fn covers_more_methods_and_guarded_wildcards() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut rows = Vec::new();
    rows.extend(vec![json!("a"), json!("b")]);
    let shown = ctx.expand_path("~/x").display().to_string();
    let mut ops = Vec::new();
    match ctx.event.as_str() {
        "render" | "click" => ops.push(Op::log(shown)),
        _ if ctx.now_ms > 0 => ops.push(Op::log("later")),
        _ => {}
    }
    let _ = rows;
    Ok(ops)
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("ipairs("));
    assert!(compiled.source.contains("elseif true then"));
    assert!(compiled.source.contains("if (noctalia.nowMs() > 0) then"));
}

#[test]
fn formats_time_at_with_optional_arguments() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let short = ctx.format_time_at("%H", Some(0));
    let long = ctx.format_time_at("%H", Some(0), Some("UTC"));
    Ok(vec![Op::log(format!("{} {}", short, long))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("noctalia.formatTime(\"%H\", (0), nil)")
    );
    assert!(
        compiled
            .source
            .contains("noctalia.formatTime(\"%H\", (0), (\"UTC\"))")
    );
}

#[test]
fn compiles_option_combinators_with_closures() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let value = ctx.config_str("message").map(|text| text.to_uppercase());
    let bool_value = ctx.config_str("message").and_then(Value::as_bool);
    let kept = ctx.config_str("message").filter(|text| !text.is_empty());
    let fallback = ctx.config_str("message").unwrap_or_else(|| "none".to_owned());
    Ok(vec![Op::log(format!("{} {} {} {}", value.unwrap_or_default(), bool_value.unwrap_or(false), kept.unwrap_or_default(), fallback))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("~= nil then"));
    assert!(compiled.source.contains("function(__value)"));
    assert!(!compiled.source.contains("not text.is_empty"));
}

#[test]
fn covers_impl_methods_and_struct_fields() {
    let handler = r#"
struct State { name: String, count: i64 }
impl State {
    fn new(ctx: &Context) -> State { State { name: ctx.config_str("message").unwrap_or_default().into_owned(), count: 0 } }
    fn bump(&mut self, amount: i64) { self.count += amount; }
    fn label(&self) -> String { self.name.clone() }
}
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let mut state = State::new(ctx);
    state.bump(2);
    let label = state.label();
    state.name = "changed".to_owned();
    Ok(vec![Op::log(label)])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("self.count += amount"));
    assert!(compiled.source.contains("state.name ="));
}

#[test]
fn covers_all_option_combinators() {
    let handler = r#"
fn widget(ctx: &Context) -> Result<Vec<Op>> {
    let base = ctx.config_str("message");
    let mapped = base.clone().map(|value| value.to_uppercase());
    let chained = base.clone().and_then(Value::as_str);
    let filtered = base.clone().filter(|value| !value.is_empty());
    let some = base.clone().is_some_and(|value| value == "x");
    let either = base.clone().map_or("default".to_owned(), |value| value);
    let or_else = base.clone().map_or_else(|| "fallback".to_owned(), |value| value);
    let fallback = base.clone().unwrap_or_else(|| "none".to_owned());
    let alt = base.clone().or_else(|| Some("alt".to_owned()));
    let result = base.clone().ok_or("missing");
    let result2 = base.clone().ok_or_else(|| "missing");
    Ok(vec![Op::log(format!("{} {} {} {} {} {} {} {} {} {}", mapped.unwrap_or(""), chained.unwrap_or(""), filtered.unwrap_or(""), some, either, or_else, fallback, alt.unwrap_or(""), result.unwrap_or(""), result2.unwrap_or("")))])
}
"#;
    let compiled =
        compile(&source(handler), &options("widget", EntryKind::Widget))
            .expect("compiles");
    assert!(compiled.source.contains("function(__value)"));
}

#[test]
fn compiles_iterator_chains_and_split_cursors() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let values = [1, 2, 3];
    let doubled = values.iter().map(|value| value * 2).collect::<Vec<i64>>();
    let kept = values.iter().filter(|value| *value > 1).collect::<Vec<i64>>();
    let words = "a  b".split_whitespace().collect::<Vec<&str>>();
    let mut parts = "a.b".split('.');
    let first = parts.next().unwrap_or_default();
    Ok(vec![Op::log(format!("{}{}{}{}{}", doubled.len(), kept.len(), words.len(), first, ctx.event))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("string.split("));
    assert!(compiled.source.contains("__next = 1"));
    assert!(compiled.source.contains("#__t"));
    assert!(compiled.source.contains("if __t"));
}

#[test]
fn compiles_numeric_string_and_path_methods() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = 1.25_f64;
    let rounded = value.floor();
    let fraction = value.fract();
    let finite = value.is_finite();
    let joined = "/tmp".join("file");
    let replaced = "a,b".replace(',', "-");
    let trimmed = "'hi'".trim_matches('\'');
    let encoded = serde_json::Value::String("x".to_owned()).to_string();
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}", rounded, fraction, finite, joined, replaced, trimmed, encoded))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("math.floor(value)"));
    assert!(compiled.source.contains("string.sub(__t"));
    assert!(compiled.source.contains("replace_all("));
    assert!(compiled.source.contains("string.gsub("));
    assert!(compiled.source.contains("json.encode("));
}

#[test]
fn compiles_host_result_pairs() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let (text, error) = ctx.read_file_result("/tmp/x");
    let (font, font_error) = ctx.load_font_result("font.ttf");
    Ok(vec![Op::log(format!("{}{}{}{}", text.unwrap_or_default(), error.is_some(), font.is_some(), font_error.is_some()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("local text, "));
    assert!(compiled.source.contains("noctalia.readFile(\"/tmp/x\")"));
    assert!(compiled.source.contains("noctalia.loadFont(\"font.ttf\")"));
}

#[test]
fn bails_on_host_start_methods_that_report_acceptance() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if !ctx.run_async_start(json!([]), None, None) {
        return Ok(Vec::new());
    }
    Ok(Vec::new())
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("must be discarded"));
    assert!(error.line > 0 && error.column > 0);
}

#[test]
fn compiles_result_combinators_and_defaults() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let parsed = serde_json::from_str::<Value>("{}").map_err(|_| "bad")?;
    let repaired = serde_json::from_str::<Value>("x").unwrap_or_else(|_| Value::Null);
    let home = std::env::var("HOME").unwrap_or_default();
    let fallback = parsed.clone().unwrap_or(Value::Null);
    let optional = parsed.clone().ok();
    let expected = parsed.clone().expect("data");
    let value = parsed.unwrap();
    Ok(vec![Op::log(format!("{} {} {} {} {} {}", home, fallback, optional.is_some(), expected, value, ctx.event))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("json.decode(\"{}\")"));
    assert!(compiled.source.contains("error(tostring("));
    assert!(compiled.source.contains("noctalia.getenv(\"HOME\")"));
    assert!(compiled.source.contains("if __t"));
}

#[test]
fn compiles_remaining_numeric_and_string_methods() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let number = 1.5_f64;
    let bounded = number.clamp(0.0, 1.0);
    let largest = number.max(2.0);
    let smallest = number.min(2.0);
    let positive = number.rem_euclid(2.0);
    let suffix = "prefix:value".strip_prefix("prefix:").unwrap_or_default();
    let upper = "abc".to_ascii_uppercase();
    let lower = "ABC".to_ascii_lowercase();
    let starts = "abc".starts_with("a");
    let ends = "abc".ends_with("c");
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}{}", bounded, largest, smallest, positive, suffix, upper, lower, starts, ends))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("math.clamp("));
    assert!(compiled.source.contains("math.max("));
    assert!(compiled.source.contains("math.min("));
    assert!(compiled.source.contains("string.sub("));
    assert!(compiled.source.contains("string.upper("));
    assert!(compiled.source.contains("string.lower("));
}

#[test]
fn compiles_iterator_source_variants() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let parts = "a,b".split(',').collect::<Vec<&str>>();
    let letters = "ab".chars().collect::<Vec<char>>();
    let lines = "a\nb".lines().collect::<Vec<&str>>();
    let values = [1, 2].into_iter().filter_map(|value| Some(value)).collect::<Vec<i64>>();
    let mut table = std::collections::HashMap::new();
    let keys = table.keys().collect::<Vec<&str>>();
    let entries = table.values().collect::<Vec<&i64>>();
    Ok(vec![Op::log(format!("{}{}{}{}{}{}", parts.len(), letters.len(), lines.len(), values.len(), keys.len(), entries.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("string.split("));
    assert!(compiled.source.contains("for _, __t"));
    assert!(compiled.source.contains("in pairs("));
    assert!(compiled.source.contains(".__next or 1, #"));
}

#[test]
fn compiles_remaining_host_results_and_time_helpers() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let (entries, list_error) = ctx.list_dir_result("/tmp");
    let (info, info_error) = ctx.file_info_result("/tmp/x");
    let (made, mkdir_error) = ctx.mkdir_all_result("/tmp/y");
    let (removed, remove_error) = ctx.remove_file_result("/tmp/z");
    ctx.math_randomseed(7);
    let random = ctx.math_random_range(1, 6);
    let localized = ctx.trp("key", 3, &[("name", json!("x"))]);
    let load = ctx.is_load_event();
    Ok(vec![Op::log(format!("{}", entries.len() + info.len() + made.len() + removed.len() + list_error.is_some() as usize + info_error.is_some() as usize + mkdir_error.is_some() as usize + remove_error.is_some() as usize + random as usize + localized.len() + load as usize))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.listDir(\"/tmp\")"));
    assert!(compiled.source.contains("noctalia.fileInfo("));
    assert!(compiled.source.contains("noctalia.mkdirAll("));
    assert!(compiled.source.contains("noctalia.removeFile("));
    assert!(compiled.source.contains("math.randomseed(7)"));
    assert!(compiled.source.contains("math.random(1, 6)"));
    assert!(
        compiled
            .source
            .contains("noctalia.trp(\"key\", 3, { name = ")
    );
}

#[test]
fn compiles_remaining_host_paths() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let dir = ctx.host("pluginDir").unwrap_or_default();
    let data = ctx.host("pluginDataDir").unwrap_or_default();
    let clip = ctx.host("clipboard").unwrap_or_default();
    let calendar = ctx.host("calendar").unwrap_or_default();
    let media = ctx.host("media").unwrap_or_default();
    let stats = ctx.host("stats").unwrap_or_default();
    let night = ctx.host("nightlight.state").unwrap_or_default();
    let compositor = ctx.host("compositor.name").unwrap_or_default();
    let vertical = ctx.host("bar.vertical").unwrap_or_default();
    let output = ctx.host("bar.output_name").unwrap_or_default();
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}{}{}", dir, data, clip, calendar, media, stats, night, compositor, vertical, output))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("noctalia.pluginDir()"));
    assert!(compiled.source.contains("noctalia.clipboardText()"));
    assert!(compiled.source.contains("barWidget.isVertical()"));
    assert!(compiled.source.contains("noctalia.compositor.name"));
    let error = compile(
        &source("fn handler(ctx: &Context) -> Result<Vec<Op>> { let _ = ctx.host(\"missing\"); Ok(vec![]) }"),
        &options("handler", EntryKind::Service),
    )
    .expect_err("unknown host path");
    assert!(error.message.contains("host path `missing`"));
}

#[test]
fn compiles_remaining_collection_and_value_methods() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let escaped = "[]-abc".trim_matches("[]-");
    let mut table = std::collections::HashMap::new();
    table.insert("a", 1);
    table.insert(0, 2);
    table.remove("a");
    table.clear();
    let object = json!({ "a": 1 }).as_object().is_some();
    let text = json!("x").as_str().is_some();
    let number = json!(1).as_i64().is_some();
    let boolean = json!(true).as_bool().is_some();
    let array = json!([1]).as_array().is_some();
    let null = json!(null).is_null();
    let millis = ctx.now_ms.duration_since(0).as_millis();
    let secs = ctx.now_ms.as_secs();
    let nanos = ctx.now_ms.as_nanos();
    let parts = "a,b".split(',');
    let first = parts.next().unwrap_or_default();
    let bytes = json!([1]).as_array().unwrap().to_vec();
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}{}{}{}{}{}", escaped, object, text, number, boolean, array, null, millis, secs, nanos, first, bytes.len(), ctx.event))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("string.gsub("));
    assert!(compiled.source.contains("table.clear("));
    assert!(compiled.source.contains("table[0] = 2"));
    assert!(compiled.source.contains("table[\"a\"] = nil"));
    assert!(
        compiled
            .source
            .contains("math.floor(noctalia.nowMs() / 1000)")
    );
    assert!(compiled.source.contains("noctalia.nowMs() * 1000000"));
}

#[test]
fn compiles_trp_with_the_count_argument() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let localized = ctx.trp("files", 3, &[("name", json!("x"))]);
    let empty = ctx.trp("files", 1, &[]);
    Ok(vec![Op::log(format!("{}{}", localized, empty))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("noctalia.trp(\"files\", 3, { name = \"x\" })"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("noctalia.trp(\"files\", 1, {"),
        "{}",
        compiled.source
    );

    let short = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let localized = ctx.trp("files", &[]);
    Ok(vec![Op::log(localized)])
}
"#;
    let error =
        compile(&source(short), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("missing argument 2"),
        "{}",
        error.message
    );
    assert!(error.line > 0 && error.column > 0);
}

#[test]
fn compiles_env_var_as_an_optional_string() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let home = std::env::var("HOME").unwrap_or_default();
    let present = std::env::var("MISSING").is_some();
    let fallback = std::env::var("MISSING").unwrap_or("fallback".to_owned());
    Ok(vec![Op::log(format!("{}{}{}", home, present, fallback))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("(noctalia.getenv(\"HOME\") or \"\")"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("(noctalia.getenv(\"MISSING\") ~= nil)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("(noctalia.getenv(\"MISSING\") or \"fallback\")"),
        "{}",
        compiled.source
    );
}

#[test]
fn joins_lists_and_bails_on_other_receivers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let parts = vec!["a".to_owned(), "b".to_owned()];
    let joined = parts.join(",");
    let path = "/tmp".join("file");
    Ok(vec![Op::log(format!("{}{}", joined, path))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("table.concat(parts, \",\")"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("string.sub(__t"),
        "{}",
        compiled.source
    );

    let map = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut table = std::collections::HashMap::new();
    let bad = table.join(",");
    Ok(vec![Op::log(bad)])
}
"#;
    let compiled =
        compile(&source(map), &options("handler", EntryKind::Service))
            .expect("compiles with a runtime join");
    assert!(
        compiled.source.contains("join(table, \",\")"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("local function join(value, separator)")
    );
}

#[test]
fn removes_list_items_and_map_entries() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut values = vec![1, 2, 3];
    let removed = values.remove(0);
    let mut counts = std::collections::HashMap::new();
    counts.insert("a", 1);
    let taken = counts.remove("a");
    Ok(vec![Op::log(format!("{}{}{}", removed, taken.is_some(), values.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("table.remove(values, (0) + 1)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("counts[\"a\"] = nil"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("(taken ~= nil)"),
        "{}",
        compiled.source
    );

    let set = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut names = std::collections::BTreeSet::new();
    names.remove("a");
    Ok(Vec::new())
}
"#;
    let compiled =
        compile(&source(set), &options("handler", EntryKind::Service))
            .expect("compiles as a map removal");
    assert!(compiled.source.contains("names[\"a\"] = nil"));
}

#[test]
fn skips_nil_filter_map_results() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let kept = [1, 2, 3].into_iter().filter_map(|value| if value > 1 { Some(value) } else { None }).collect::<Vec<i64>>();
    Ok(vec![Op::log(format!("{}", kept.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("local __t6 = (__t5)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("if __t6 ~= nil then"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("__t1[#__t1 + 1] = __t6"),
        "{}",
        compiled.source
    );
}

#[test]
fn splits_whitespace_runs_like_rust() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let words = "a\tb".split_whitespace().collect::<Vec<&str>>();
    Ok(vec![Op::log(format!("{}", words.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("for __word in string.gmatch(\"a\\tb\", \"%S+\")"),
        "{}",
        compiled.source
    );
    assert!(!compiled.source.contains("~= \"\" then"));
}

#[test]
fn collects_only_the_items_left_by_split_next() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut parts = "a.b.c".split('.');
    let first = parts.next().unwrap_or_default();
    let rest = parts.collect::<Vec<&str>>();
    Ok(vec![Op::log(format!("{}{}", first, rest.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("parts.__next = parts.__next + 1"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("for __t5 = __t4.__next or 1, #__t4 do"),
        "{}",
        compiled.source
    );
}

#[test]
fn strips_carriage_returns_and_trailing_newlines() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let lines = "a\r\nb\n".lines().collect::<Vec<&str>>();
    Ok(vec![Op::log(format!("{}", lines.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("string.gsub(\"a\\r\\nb\\n\", \"\\r\\n\", \"\\n\")"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("table.remove(__lines)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("string.split(__text, \"\\n\")"),
        "{}",
        compiled.source
    );
}

#[test]
fn host_start_methods_compile_only_when_their_result_is_dropped() {
    let dropped = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    ctx.run_async_start(json!(["true"]), Some("cb"), Some(250.0));
    ctx.run_stream_start(json!(["tail"]), "stream");
    ctx.http_start(json!({}), "req");
    Ok(Vec::new())
}
"#;
    let compiled =
        compile(&source(dropped), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains(
            "noctalia.runAsync({ \"true\" }, { __host_callback = \"cb\" }, 250.0)"
        ),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains(
            "noctalia.runStream({ \"tail\" }, { __host_callback = \"stream\" })"
        ),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("noctalia.http({  }, { __host_callback = \"req\" })"),
        "{}",
        compiled.source
    );

    let used = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if !ctx.http_start(json!({}), "req") {
        return Ok(Vec::new());
    }
    Ok(Vec::new())
}
"#;
    let error = compile(&source(used), &options("handler", EntryKind::Service))
        .expect_err("bails");
    assert!(error.message.contains("must be discarded"));
}

#[test]
fn compiles_character_sets_and_slices() {
    let handler = r#"
fn is_space(ch: char) -> bool {
    matches!(ch, ' ' | '\t' | '\n')
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.payload_str("text").unwrap_or_default();
    let trimmed = text.trim_matches(is_space);
    let closed = text.trim_matches(|c: char| c.is_ascii_whitespace());
    let listed = text.trim_matches(&[' ', '\t'][..]);
    let head = &text[..2];
    let tail = &text[1..];
    let middle = &text[1..3];
    Ok(vec![Op::log(format!("{trimmed} {closed} {listed} {head} {tail} {middle}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("string.sub(text, 1, 2)"), "{source}");
    assert!(
        source.contains("string.sub(text, (1) + 1, #text)"),
        "{source}"
    );
    assert!(source.contains("string.sub(text, (1) + 1, 3)"), "{source}");
}

#[test]
fn compiles_host_queries_and_join() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let dark = ctx.query_bool("noctalia", &["isDarkMode"], Vec::new());
    let value = ctx.query_value("noctalia", &["getSetting"], vec![json!("shell.lang")]);
    let score = ctx.fuzzy_score("abc", "abcdef");
    let dir = ctx.plugin_dir();
    let path = dir.join("icons");
    let parts = ctx.list_dir(&path);
    let joined = parts.join("/");
    Ok(vec![Op::log(format!("{dark} {value} {score} {path} {joined}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("local function host_query("), "{source}");
    assert!(
        source.contains("noctalia.fuzzyScore(\"abc\", \"abcdef\")"),
        "{source}"
    );
    assert!(source.contains("table.concat("), "{source}");
}

#[test]
fn read_file_reports_errors_like_a_result() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    if let Ok(raw) = ctx.read_file("/tmp/x") {
        return Ok(vec![Op::log(raw)]);
    }
    let Ok(exact) = ctx.read_file("/tmp/y") else {
        return Ok(Vec::new());
    };
    Ok(vec![Op::log(exact)])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(
        source.contains("local __t1, __t2 = noctalia.readFile(\"/tmp/x\")"),
        "{source}"
    );
    assert!(source.contains("local exact = __t"), "{source}");
}

#[test]
fn compiles_map_removals_and_unit_defaults() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut queries: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
    queries.insert("a".to_owned(), json!(1));
    let removed = queries.remove("a");
    let empty: std::collections::HashMap<String, Value> = Default::default();
    Ok(vec![Op::log(format!("{removed} {empty}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("= nil"), "{}", compiled.source);
}

#[test]
fn compiles_iterator_aggregates() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let items = ctx.config.get("items").and_then(Value::as_array).cloned().unwrap_or_default();
    let any = items.iter().any(|item| item.is_string());
    let all = items.iter().all(|item| item.is_string());
    let sum: f64 = items.len() as f64;
    let count = items.iter().count();
    let mut seen = 0;
    items.iter().for_each(|_| { seen += 1 });
    Ok(vec![Op::log(format!("{any} {all} {sum} {count} {seen}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("= true"), "{source}");
    assert!(source.contains("= false"), "{source}");
    assert!(source.contains("break"), "{source}");
}

#[test]
fn compiles_the_remaining_string_and_number_methods() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.payload_str("text").unwrap_or_default();
    let mut list = vec![text.clone()];
    let first = list.first().cloned().unwrap_or_default();
    list.push_back("tail".to_owned());
    let popped = list.pop().unwrap_or_default();
    let found = text.find("|").unwrap_or(0);
    let nan = (0.0f64 / 0.0).is_nan();
    let quotient = 7.0.div_euclid(2.0);
    let rounded = 1.5f64.round();
    let optional = ctx.config.get("name").cloned();
    let chosen = optional.clone().or(Some(json!("x")));
    let flag = optional.clone().is_none_or(|value| value.is_string());
    let upper = text.is_ascii_uppercase();
    let alnum = text.is_ascii_alphanumeric();
    let digit = text.is_ascii_digit();
    let space = text.is_whitespace();
    let started = text.trim_start();
    let ended = text.trim_end();
    let suffix = text.strip_suffix(".txt").unwrap_or_default();
    let bytes = text.as_bytes();
    let owned: String = text.clone().into();
    let sized = bytes.len();
    let pushed = pushed_value(text);
    Ok(vec![Op::log(format!("{first}{popped}{found}{nan}{quotient}{rounded}{chosen}{flag}{upper}{alnum}{digit}{space}{started}{ended}{suffix}{sized}{pushed}"))])
}
fn pushed_value(text: String) -> String {
    let mut buffer = String::new();
    buffer.push_str(&text);
    buffer
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("table.remove(list)"), "{source}");
    assert!(
        source.contains("string.find(text, \"|\", 1, true)"),
        "{source}"
    );
    assert!(source.contains("math.floor(7.0 / 2.0)"), "{source}");
    assert!(
        source.contains("~= nil then return")
            && source.contains("tostring(chosen)"),
        "{source}"
    );
}

#[test]
fn compiles_character_set_variants() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.payload_str("text").unwrap_or_default();
    let bytes = text.trim_matches(b' ');
    let packed = text.trim_matches(b" \t");
    let digits = text.trim_matches(|c: char| c.is_ascii_digit());
    let matched = text.trim_matches(|c: char| matches!(c, '-' | '+'));
    let wrapped = text.trim_matches(&['^', ']', '%', '-'][..]);
    Ok(vec![Op::log(format!("{bytes}{packed}{digits}{matched}{wrapped}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("[%^%]%%%-]"), "{source}");
    assert!(source.contains("0123456789"), "{source}");
    assert!(source.contains("[%-+]"), "{source}");
}

#[test]
fn bails_on_character_sets_it_cannot_read() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.payload_str("text").unwrap_or_default();
    let unknown = text.trim_matches(|c: char| c == 'a' && c != 'b');
    Ok(vec![Op::log(unknown)])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("string or char literal"),
        "{}",
        error.message
    );
}

#[test]
fn compiles_container_callbacks_and_bounds() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let list = ctx.config.get("list").and_then(Value::as_array).cloned();
    let owned = list.clone().map_or(Vec::new(), Vec::as_slice);
    let fresh = list.clone().unwrap_or_else(Map::new);
    let default = list.clone().unwrap_or_else(Vec::new);
    let empty = list.clone().is_some_and(Vec::is_empty);
    let bounded = ctx.config.get("n").and_then(Value::as_f64).unwrap_or(i64::MAX as f64);
    let endless = f64::MAX;
    let lowest = i64::MIN;
    let minimal = f64::MIN;
    Ok(vec![Op::log(format!("{owned}{fresh}{default}{empty}{bounded}{endless}{lowest}{minimal}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(!source.contains("math.huge"), "{source}");
    assert!(source.contains("1.7976931348623157e308"), "{source}");
    assert!(source.contains("9223372036854775807"), "{source}");
}

#[test]
fn bails_on_open_ended_list_slices() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let list = ctx.config.get("list").and_then(Value::as_array).cloned().unwrap_or_default();
    let tail = &list[1..];
    Ok(vec![Op::log(format!("{tail:?}"))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("string receiver"),
        "{}",
        error.message
    );
}

#[test]
fn compiles_option_or_with_an_explicit_nil_check() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let flag = ctx.config.get("flag").and_then(Value::as_bool);
    let chosen = flag.or(Some(false));
    Ok(vec![Op::log(format!("{chosen}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(!source.contains("(flag or "), "{source}");
    assert!(source.contains("~= nil then return"), "{source}");
    assert!(source.contains("return false end)()"), "{source}");
}

#[test]
fn bails_on_char_boundary_and_char_indices() {
    let boundary = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.payload_str("text").unwrap_or_default();
    let inside = text.is_char_boundary(1);
    Ok(vec![Op::log(format!("{inside}"))])
}
"#;
    let error =
        compile(&source(boundary), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("is_char_boundary"),
        "{}",
        error.message
    );

    let indices = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.payload_str("text").unwrap_or_default();
    for (at, ch) in text.char_indices() {
        let _ = (at, ch);
    }
    Ok(vec![Op::log(text)])
}
"#;
    let error =
        compile(&source(indices), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("char_indices"), "{}", error.message);
}

#[test]
fn bails_on_list_range_indexes() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let list = ctx.config.get("list").and_then(Value::as_array).cloned().unwrap_or_default();
    let part = &list[1..3];
    Ok(vec![Op::log(format!("{part}"))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error.message.contains("string receiver"),
        "{}",
        error.message
    );
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn compiles_then_some_with_an_eager_value() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let ready = true;
    let picked = ready.then_some("yes".to_owned());
    Ok(vec![Op::log(format!("{picked}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    let value = source.find("local __t1 = \"yes\"").expect("value temp");
    let condition = source.find("if ready then return __t1").expect("guard");
    assert!(value < condition, "{source}");
}

#[test]
fn compiles_exact_iterator_and_string_methods() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let values = vec![1, 2, 3];
    let found = values.iter().find(|value| **value == 2).is_some();
    let position = values.iter().position(|value| **value == 3).unwrap_or(0);
    let tail: Vec<i64> = values.iter().skip(1).take(1).copied().collect();
    let parts: Vec<&str> = "a/b/c".rsplit('/').collect();
    let pair = "name=value".split_once('=').is_some();
    let last = "left:right".rfind(':').unwrap_or(0);
    let mut header = vec!["a".to_owned()];
    header.extend_from_slice(&["b".to_owned()]);
    let tail_len = tail.len();
    let parts_len = parts.len();
    let header_len = header.len();
    Ok(vec![Op::log(format!("{found} {position} {tail_len} {parts_len} {pair} {last} {header_len}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("break"), "{}", compiled.source);
    assert!(
        compiled.source.contains("string.split"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("table.insert"),
        "{}",
        compiled.source
    );
}

#[test]
fn compiles_flatten_math_and_character_methods() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let nested: Option<Vec<i64>> = Some(vec![1, 2]);
    let mut total = 0;
    for value in nested.into_iter().flatten() {
        total += value;
    }
    let text = "abc";
    let hex = text.bytes().all(|byte| byte.is_ascii_hexdigit());
    let wave = (1.0_f64).sin() + (1.0_f64).cos();
    let infinite = wave.is_infinite();
    let whole = wave.trunc();
    let mut rows = vec![1, 2, 3];
    rows.retain(|value| *value > 1);
    rows.truncate(1);
    let rows_len = rows.len();
    Ok(vec![Op::log(format!("{total} {hex} {infinite} {whole} {rows_len}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(compiled.source.contains("math.sin"), "{}", compiled.source);
    assert!(compiled.source.contains("math.cos"), "{}", compiled.source);
    assert!(
        compiled.source.contains("table.remove"),
        "{}",
        compiled.source
    );
}

#[test]
fn bails_on_comparators_and_type_erased_maps() {
    for handler in [
        "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let mut values = vec![2, 1]; values.sort_by(|a, b| a.cmp(b)); Ok(vec![]) }",
        "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let cell = once_cell(); cell.get_or_init(|| 1); Ok(vec![]) }",
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("bail");
        assert!(error.line > 0 && error.column > 0, "{error:?}");
    }
}

#[test]
fn compiles_empty_vector_constructors() {
    let handler = r#"
use std::collections::VecDeque;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let list = Vec::<i64>::new();
    let queue = VecDeque::<i64>::new();
    Ok(vec![Op::log(format!("{}{}", list.len(), queue.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("local list = {}"), "{generated}");
    assert!(generated.contains("local queue = {}"), "{generated}");
}

#[test]
fn flattens_a_list_of_lists_into_one_list() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let nested: Vec<Vec<i64>> = vec![vec![1, 2], vec![3]];
    let mut total = 0;
    for value in nested.into_iter().flatten() {
        total += value;
    }
    Ok(vec![Op::log(format!("{total}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(
        generated.contains("for __j = 1, #__inner do"),
        "{generated}"
    );
    assert!(generated.contains("__out.__next = 1"), "{generated}");
}

#[test]
fn bails_on_find_after_flatten() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let nested: Vec<Vec<i64>> = vec![vec![1, 2], vec![3]];
    let found = nested.iter().flatten().find(|value| **value == 3).is_some();
    Ok(vec![Op::log(format!("{found}"))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bail");
    assert!(error.message.contains("flatten"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn unwraps_newtype_field_access_on_a_constructor_call() {
    let handler = r#"
struct Score(pub i64);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = Score(4).0;
    Ok(vec![Op::log(format!("{value}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("local value = 4"), "{generated}");
    assert!(!generated.contains("[1]"), "{generated}");
}

#[test]
fn checks_hex_digits_on_byte_receivers() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let byte = 65_u8;
    let hex = byte.is_ascii_hexdigit();
    Ok(vec![Op::log(format!("{hex}"))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("local __byte = byte"), "{generated}");
    assert!(generated.contains("__byte >= 48"), "{generated}");
    assert!(!generated.contains("string.find(byte"), "{generated}");
}

#[test]
fn compiles_vector_constructors_with_capacity() {
    let handler = r#"
use std::collections::VecDeque;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut list = Vec::<i64>::with_capacity(4);
    list.push(1);
    let queue = VecDeque::<i64>::with_capacity(2);
    let sizes = (list.len(), queue.len());
    Ok(vec![Op::log(format!("{}{}", sizes.0, sizes.1))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("local list = {}"), "{generated}");
    assert!(generated.contains("local queue = {}"), "{generated}");
}
