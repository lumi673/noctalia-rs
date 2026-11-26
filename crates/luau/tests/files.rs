//! The `std::fs` and `std::path` translations, driven as real Luau.
//!
//! These tests compile a Rust handler, then run the generated script through
//! `noctalia_rs_luau::dispatch` with scripted host answers. A translation that
//! compiles but does not parse or computes something else than the Rust code
//! fails here.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use noctalia_rs::{Context, Parts};
use noctalia_rs_compiler::{
    EntryKind, NativeMode, Options, compile as compile_rust,
};
use serde_json::{Value, json};

struct QueryDir(PathBuf);

impl QueryDir {
    fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("noctalia-files-{nanos}"));
        fs::create_dir_all(root.join(".noctalia-rs/spool")).expect("spool");
        fs::create_dir_all(root.join(".noctalia-rs/query")).expect("query");
        Self(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// Answers one query with an explicit value and error string.
    #[expect(
        clippy::too_many_arguments,
        reason = "one response carries the id, value and error"
    )]
    fn respond(&self, id: usize, value: Value, error: Option<&str>) {
        let body = json!({ "ok": true, "value": value, "error": error });
        let path = self
            .0
            .join(".noctalia-rs/query")
            .join(format!("noctalia_rs_luau_{id}.json"));
        fs::write(path, body.to_string()).expect("response");
    }

    /// Answers one query with an error, which the script sees in the second
    /// return value.
    fn fail(&self, id: usize, message: &str) {
        let body =
            json!({ "ok": true, "value": Value::Null, "error": message });
        let path = self
            .0
            .join(".noctalia-rs/query")
            .join(format!("noctalia_rs_luau_{id}.json"));
        fs::write(path, body.to_string()).expect("response");
    }

    /// Answers the queries in call order. A widget script registers its state
    /// watch first, which takes callback id 1, and the body runs on load and on
    /// the update event, so the same answers are written for both passes.
    fn queue(&self, values: &[Value]) {
        for pass in 0..2 {
            for (index, value) in values.iter().enumerate() {
                let body = json!({ "ok": true, "value": value });
                let id = 2 + pass * values.len() + index;
                let path = self
                    .0
                    .join(".noctalia-rs/query")
                    .join(format!("noctalia_rs_luau_{id}.json"));
                fs::write(path, body.to_string()).expect("response");
            }
        }
    }
}

impl Drop for QueryDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn context(dir: &Path) -> Context {
    Context::new(
        Parts {
            entry: "widget".to_owned(),
            event: "update".to_owned(),
            payload: json!({}),
            config: json!({}),
            state: json!({}),
            host: json!({ "pluginDir": "/plugin" }),
            data_dir: dir.to_path_buf(),
        },
        json!({}),
    )
}

fn run(handler: &str, dir: &Path) -> (String, Vec<Value>) {
    let source = format!(
        "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::json;\nuse std::fs::OpenOptions;\n\n{handler}\n"
    );
    let script = compile_rust(
        &source,
        &Options {
            handler: "handler",
            entry: "widget",
            state_key: "files.state",
            kind: EntryKind::Widget,
            strings: &[],
            settings: &[],
            module: "main",
            modules: &[],
            native: NativeMode::Deny,
            strict: None,
        },
    )
    .expect("compile")
    .source;
    let ops = noctalia_rs_luau::dispatch(
        &context(dir),
        &[("widget", script.as_str())],
    )
    .expect("dispatch");
    let values = ops
        .iter()
        .map(|op| serde_json::to_value(op).expect("serialize"))
        .collect::<Vec<_>>();
    let text = values
        .iter()
        .find_map(|value| {
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
        .unwrap_or_else(|| panic!("missing bar text: {values:?}"));
    (text, values)
}

#[test]
fn path_components_compute_the_rust_answers() {
    let dir = QueryDir::new();
    let (text, _) = run(
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let inside = Path::new("/a/b/c").starts_with("/a/b");
    let sibling = Path::new("/a/bc").starts_with("/a/b");
    let suffix = Path::new("/a/b/").ends_with("b");
    let root_only = Path::new("/a").starts_with("/");
    let name = Path::new("/a/b.txt").file_name().unwrap_or_default();
    let stem = Path::new("/a/b.tar.gz").file_stem().unwrap_or_default();
    let extension = Path::new("/a/b.tar.gz").extension().unwrap_or_default();
    let hidden = Path::new("/a/.bashrc").extension().is_none();
    let parent = Path::new("/a/b").parent().unwrap_or_default();
    let leaf = Path::new("b").parent().unwrap_or_default();
    let swapped = Path::new("/a/b.txt").with_extension("md");
    let joined = Path::new("/a").join("b").join("/abs");
    Ok(vec![Op::BarText { text: format!(
        "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
        inside, sibling, suffix, root_only, name, stem, extension, hidden, parent, leaf, swapped.display(), joined.display()
    ) }])
}
"#,
        dir.path(),
    );
    assert_eq!(
        text, "true|false|true|true|b.txt|b.tar|gz|true|/a||/a/b.md|/abs",
        "{text}"
    );
}

#[test]
fn fs_members_read_list_and_write_in_host_order() {
    let dir = QueryDir::new();
    dir.queue(&[
        json!("hello"),
        json!(["/dir/b.md", "/dir/a.txt"]),
        json!({ "size": 5, "mtime": 7, "isDir": false }),
        json!(true),
    ]);
    let (text, _) = run(
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let text = std::fs::read_to_string("/notes.txt")?;
    let mut names = Vec::new();
    for entry in std::fs::read_dir("/dir")? {
        names.push(entry.file_name().unwrap_or_default());
    }
    let info = std::fs::metadata("/notes.txt")?;
    std::fs::write("/out.txt", text.clone())?;
    Ok(vec![Op::BarText { text: format!(
        "{}|{}|{}|{}|{}",
        text, names.join(","), info.size, info.is_dir, names.len()
    ) }])
}
"#,
        dir.path(),
    );
    assert_eq!(text, "hello|b.md,a.txt|5|false|2", "{text}");
}

#[test]
fn append_writes_append_the_existing_contents() {
    let dir = QueryDir::new();
    dir.queue(&[json!("/plugin"), json!("first\n"), json!(true)]);
    let (text, values) = run(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let path = ctx.plugin_dir().join("log.txt");
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all("second\n")?;
    Ok(vec![Op::BarText { text: format!("{}", path.display()) }])
}
"#,
        dir.path(),
    );
    assert_eq!(text, "/plugin/log.txt", "{text}");
    let written = values
        .iter()
        .find(|value| {
            value["op"] == "query" && value["path"] == json!(["writeFile"])
        })
        .expect("the append writes");
    assert_eq!(
        written["args"],
        json!(["/plugin/log.txt", "first\nsecond\n"])
    );
}

#[test]
fn missing_files_take_the_error_arm() {
    let dir = QueryDir::new();
    dir.fail(2, "no such file");
    dir.fail(3, "no such file");
    let (text, _) = run(
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let missing = std::fs::read_to_string("/notes.txt").unwrap_or_default();
    Ok(vec![Op::BarText { text: format!("[{}]", missing) }])
}
"#,
        dir.path(),
    );
    assert_eq!(text, "[]", "{text}");
}

#[test]
fn enumerate_tuple_loops_bind_the_element() {
    let dir = QueryDir::new();
    let (text, _) = run(
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let rows = vec!["a".to_owned(), "b".to_owned()];
    let mut text = String::new();
    for (index, value) in rows.iter().enumerate() {
        text.push_str(&format!("{}={};", index, value));
    }
    let mut counts = std::collections::HashMap::new();
    counts.insert("x", 1_i64);
    for (key, count) in &counts {
        text.push_str(&format!("{}={};", key, count));
    }
    for value in &rows {
        text.push_str(value);
    }
    Ok(vec![Op::BarText { text }])
}
"#,
        dir.path(),
    );
    assert!(
        !text.contains("nil") && text.contains("a") && text.contains("b"),
        "{text}"
    );
    assert!(text.ends_with("ab"), "{text}");
}

#[test]
fn write_file_result_reports_the_host_error_string() {
    let dir = QueryDir::new();
    dir.respond(2, json!(false), Some("host denied the write"));
    dir.respond(4, json!(false), Some("host denied the write"));
    let (text, values) = run(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let (written, error) = ctx.write_file_result("/a.txt", "x");
    let report = error.unwrap_or_else(|| "fallback".to_owned());
    Ok(vec![Op::BarText { text: format!("{}:{}", written, report) }])
}
"#,
        dir.path(),
    );
    assert_eq!(text, "false:host denied the write", "{text}");
    let query = values
        .iter()
        .find(|value| {
            value["op"] == "query" && value["path"] == json!(["writeFile"])
        })
        .expect("writeFile query");
    assert_eq!(query["args"], json!(["/a.txt", "x"]));
}
