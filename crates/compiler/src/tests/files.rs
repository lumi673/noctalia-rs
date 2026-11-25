use super::*;

fn compiled(handler: &str) -> String {
    compile(&source(handler), &options("handler", EntryKind::Service))
        .expect("compiles")
        .source
}

#[test]
fn fs_calls_go_through_the_host_members() {
    let source = compiled(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let text = std::fs::read_to_string("/a.txt")?;
    std::fs::write("/b.txt", text.trim())?;
    std::fs::create_dir_all("/cache")?;
    std::fs::remove_file("/old.txt")?;
    std::fs::rename("/b.txt", "/c.txt")?;
    let entries = std::fs::read_dir("/dir")?;
    let info = std::fs::metadata("/c.txt")?;
    Ok(vec![Op::log(format!("{}{}", info.size, entries.len()))])
}
"#,
    );
    assert!(source.contains("noctalia.readFile(\"/a.txt\")"), "{source}");
    assert!(
        source.contains("noctalia.writeFile(\"/b.txt\", string.trim(text))"),
        "{source}"
    );
    assert!(source.contains("noctalia.mkdirAll(\"/cache\")"), "{source}");
    assert!(
        source.contains("noctalia.removeFile(\"/old.txt\")"),
        "{source}"
    );
    assert!(
        source.contains("noctalia.renameFile(\"/b.txt\", \"/c.txt\")"),
        "{source}"
    );
    assert!(source.contains("noctalia.listDir(\"/dir\")"), "{source}");
    assert!(source.contains("noctalia.fileInfo(\"/c.txt\")"), "{source}");
    assert!(source.contains(".size"), "{source}");
}

#[test]
fn fs_write_and_remove_fail_the_build_when_the_declared_value_is_used() {
    let error = compile(
        &source(
            r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let written = std::fs::write("/a.txt", "x")?;
    Ok(vec![Op::log(format!("{:?}", written))])
}
"#,
        ),
        &options("handler", EntryKind::Service),
    )
    .expect_err("a unit Ok() has no Luau value");
    assert!(error.message.contains("format"), "{error}");
}

#[test]
fn remove_dir_and_temp_dir_report_the_api_gap() {
    let error = compile(
        &source(
            r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    std::fs::remove_dir("/dir")?;
    Ok(Vec::new())
}
"#,
        ),
        &options("handler", EntryKind::Service),
    )
    .expect_err("remove_dir has no member");
    assert!(error.message.contains("std::fs::remove_dir"), "{error}");
    assert!(error.line > 0 && error.column > 0);

    let error = compile(
        &source(
            r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let dir = std::env::temp_dir();
    Ok(vec![Op::log(dir.display().to_string())])
}
"#,
        ),
        &options("handler", EntryKind::Service),
    )
    .expect_err("temp_dir has no member");
    assert!(error.message.contains("temp_dir"), "{error}");
}

#[test]
fn path_values_join_and_stay_paths() {
    let source = compiled(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let mut path = PathBuf::from(ctx.plugin_dir());
    path.push("state.json");
    let other = ctx.plugin_data_dir().unwrap_or_default().join("x");
    let same = Path::new("/tmp/a").join("b");
    Ok(vec![Op::log(format!("{}{}{}", path.display(), other.to_string_lossy(), same.display()))])
}
"#,
    );
    assert!(source.contains("local __base, __part"), "{source}");
    assert!(source.contains("noctalia.pluginDir()"), "{source}");
}

#[test]
fn path_starts_with_is_component_wise() {
    let source = compiled(
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let path = Path::new("/a/bc");
    let inside = path.starts_with("/a/b");
    Ok(vec![Op::log(format!("{}", inside))])
}
"#,
    );
    assert!(
        source.contains("string.sub(__path, #__prefix + 1, #__prefix + 1)"),
        "{source}"
    );
    assert!(source.contains("gsub(__path, \"//+\", \"/\")"), "{source}");
}

#[test]
fn string_starts_with_keeps_the_byte_prefix() {
    let source = compiled(
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let text = "/a/bc".to_owned();
    let inside = text.starts_with("/a/b");
    Ok(vec![Op::log(format!("{}", inside))])
}
"#,
    );
    assert!(
        source.contains("(string.sub(text, 1, #\"/a/b\") == \"/a/b\")"),
        "{source}"
    );
}

#[test]
fn path_queries_use_the_host_members() {
    let source = compiled(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let path = ctx.plugin_dir().join("x");
    let present = path.exists();
    let directory = path.is_dir();
    let absolute = path.is_absolute();
    let name = path.file_name().unwrap_or_default();
    let stem = path.file_stem().unwrap_or_default();
    let extension = path.extension().unwrap_or_default();
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}", present, directory, absolute, name, stem, extension, ctx.file_exists(&path)))])
}
"#,
    );
    assert!(source.contains("noctalia.fileExists("), "{source}");
    assert!(source.contains(".isDir == true"), "{source}");
    assert!(
        source.contains("(string.sub(path, 1, 1) == \"/\")"),
        "{source}"
    );
    assert_eq!(source.matches("gmatch").count(), 6, "{source}");
}

#[test]
fn parent_and_extension_follow_component_rules() {
    let source = compiled(
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let parent = Path::new("/a/b").parent().unwrap_or_default();
    let swapped = Path::new("/a/b.txt").with_extension("md");
    let none = Path::new("/a/b").extension().is_none();
    Ok(vec![Op::log(format!("{}{}{}", parent, swapped, none))])
}
"#,
    );
    assert!(source.contains("table.concat(__parts, \"/\")"), "{source}");
    assert!(
        source.contains("__stem .. \".\" .. __extension"),
        "{source}"
    );
}

#[test]
fn metadata_reports_size_and_kind_but_not_a_system_time() {
    let emitted = compiled(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let info = ctx.file_info("/a.txt");
    let size = info.size;
    let directory = info.is_dir;
    let modified = std::fs::metadata("/a.txt").map(|value| value.len()).unwrap_or(0);
    Ok(vec![Op::log(format!("{}{}{}", size, directory, modified))])
}
"#,
    );
    assert!(emitted.contains(".size"), "{emitted}");
    assert!(emitted.contains(".isDir"), "{emitted}");

    let error = compile(
        &source(
            r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let info = std::fs::metadata("/a.txt")?;
    let stamp = info.modified();
    Ok(vec![Op::log(format!("{:?}", stamp))])
}
"#,
        ),
        &options("handler", EntryKind::Service),
    )
    .expect_err("modified cannot be translated");
    assert!(error.message.contains("SystemTime"), "{error}");
}

#[test]
fn read_dir_keeps_the_host_order() {
    let source = compiled(
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir("/dir")? {
        names.push(entry.file_name().unwrap_or_default());
    }
    Ok(vec![Op::log(format!("{}", names.len()))])
}
"#,
    );
    assert!(source.contains("noctalia.listDir(\"/dir\")"), "{source}");
    assert!(source.contains("table.insert(names"), "{source}");
    assert!(!source.contains("table.sort"), "{source}");
}

#[test]
fn append_writes_read_and_write_the_same_path() {
    let source = compiled(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let path = ctx.plugin_dir().join("debug.log");
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    file.write_all("one\n")?;
    let _ = writeln!(file, "{}", ctx.now_ms);
    file.flush()?;
    Ok(vec![Op::log("done")])
}
"#,
    );
    assert!(source.contains("local __path, __text"), "{source}");
    assert!(source.contains("noctalia.readFile(__path)"), "{source}");
    assert!(
        source.contains(
            "noctalia.writeFile(__path, (__previous or \"\") .. __text)"
        ),
        "{source}"
    );
    assert!(source.contains("noctalia.nowMs()"), "{source}");
    assert!(
        source.contains(".. \"\\n\"") || source.contains(".. \"\\n\""),
        "{source}"
    );
}

#[test]
fn open_options_without_append_reports_the_gap() {
    let error = compile(
        &source(
            r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let file = OpenOptions::new().write(true).truncate(true).open("/a.txt")?;
    Ok(vec![Op::log(format!("{:?}", file))])
}
"#,
        ),
        &options("handler", EntryKind::Service),
    )
    .expect_err("only append maps");
    assert!(error.message.contains("append(true)"), "{error}");
}

#[test]
fn write_macro_needs_an_append_handle() {
    let error = compile(
        &source(
            r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let mut text = String::new();
    let _ = write!(text, "x");
    Ok(vec![Op::log(text)])
}
"#,
        ),
        &options("handler", EntryKind::Service),
    )
    .expect_err("a String is not a file");
    assert!(error.message.contains("append(true)"), "{error}");
}

#[test]
fn os_str_conversions_stay_paths() {
    let source = compiled(
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let base = Path::new("/a");
    let joined = base.join(OsStr::new("b"));
    let text = joined.as_os_str().to_string_lossy().to_string();
    let url = std::env::var_os("HOME").unwrap_or_default();
    Ok(vec![Op::log(format!("{}{}", text, url.display()))])
}
"#,
    );
    assert!(source.contains("noctalia.getenv(\"HOME\")"), "{source}");
}

#[test]
fn main_separator_is_the_unix_separator() {
    let source = compiled(
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let command = "/bin/true";
    let direct = command.contains(std::path::MAIN_SEPARATOR);
    Ok(vec![Op::log(format!("{}", direct))])
}
"#,
    );
    assert!(
        source.contains("string.find(command, \"/\", 1, true)"),
        "{source}"
    );
}

#[test]
fn split_paths_reports_the_api_gap() {
    let error = compile(
        &source(
            r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let dirs = std::env::split_paths(&"/a:/b").collect::<Vec<_>>();
    Ok(vec![Op::log(format!("{}", dirs.len()))])
}
"#,
        ),
        &options("handler", EntryKind::Service),
    )
    .expect_err("split_paths has no member");
    assert!(error.message.contains("split_paths"), "{error}");
}

#[test]
fn path_order_compares_components() {
    let source = compiled(
        r#"
fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let left = Path::new("/a/b");
    let right = Path::new("/a.c");
    let earlier = left < right;
    Ok(vec![Op::log(format!("{}", earlier))])
}
"#,
    );
    assert!(source.contains("__left_root and -1 or 1"), "{source}");
    assert!(!source.contains("(left < right)"), "{source}");
}

#[test]
fn context_fs_results_map_to_the_same_members() {
    let source = compiled(
        r#"
fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let (written, write_error) = ctx.write_file_result("/a.txt", "x");
    let (renamed, rename_error) = ctx.rename_file_result("/a.txt", "/b.txt");
    let (bytes, bytes_error) = ctx.read_file_bytes_result("/b.txt");
    let (entries, list_error) = ctx.list_dir_result("/dir");
    Ok(vec![Op::log(format!("{}{}{}{}{}{}{}{}", written, write_error.is_none(), renamed, rename_error.is_none(), bytes.is_some(), bytes_error.is_none(), entries.is_some(), list_error.is_none()))])
}
"#,
    );
    assert!(
        source.contains("(noctalia.writeFile(\"/a.txt\", \"x\"))"),
        "{source}"
    );
    assert!(
        source.contains("(noctalia.renameFile(\"/a.txt\", \"/b.txt\"))"),
        "{source}"
    );
    assert!(
        source.contains("(noctalia.readFile(\"/b.txt\"))"),
        "{source}"
    );
    assert!(source.contains("(noctalia.listDir(\"/dir\"))"), "{source}");
}
