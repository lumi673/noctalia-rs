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
    assert!(compiled.source.contains("local __base, __part"));
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
    assert!(compiled.source.contains("for __key in pairs("));
    assert!(compiled.source.contains("table.sort(__t"));
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
        compiled.source.contains("noctalia.getenv(\"HOME\")"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("== nil then __t"),
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
        compiled.source.contains("= \"fallback\" end"),
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
        compiled.source.contains("local __base, __part"),
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
        compiled.source.contains("if __t5 ~= nil then"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("if __t5 ~= nil then")
            && compiled.source.contains("[#__t1 + 1] = __t5"),
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
        compiled.source.contains("parts[__t3]")
            && compiled.source.contains("parts.__next = __t3"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains(".__next or 1, #"),
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
            && source.contains("json.encode(chosen)"),
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
    let compiled =
        compile(&source(indices), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("utf8.codes(text)"),
        "{}",
        compiled.source
    );
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
    let handler = "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let cell = once_cell(); cell.get_or_init(|| 1); Ok(vec![]) }";
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bail");
    assert!(error.line > 0 && error.column > 0, "{error:?}");
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
    assert!(
        generated.contains(
            "local list = setmetatable({}, { __noctalia_array = true })"
        ),
        "{generated}"
    );
    assert!(
        generated.contains(
            "local queue = setmetatable({}, { __noctalia_array = true })"
        ),
        "{generated}"
    );
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
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("__t1 == nil and"),
        "{}",
        compiled.source
    );
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

#[test]
fn emits_typed_json_accessors() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = ctx.setting("payload");
    let text = value.as_str();
    let count = value.as_i64();
    let ratio = value.as_f64();
    let flag = value.as_bool();
    let items = value.as_array();
    let object = value.as_object();
    let number = value.as_number();
    let integer = value.is_i64() || value.is_u64();
    let kind = value.is_null() || value.is_object() || value.is_array();
    let _ = (text, count, ratio, flag, items, object, number);
    Ok(vec![Op::log(format!("{}|{}", integer, kind))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(
        generated.contains("type(value) == \"string\" and value or nil"),
        "{generated}"
    );
    assert!(generated.contains("value % 1 == 0"), "{generated}");
    assert!(
        generated.contains("value >= -9223372036854775808")
            && generated.contains("value <= 9223372036854775807"),
        "{generated}"
    );
    assert!(
        generated.contains("type(__value) == \"boolean\" then return __value"),
        "{generated}"
    );
    assert!(generated.contains("is_array(value)"), "{generated}");
    assert!(generated.contains("not is_array(value)"), "{generated}");
}

#[test]
fn emits_map_lookup_mutation_and_clone() {
    let handler = r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut map = Map::new();
    map.insert("a".to_owned(), json!(1));
    let known = map.contains_key("a");
    let hit = map.get("a");
    let mut slot = map.get_mut("a");
    let taken = slot.take();
    let keys = map.keys();
    let size = map.len();
    let mut copy = map.clone();
    copy.insert("b".to_owned(), json!(2));
    let counts = Map::from_iter([("x", json!(1)), ("y", json!(2))]);
    let _ = (known, hit, taken, keys, counts);
    Ok(vec![Op::log(format!("{}|{}", size, map.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("(map[\"a\"] ~= nil)"), "{generated}");
    assert!(
        generated.contains("local function count_keys(value)"),
        "{generated}"
    );
    assert!(generated.contains("deep_clone(map)"), "{generated}");
    assert!(generated.contains("for _, __pair in ipairs"), "{generated}");
}

#[test]
fn emits_entry_insert_and_take() {
    let handler = r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut map = Map::new();
    let hit = map.entry("hits".to_owned()).or_insert(json!(0));
    let made = map.entry("made".to_owned()).or_insert_with(|| json!({ "a": 1 }));
    map.entry("mod".to_owned()).and_modify(|value| json!(value)).or_insert(json!(3));
    let mut pending = Some(json!(4));
    let taken = pending.take();
    let _ = (hit, made, taken);
    Ok(vec![Op::log("done")])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let generated = &compiled.source;
    assert!(generated.contains("local __t1 = map"), "{generated}");
    assert!(generated.contains("if __t3 == nil then"), "{generated}");
    assert!(generated.contains("__t1[__t2] = __t3"), "{generated}");
    assert!(generated.contains("pending = nil"), "{generated}");
    assert!(generated.contains("if __t9 ~= nil then"), "{generated}");
}

#[test]
fn rejects_entry_default_without_a_value_type() {
    let handler = r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut map = Map::new();
    let hit = map.entry("hits".to_owned()).or_default();
    let _ = hit;
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("or_default"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0);
}

#[test]
fn rejects_a_json_null_stored_in_a_container() {
    for handler in [
        r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let null = Value::Null;
    let mut map = Map::new();
    map.insert("a".to_owned(), null);
    Ok(vec![Op::log("done")])
}
"#,
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let null = json!(null);
    let mut items = Vec::new();
    items.push(null);
    let _ = ctx;
    Ok(vec![Op::log("done")])
}
"#,
        r#"
use serde_json::Map;
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let picked = if ctx.now_ms > 0 { json!(1) } else { Value::Null };
    let mut map = Map::new();
    map.insert("a".to_owned(), picked);
    Ok(vec![Op::log("done")])
}
"#,
        r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut map = Map::new();
    map.insert("a".to_owned(), Value::Null);
    Ok(vec![Op::log("done")])
}
"#,
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut items = Vec::new();
    items.push(Value::Null);
    Ok(vec![Op::log("done")])
}
"#,
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut props = Map::new();
    props["width"] = Value::Null;
    Ok(vec![Op::log("done")])
}
"#,
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("bails");
        assert!(error.message.contains("json null"), "{}", error.message);
        assert!(error.line > 0 && error.column > 0, "{error:?}");
    }
}

#[test]
fn answers_integer_accessors_per_json_type() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let float = json!(3.0);
    let integer = json!(3);
    let shaped = ctx.setting("payload");
    let a = float.as_i64();
    let b = float.is_i64();
    let c = integer.as_i64();
    let d = shaped.as_u64();
    let _ = (a, b, c, d);
    Ok(vec![Op::log("done")])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("local a = nil"), "{source}");
    assert!(source.contains("local b = (false)"), "{source}");
    assert!(source.contains("local c = integer"), "{source}");
    assert!(source.contains("% 1 == 0"), "{source}");
}

#[test]
fn insert_returns_the_previous_slot() {
    let handler = r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut map = Map::new();
    map.insert("a".to_owned(), json!(1));
    let previous = map.insert("a".to_owned(), json!(2));
    let text = previous.map(|value| value.to_string()).unwrap_or_default();
    Ok(vec![Op::log(text)])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("local __t"), "{source}");
    assert!(
        source.contains("[\"a\"] = 2") && source.contains("map[\"a\"]"),
        "{source}"
    );
}

#[test]
fn is_empty_and_len_count_map_keys() {
    let handler = r#"
use serde_json::Map;
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut map = Map::new();
    map.insert("a".to_owned(), json!(1));
    let empty = map.is_empty();
    let size = map.len();
    let value = ctx.setting("payload");
    let hollow = value.is_empty();
    Ok(vec![Op::log(format!("{}{}{}", empty, size, hollow))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("(count_keys(map) == 0)"), "{source}");
    assert!(source.contains("count_keys(map)"), "{source}");
    assert!(
        source.contains("type(value) == \"table\" and count_keys"),
        "{source}"
    );
}

#[test]
fn map_values_iterate_in_key_order() {
    let handler = r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut map = Map::new();
    map.insert("b".to_owned(), json!(2));
    map.insert("a".to_owned(), json!(1));
    let first = map.keys().next().unwrap_or_default();
    let mut names = Vec::new();
    for (key, value) in map.iter() {
        let _ = value;
        names.push(key);
    }
    let mut values = Vec::new();
    for value in map.values() {
        values.push(value);
    }
    let _ = (first, values);
    Ok(vec![Op::log(names.join(","))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let source = &compiled.source;
    assert!(source.contains("table.sort(__keys"), "{source}");
    assert!(source.contains("__noctalia_cursor = 0"), "{source}");
    assert!(source.contains("for __key in pairs(map) do"), "{source}");
    assert!(source.contains("local key = __t"), "{source}");
}

#[test]
fn null_flag_follows_value_parameters_and_defaults() {
    for handler in [
        r#"
use serde_json::Map;
fn put(map: &mut Map, value: Value) {
    map.insert("k".to_owned(), value);
}
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut map = Map::new();
    let value = Value::Null;
    put(&mut map, value);
    Ok(vec![Op::log("done")])
}
"#,
        r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut map = Map::new();
    let value = None.unwrap_or_default();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#,
        r#"
use serde_json::Map;
fn text(_value: &str) {}
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut map = Map::new();
    let value = Value::Null;
    text(value.as_str().unwrap_or_default());
    map.insert("k".to_owned(), json!(1));
    Ok(vec![Op::log("done")])
}
"#,
    ] {
        let result =
            compile(&source(handler), &options("handler", EntryKind::Service));
        if handler.contains("fn put")
            || handler.contains("let value = None.unwrap_or_default()")
        {
            let error = result.expect_err("bails");
            assert!(error.message.contains("json null"), "{}", error.message);
            assert!(error.line > 0 && error.column > 0, "{error:?}");
        } else {
            result.expect("compiles");
        }
    }
}

#[test]
fn a_non_null_fallback_clears_the_null_flag() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut items = Vec::new();
    let fallback = None.unwrap_or_else(|| json!(1));
    items.push(fallback);
    let chosen = if false { json!(null) } else { Value::Null }
        .unwrap_or_else(|| json!(2));
    items.push(chosen);
    Ok(vec![Op::log(format!("{}", items.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("table.insert(items, fallback)")
            && compiled.source.contains("table.insert(items, chosen)"),
        "{}",
        compiled.source
    );
}

#[test]
fn rejects_a_null_fallback_from_unwrap_or_else() {
    let handler = r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut map = Map::new();
    let missing: Option<Value> = None;
    let value = missing.unwrap_or_else(|| Value::Null);
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("json null"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn map_iter_closure_bindings_keep_the_string_key_type() {
    let handler = r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut map = Map::new();
    map.insert("ab".to_owned(), json!(1));
    let lens: Vec<usize> = map.iter().map(|(key, _)| key.len()).collect();
    Ok(vec![Op::log(format!("{}", lens.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("#key")
            && !compiled.source.contains("count_keys(key)"),
        "{}",
        compiled.source
    );
}

#[test]
fn rejects_a_null_from_a_some_null_pattern() {
    for handler in [
        r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = Some(Value::Null);
    let mut map = Map::new();
    if let Some(inner) = value {
        map.insert("k".to_owned(), inner);
    }
    Ok(vec![Op::log("done")])
}
"#,
        r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = Some(Value::Null);
    let mut map = Map::new();
    match value {
        Some(Value::Null) => map.insert("k".to_owned(), Value::Null),
        _ => None,
    };
    Ok(vec![Op::log("done")])
}
"#,
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("bails");
        assert!(error.message.contains("json null"), "{}", error.message);
        assert!(error.line > 0 && error.column > 0, "{error:?}");
    }
}

#[test]
fn rejects_a_null_from_a_tuple_initializer() {
    let handler = r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let pair = (json!(1), Value::Null);
    let (_, value) = pair;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("json null"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn rejects_a_null_from_a_struct_field() {
    let handler = r#"
use serde_json::Map;
struct Holder { value: Value }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let holder = Holder { value: Value::Null };
    let mut map = Map::new();
    map.insert("k".to_owned(), holder.value);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("json null"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn nested_tuple_unpacking_keeps_nullness() {
    for handler in [
        r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let nested = ((json!(1), Value::Null), json!(2));
    let (inner, _) = nested;
    let (_, value) = inner;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#,
        r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let three = ((json!(1), (json!(2), Value::Null)), json!(3));
    let (pair, _) = three;
    let (_, inner) = pair;
    let (_, value) = inner;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#,
        r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let one = ((Value::Null,),);
    let (pair,) = one;
    let (value,) = pair;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#,
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("bails");
        assert!(error.message.contains("json null"), "{}", error.message);
        assert!(error.line > 0 && error.column > 0, "{error:?}");
    }
}

#[test]
fn nested_non_null_unpacking_still_compiles() {
    let handler = r#"
use serde_json::Map;
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let nested = ((json!(1), json!(2)), json!(3));
    let (inner, _) = nested;
    let (_, value) = inner;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("map[\"k\"] ="),
        "{}",
        compiled.source
    );
}

#[test]
fn newtype_field_keeps_the_constructor_nullness() {
    for handler in [
        r#"
use serde_json::Map;
struct Holder(Value);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let holder = Holder(Value::Null);
    let mut map = Map::new();
    map.insert("k".to_owned(), holder.0);
    Ok(vec![Op::log("done")])
}
"#,
        r#"
use serde_json::Map;
struct Holder(Value);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let holder = Holder(json!(1));
    let copy = holder;
    let mut map = Map::new();
    map.insert("k".to_owned(), copy.0);
    Ok(vec![Op::log("done")])
}
"#,
    ] {
        let result =
            compile(&source(handler), &options("handler", EntryKind::Service));
        if handler.contains("Value::Null") {
            let error = result.expect_err("bails");
            assert!(error.message.contains("json null"), "{}", error.message);
            assert!(error.line > 0 && error.column > 0, "{error:?}");
        } else {
            let compiled = result.expect("compiles");
            assert!(
                compiled.source.contains("map[\"k\"] ="),
                "{}",
                compiled.source
            );
        }
    }
}

#[test]
fn named_struct_field_stays_null_aware() {
    let handler = r#"
use serde_json::Map;
struct Holder { value: Value }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let holder = Holder { value: Value::Null };
    let mut map = Map::new();
    map.insert("k".to_owned(), holder.value);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("json null"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn tuple_shapes_from_fields_and_helpers_keep_nullness() {
    for handler in [
        r#"
use serde_json::Map;
struct Holder { pair: (Value, Value) }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let holder = Holder { pair: (json!(1), Value::Null) };
    let (_, value) = holder.pair;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#,
        r#"
use serde_json::Map;
fn make() -> (Value, Value) {
    (json!(1), Value::Null)
}
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let (_, value) = make();
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#,
        r#"
use serde_json::Map;
struct Holder { pair: (Value, Value) }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let holder = Holder { pair: (json!(1), (json!(2), Value::Null)) };
    let copy = holder;
    let (_, inner) = copy.pair;
    let (_, value) = inner;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#,
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("bails");
        assert!(error.message.contains("json null"), "{}", error.message);
        assert!(error.line > 0 && error.column > 0, "{error:?}");
    }
}

#[test]
fn tuple_fields_and_helpers_without_null_still_compile() {
    let handler = r#"
use serde_json::Map;
struct Holder { pair: (Value, Value) }
fn make() -> (Value, Value) { (json!(1), json!(2)) }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let holder = Holder { pair: (json!(1), json!(2)) };
    let (_, from_field) = holder.pair;
    let (_, from_helper) = make();
    let mut map = Map::new();
    map.insert("a".to_owned(), from_field);
    map.insert("b".to_owned(), from_helper);
    Ok(vec![Op::log("done")])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("map[\"a\"] ="),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("map[\"b\"] ="),
        "{}",
        compiled.source
    );
}

#[test]
fn nested_newtype_field_keeps_nullness() {
    for handler in [
        r#"
use serde_json::Map;
struct Inner(Value);
struct Outer(Inner);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut map = Map::new();
    map.insert("k".to_owned(), Outer(Inner(Value::Null)).0.0);
    Ok(vec![Op::log("done")])
}
"#,
        r#"
use serde_json::Map;
struct Inner(Value);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let holder = Inner(Value::Null);
    let inner = holder;
    let mut map = Map::new();
    map.insert("k".to_owned(), inner.0);
    Ok(vec![Op::log("done")])
}
"#,
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("bails");
        assert!(error.message.contains("json null"), "{}", error.message);
        assert!(error.line > 0 && error.column > 0, "{error:?}");
    }
}

#[test]
fn nested_newtype_and_tuple_struct_controls_compile() {
    let handler = r#"
use serde_json::Map;
struct Inner(Value);
struct Outer(Inner);
struct Pair(Value, Value);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let outer = Outer(Inner(json!(1)));
    let inner = outer.0;
    let pair = Pair(json!(2), json!(3));
    let mut map = Map::new();
    map.insert("a".to_owned(), inner.0);
    map.insert("b".to_owned(), pair.0);
    Ok(vec![Op::log("done")])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("map[\"a\"] ="),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("map[\"b\"] ="),
        "{}",
        compiled.source
    );
}

#[test]
fn helper_return_shape_comes_from_the_inlined_value() {
    for body in [
        r#"let pair = (json!(1), Value::Null); pair"#,
        r#"(json!(1), Value::Null)"#,
    ] {
        let handler = format!(
            r#"
use serde_json::Map;
fn make() -> (Value, Value) {{ {body} }}
fn handler(_ctx: &Context) -> Result<Vec<Op>> {{
    let (_, value) = make();
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}}
"#
        );
        let error =
            compile(&source(&handler), &options("handler", EntryKind::Service))
                .expect_err("bails");
        assert!(error.message.contains("json null"), "{}", error.message);
        assert!(error.line > 0 && error.column > 0, "{error:?}");
    }
}

#[test]
fn named_field_shape_comes_from_the_initializer() {
    let handler = r#"
use serde_json::Map;
fn make() -> (Value, Value) { (json!(1), Value::Null) }
struct Holder { pair: (Value, Value) }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let holder = Holder { pair: make() };
    let (_, value) = holder.pair;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(
        error
            .message
            .contains("tuple initializer with known element types")
            || error.message.contains("json null"),
        "{}",
        error.message
    );
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn reassignment_refreshes_tuple_shapes() {
    let handler = r#"
use serde_json::Map;
struct Holder { pair: (Value, Value) }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let holder = Holder { pair: (json!(1), json!(2)) };
    let other = Holder { pair: (json!(3), Value::Null) };
    let mut copy = holder;
    copy = other;
    let (_, value) = copy.pair;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("json null"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn tuple_struct_element_shapes_are_read_by_index() {
    let handler = r#"
use serde_json::Map;
struct Pair(Value, Value);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let pair = Pair(json!(1), Value::Null);
    let mut map = Map::new();
    map.insert("k".to_owned(), pair.1);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("json null"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn unknown_static_tuple_fails_at_destructuring() {
    let handler = r#"
use serde_json::Map;
static PAIR: (Value, Value) = (Value::Null, Value::Null);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let (_, value) = PAIR;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("json null"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn round_eight_non_null_controls_compile() {
    let handler = r#"
use serde_json::Map;
fn make() -> (Value, Value) { let pair = (json!(1), json!(2)); pair }
struct Holder { pair: (Value, Value) }
struct Pair(Value, Value);
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let (_, from_helper) = make();
    let holder = Holder { pair: (json!(5), json!(6)) };
    let (_, from_field) = holder.pair;
    let pair = Pair(json!(3), json!(4));
    let copy = pair;
    let mut map = Map::new();
    map.insert("a".to_owned(), from_helper);
    map.insert("b".to_owned(), from_field);
    map.insert("c".to_owned(), copy.1);
    Ok(vec![Op::log("done")])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    for key in ["a", "b", "c"] {
        assert!(
            compiled.source.contains(&format!("map[\"{key}\"] =")),
            "{}",
            compiled.source
        );
    }
}

#[test]
fn helper_parameter_and_local_shapes_are_preserved() {
    for handler in [
        r#"
use serde_json::Map;
fn passthrough(t: (Value, Value)) -> (Value, Value) { t }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let t = (json!(1), Value::Null);
    let (_, value) = passthrough(t);
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#,
        r#"
use serde_json::Map;
fn make() -> (Value, Value) { let pair = (json!(1), Value::Null); pair }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let t = make();
    let (_, value) = t;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#,
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("bails");
        assert!(error.message.contains("json null"), "{}", error.message);
        assert!(error.line > 0 && error.column > 0, "{error:?}");
    }
}

#[test]
fn field_shape_from_helper_initializer_is_carried() {
    let handler = r#"
use serde_json::Map;
fn make() -> (Value, Value) { (json!(1), Value::Null) }
struct Holder { pair: (Value, Value) }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let holder = Holder { pair: make() };
    let t = holder.pair;
    let (_, value) = t;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("json null"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn assignment_refreshes_struct_field_shapes() {
    let handler = r#"
use serde_json::Map;
struct Holder { pair: (Value, Value) }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut holder = Holder { pair: (json!(1), json!(2)) };
    holder = Holder { pair: (json!(3), Value::Null) };
    let (_, value) = holder.pair;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("json null"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn local_static_tuple_shape_is_recorded() {
    let handler = r#"
use serde_json::Map;
fn make() -> Value { Value::Null }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    static PAIR: (Value, Value) = (make(), Value::Null);
    let (_, value) = PAIR;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("json null"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn round_nine_non_null_controls_compile() {
    let handler = r#"
use serde_json::Map;
fn make() -> (Value, Value) { (json!(1), json!("a")) }
fn passthrough(t: (Value, Value)) -> (Value, Value) { t }
struct Holder { pair: (Value, Value) }
static PAIR: (Value, Value) = (json!(2), json!("s"));
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let t = make();
    let (_, from_local) = t;
    let (_, from_helper) = passthrough(make());
    let mut holder = Holder { pair: (json!(3), json!(4)) };
    holder = Holder { pair: make() };
    let (_, from_field) = holder.pair;
    let (_, from_static) = PAIR;
    let mut map = Map::new();
    map.insert("a".to_owned(), from_local);
    map.insert("b".to_owned(), from_helper);
    map.insert("c".to_owned(), from_field);
    map.insert("d".to_owned(), from_static);
    Ok(vec![Op::log("done")])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    for key in ["a", "b", "c", "d"] {
        assert!(
            compiled.source.contains(&format!("map[\"{key}\"] =")),
            "{}",
            compiled.source
        );
    }
}

#[test]
fn literal_tuple_argument_shape_reaches_the_binding() {
    let handler = r#"
use serde_json::Map;
fn passthrough(t: (Value, Value)) -> (Value, Value) { t }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let (_, value) = passthrough((json!(1), Value::Null));
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("json null"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn call_assignment_refreshes_shape_to_null() {
    let handler = r#"
use serde_json::Map;
fn make_null() -> (Value, Value) { (json!(1), Value::Null) }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut t = (json!(1), json!(2));
    t = make_null();
    let (_, value) = t;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("json null"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn local_static_tuple_with_helper_element_is_rejected() {
    let handler = r#"
use serde_json::Map;
fn make() -> Value { Value::Null }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    static PAIR: (Value, Value) = (json!(1), make());
    let (_, value) = PAIR;
    let mut map = Map::new();
    map.insert("k".to_owned(), value);
    Ok(vec![Op::log("done")])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("bails");
    assert!(error.message.contains("json null"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn round_ten_non_null_controls_compile() {
    let handler = r#"
use serde_json::Map;
fn passthrough(t: (Value, Value)) -> (Value, Value) { t }
fn make_pair() -> (Value, Value) { (json!(1), json!(2)) }
fn make_value() -> Value { json!(3) }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let (_, from_arg) = passthrough((json!(1), json!(2)));
    let mut t = (json!(1), json!(2));
    t = make_pair();
    let (_, from_call) = t;
    static PAIR: (Value, Value) = (json!(4), make_value());
    let (_, from_static) = PAIR;
    let mut map = Map::new();
    map.insert("a".to_owned(), from_arg);
    map.insert("b".to_owned(), from_call);
    map.insert("c".to_owned(), from_static);
    Ok(vec![Op::log("done")])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    for key in ["a", "b", "c"] {
        assert!(
            compiled.source.contains(&format!("map[\"{key}\"] =")),
            "{}",
            compiled.source
        );
    }
}

#[test]
fn lowers_numeric_edge_methods() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let clamped = 7_i64.saturating_sub(9);
    let wrapped = 7_i64.wrapping_add(2);
    let divided = 7_i64.div_ceil(3);
    let power = 2.0_f64.powi(3);
    let decimal = 100.0_f64.log10();
    let signed = (-1.0_f64).is_sign_positive();
    Ok(vec![Op::log(format!("{} {} {} {} {} {}", clamped, wrapped, divided, power, decimal, signed))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let emitted = &compiled.source;
    assert!(emitted.contains("local clamped = (-2)"), "{emitted}");
    assert!(emitted.contains("local wrapped = (9)"), "{emitted}");
    assert!(
        emitted.contains("math.floor(7), math.floor(3)"),
        "{emitted}"
    );
    assert!(emitted.contains("math.pow(2.0, 3)"), "{emitted}");
    assert!(emitted.contains("math.log(100.0, 10)"), "{emitted}");
    assert!(emitted.contains("(1 / __v) > 0"), "{emitted}");
}

#[test]
fn lowers_remaining_string_methods() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.config_str("message").unwrap_or_default();
    let pieces = text.splitn(2, ',').collect::<Vec<_>>();
    let replaced = text.replacen("a", "b", 1);
    let divided = text.rsplit_once('/');
    let terminated = text.split_terminator(',');
    let same = text.eq_ignore_ascii_case("ABC");
    let indexed = text.char_indices().collect::<Vec<_>>();
    let matches = text.match_indices("a").collect::<Vec<_>>();
    let mut owned = text.clone();
    owned.truncate(2);
    let _ = (divided, terminated, matches);
    Ok(vec![Op::log(format!("{} {} {} {} {}", pieces.len(), replaced, same, indexed.len(), owned))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let emitted = &compiled.source;
    assert!(
        emitted.contains("string.find(__s, __sep, __start, true)"),
        "{emitted}"
    );
    assert!(emitted.contains("table.concat(__out)"), "{emitted}");
    assert!(
        emitted.contains("if not __last then return nil end"),
        "{emitted}"
    );
    assert!(emitted.contains("__parts[#__parts] == \"\""), "{emitted}");
    assert!(emitted.contains("string.lower("), "{emitted}");
    assert!(emitted.contains("utf8.codes("), "{emitted}");
    assert!(emitted.contains("__s, __n = owned"), "{emitted}");
}

#[test]
fn lowers_sort_and_iterator_adaptors() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut names = vec!["b".to_owned(), "a".to_owned()];
    names.sort();
    names.sort_by(|left, right| left.cmp(right));
    let mut numbers = vec![1, 2, 3];
    numbers.retain(|number| *number > 1);
    let pairs = numbers.iter().enumerate().collect::<Vec<_>>();
    let backwards = numbers.iter().rev().collect::<Vec<_>>();
    let taken = numbers.iter().take_while(|number| **number < 3).collect::<Vec<_>>();
    let zipped = numbers.iter().zip(names.iter()).collect::<Vec<_>>();
    let found = names.iter().find(|name| name.as_str() == "a");
    let _ = (pairs, backwards, taken, zipped, found);
    Ok(Vec::new())
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let emitted = &compiled.source;
    assert!(emitted.contains("table.sort(__t1, function"), "{emitted}");
    assert!(
        emitted.contains("== \"Less\" then return true"),
        "{emitted}"
    );
    assert!(emitted.contains("table.remove(numbers"), "{emitted}");
    assert!(emitted.contains(", 1, -1 do"), "{emitted}");
    assert!(emitted.contains("__t16 = __t16 + 1"), "{emitted}");
    assert!(emitted.contains(" = false end"), "{emitted}");
    assert!(emitted.contains("local zipped ="), "{emitted}");
}

#[test]
fn lowers_cells_ranges_and_bool_then() {
    let handler = r#"
static CELL: OnceLock<i64> = OnceLock::new();
fn cell() -> i64 {
    *CELL.get_or_init(|| 3)
}
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = ctx.config_str("count").unwrap_or_default().parse::<f64>().unwrap_or(0.0);
    let inside = (0.0..300.0).contains(&value);
    let kept = (!String::new().is_empty()).then(|| 1);
    let nth = (0..4).map(|index| index + 1).nth(2);
    Ok(vec![Op::log(format!("{} {} {} {}", cell(), inside, kept.unwrap_or(0), nth.unwrap_or(0)))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let emitted = &compiled.source;
    assert!(emitted.contains("__static_main__CELL == nil"), "{emitted}");
    assert!(
        emitted.contains("__v >= (0.0) and __v < (300.0)"),
        "{emitted}"
    );
    assert!(emitted.contains("local kept ="), "{emitted}");
    assert!(emitted.contains("(0), ((4) - 1) do"), "{emitted}");
}

#[test]
fn rejects_inclusive_range_contains() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let value = 2;
    if (0..=3).contains(&value) {
        return Ok(vec![Op::log("yes")]);
    }
    Ok(Vec::new())
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("inclusive range is rejected");
    let rendered = error.render("main.rs");
    assert!(
        rendered.contains("inclusive ranges are not supported"),
        "{rendered}"
    );
    assert!(error.line > 0 && error.column > 0, "{rendered}");
}

#[test]
fn lowers_remaining_numeric_string_and_iterator_branches() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let product = 3_i64.saturating_mul(4);
    let underflow = 3_i64.wrapping_sub(9);
    let order = 3_i64.cmp(&4);
    let left = 1.0_f64;
    let right = 2.0_f64;
    let partial = left.partial_cmp(&right);
    let total = left.total_cmp(&right);
    let text = ctx.config_str("message").unwrap_or_default();
    let repeated = text.repeat(2);
    let utf8 = "é".len_utf8();
    let cascade = "a".cmp("b").then_with(|| "x".cmp("y")).reverse();
    let flat = vec![vec![1, 2], vec![3]].into_iter().flat_map(|items| items).collect::<Vec<_>>();
    let mut words = vec!["aaa".to_owned(), "b".to_owned()];
    words.sort_by_key(|word| word.len());
    let _ = (partial, total, utf8, cascade, flat);
    Ok(vec![Op::log(format!("{} {} {} {} {}", product, underflow, order, repeated, words.len()))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    let emitted = &compiled.source;
    assert!(emitted.contains("local product = (12)"), "{emitted}");
    assert!(emitted.contains("local underflow = (-6)"), "{emitted}");
    assert!(
        emitted.contains("return nil end; if __a < __b"),
        "{emitted}"
    );
    assert!(
        emitted.contains("if (1 / __a) < (1 / __b) then"),
        "{emitted}"
    );
    assert!(emitted.contains("string.rep("), "{emitted}");
    assert!(emitted.contains("utf8.codes(__s)"), "{emitted}");
    assert!(
        emitted.contains("== \"Less\" then return \"Greater\""),
        "{emitted}"
    );
    assert!(emitted.contains("table.sort(__t"), "{emitted}");
}

#[test]
fn accepts_known_typed_string_arguments() {
    let handler = r#"
struct State { name: String }
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let state = State { name: ctx.config_str("message").unwrap_or_default() };
    if ctx.command_exists(&state.name) {
        return Ok(vec![Op::log(state.name)]);
    }
    Ok(Vec::new())
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("noctalia.commandExists(state.name)"),
        "{}",
        compiled.source
    );
}

#[test]
fn rejects_string_arguments_without_a_known_type() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let names = vec!["a".to_owned()];
    if ctx.command_exists(&names[0]) {
        return Ok(vec![Op::log("yes")]);
    }
    Ok(Vec::new())
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("unknown string type bails");
    assert!(
        error
            .message
            .contains("expected a string argument with a known type"),
        "{}",
        error.message
    );
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn review_l1_stable_sort_caches_keys_and_rejects_unstable() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut names = vec!["b".to_owned(), "a".to_owned()];
    names.sort_by_key(|name| name.len());
    Ok(Vec::new())
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("for __t6 = 1, #names do"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("table.sort("),
        "{}",
        compiled.source
    );

    let unstable = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut names = vec!["b".to_owned(), "a".to_owned()];
    names.sort_unstable();
    Ok(Vec::new())
}
"#;
    let error =
        compile(&source(unstable), &options("handler", EntryKind::Service))
            .expect_err("unstable sort bails");
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn review_l2_retain_collects_then_removes_in_reverse() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut numbers = vec![1, 2, 3];
    numbers.retain(|number| *number > 1);
    Ok(Vec::new())
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("for __t1 = 1, #numbers do"),
        "{}",
        compiled.source
    );
    assert!(
        compiled
            .source
            .contains("table.remove(numbers, __t2[__t3])"),
        "{}",
        compiled.source
    );
}

#[test]
fn review_l3_rejects_unbounded_range_iterators() {
    for handler in [
        "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let values = (..4).map(|n| n + 1).collect::<Vec<_>>(); let _ = values; Ok(Vec::new()) }",
        "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let values = (4..).map(|n| n + 1).collect::<Vec<_>>(); let _ = values; Ok(Vec::new()) }",
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("unbounded range bails");
        assert!(error.message.contains("explicit"), "{}", error.message);
        assert!(error.line > 0 && error.column > 0, "{error:?}");
    }
}

#[test]
fn review_l4_rejects_unproven_replacen_pattern() {
    for handler in [
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { let text = ctx.config_str(\"message\").unwrap_or_default(); let out = text.replacen(\"\", \"x\", 1); let _ = out; Ok(Vec::new()) }",
        "fn handler(ctx: &Context) -> Result<Vec<Op>> { let text = ctx.config_str(\"message\").unwrap_or_default(); let out = text.replacen(&text, \"x\", 1); let _ = out; Ok(Vec::new()) }",
    ] {
        let error =
            compile(&source(handler), &options("handler", EntryKind::Service))
                .expect_err("replacen pattern bails");
        assert!(error.message.contains("replacen()"), "{}", error.message);
        assert!(error.line > 0 && error.column > 0, "{error:?}");
    }
}

#[test]
fn review_l5_truncate_checks_utf8_boundaries() {
    let handler = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut text = ctx.config_str("message").unwrap_or_default();
    text.truncate(2);
    Ok(vec![Op::log(text)])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("not a char boundary"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("byte index out of bounds"),
        "{}",
        compiled.source
    );
}

#[test]
fn review_l6_wrapping_uses_i64_arithmetic() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = i64::MAX.wrapping_add(1);
    Ok(vec![Op::log(format!("{}", value))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("(-9223372036854775808)"),
        "{}",
        compiled.source
    );
}

#[test]
fn review_l7_get_or_init_local_alias_writes_through() {
    let handler = r#"
static CELL: OnceLock<i64> = OnceLock::new();
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let slot = &CELL;
    let value = slot.get_or_init(|| 3);
    Ok(vec![Op::log(format!("{}", value))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("if __static_main__CELL == nil"),
        "{}",
        compiled.source
    );
    assert!(
        !compiled.source.contains("if __local_"),
        "{}",
        compiled.source
    );
}

#[test]
fn review_l8_to_lowercase_rejects_non_ascii_literal() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let text = "İ".to_lowercase();
    Ok(vec![Op::log(text)])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("non-ASCII literal bails");
    assert!(error.message.contains("non-ASCII"), "{}", error.message);
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn review_l9_min_max_ignore_nan() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = f64::NAN.max(2.0);
    Ok(vec![Op::log(format!("{}", value))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled
            .source
            .contains("if __a ~= __a then return __b end"),
        "{}",
        compiled.source
    );
}

#[test]
fn review_l10_parse_returns_a_result() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = "42".parse::<i64>().unwrap_or(0);
    Ok(vec![Op::log(format!("{}", value))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("tonumber(__s)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("invalid digit found in string"),
        "{}",
        compiled.source
    );
}

#[test]
fn review_round2_integer_boundaries() {
    for literal in [
        "9223372036854775807",
        "9223372036854775808",
        "-9223372036854775809",
        "9007199254740993",
    ] {
        let handler = format!(
            r#"fn handler(_ctx: &Context) -> Result<Vec<Op>> {{
    let value = "{literal}".parse::<i64>().unwrap_or(0);
    Ok(vec![Op::log(format!("{{}}", value))])
}}
"#
        );
        let error =
            compile(&source(&handler), &options("handler", EntryKind::Service))
                .expect_err("non-exact or out-of-range parse bails");
        assert!(error.line > 0 && error.column > 0, "{error:?}");
    }

    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let a = (i64::MAX - 1).wrapping_add(2);
    let b = i64::MIN.wrapping_sub(2);
    let c = i64::MIN.div_ceil(-1);
    Ok(vec![Op::log(format!("{} {} {}", a, b, c))])
}
"#;
    let error =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect_err("div_ceil overflow bails");
    assert!(
        error.message.contains("overflow") || error.message.contains("exactly"),
        "{}",
        error.message
    );
}

#[test]
fn review_round2_get_or_init_receiver_classes() {
    let parameter = r#"
static CELL: OnceLock<i64> = OnceLock::new();
fn take(cell: &OnceLock<i64>) -> i64 { *cell.get_or_init(|| 3) }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let v = take(&CELL);
    Ok(vec![Op::log(format!("{}", v))])
}
"#;
    let compiled =
        compile(&source(parameter), &options("handler", EntryKind::Service))
            .expect("parameter alias writes through");
    assert!(
        compiled.source.contains("__static_main__CELL = 3"),
        "{}",
        compiled.source
    );

    let call_result = r#"
static CELL: OnceLock<i64> = OnceLock::new();
fn cell() -> &'static OnceLock<i64> { &CELL }
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let v = *cell().get_or_init(|| 3);
    Ok(vec![Op::log(format!("{}", v))])
}
"#;
    let error = compile(
        &source(call_result),
        &options("handler", EntryKind::Service),
    )
    .expect_err("call-result receiver bails");
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}

#[test]
fn review_round3_width_and_pattern_fixes() {
    let handler = r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let a = u8::MAX.saturating_add(1);
    let b = u32::MAX.wrapping_add(1);
    let c = "0000000000000000042".parse::<i64>().unwrap_or(0);
    let d = "1.5".parse::<f64>().unwrap_or(0.0);
    let e = "1e2".parse::<f64>().unwrap_or(0.0);
    Ok(vec![Op::log(format!("{} {} {} {} {}", a, b, c, d, e))])
}
"#;
    let compiled =
        compile(&source(handler), &options("handler", EntryKind::Service))
            .expect("compiles");
    assert!(
        compiled.source.contains("local a = (255)"),
        "{}",
        compiled.source
    );
    assert!(
        compiled.source.contains("local b = (0)"),
        "{}",
        compiled.source
    );

    let predicate = r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = ctx.config_str("message").unwrap_or_default();
    let out = text.splitn(2, |ch| ch == ',');
    let _ = out;
    Ok(Vec::new())
}
"#;
    let error =
        compile(&source(predicate), &options("handler", EntryKind::Service))
            .expect_err("predicate separator bails");
    assert!(error.line > 0 && error.column > 0, "{error:?}");
}
