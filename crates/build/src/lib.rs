use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};

use toml::Value;

type BoxError = Box<dyn Error>;
type EntryScripts = Vec<(String, String)>;

const ENTRY_KINDS: &[&str] = &[
    "service",
    "widget",
    "shortcut",
    "launcher_provider",
    "desktop_widget",
    "panel",
    "layer",
];

#[derive(Default)]
struct Generated {
    prefix: String,
    body: String,
}

struct FileContext<'a> {
    generated: &'a mut Generated,
    source: &'a Path,
    out_dir: &'a Path,
    root: &'a toml::Table,
}
fn identifier(value: &str) -> String {
    let mut out = String::new();
    for (index, item) in value.chars().enumerate() {
        if item.is_ascii_alphanumeric() || item == '_' {
            out.push(item);
        } else {
            out.push('_');
        }
        if index == 0 && item.is_ascii_digit() {
            out.insert(0, '_');
        }
    }
    out
}

fn watch_tree(dir: &Path) -> Result<(), BoxError> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            println!("cargo:rerun-if-changed={}", dir.display());
            if matches!(name, "target" | ".git" | ".tmp") {
                continue;
            }
            watch_tree(&path)?;
        } else {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    Ok(())
}
pub fn generate() -> Result<(), BoxError> {
    let manifest = Path::new("noctalia.toml");
    let out = std::env::var("OUT_DIR")?;
    println!("cargo:rerun-if-changed=noctalia.toml");
    watch_tree(Path::new("."))?;
    generate_to(manifest, Path::new(&out))
}

pub fn generate_to(manifest: &Path, out_dir: &Path) -> Result<(), BoxError> {
    let text = fs::read_to_string(manifest)?;
    let value: Value = toml::from_str(&text)?;
    let root = value.as_table().ok_or("manifest is not a table")?;
    let source = manifest
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));

    let mut generated = Generated::default();
    write_meta(&mut generated.body, root)
        .map_err(|error| format!("meta: {error}"))?;
    write_settings(&mut generated.body, root, "plugin")
        .map_err(|error| format!("settings: {error}"))?;
    write_entries(&mut generated.body, root)
        .map_err(|error| format!("entries: {error}"))?;
    {
        let mut context = FileContext {
            generated: &mut generated,
            source,
            out_dir,
            root,
        };
        write_files(&mut context).map_err(|error| format!("files: {error}"))?;
    }
    let _ = write!(generated.body, "    plugin\n}}\n");
    let defaults = default_entry_scripts(root)?;
    if !defaults.is_empty() {
        let _ = writeln!(
            generated.prefix,
            "pub fn dispatch(ctx: &Context) -> Result<Vec<Op>> {{"
        );
        if generated.prefix.contains("pub const LUAU_SOURCES") {
            let _ = writeln!(
                generated.prefix,
                "    let script = match ctx.entry.as_str() {{"
            );
            for (id, script) in &defaults {
                let _ = writeln!(
                    generated.prefix,
                    "        {} => {},",
                    quote(id),
                    quote(script)
                );
            }
            let _ = writeln!(
                generated.prefix,
                "        _ => return Err(format!(\"unknown entry {{}}\", ctx.entry).into()),"
            );
            let _ = writeln!(generated.prefix, "    }};");
            let _ = writeln!(
                generated.prefix,
                "    noctalia_rs_luau::dispatch_entry(ctx, LUAU_SOURCES, script)"
            );
        } else {
            let _ = writeln!(
                generated.prefix,
                "    let _ = ctx;\n    let _ = serde_json::from_str::<serde_json::Value>(\"null\")?;"
            );
        }
        let _ = writeln!(generated.prefix, "}}\n");
    }
    generated.prefix.push_str(&generated.body);
    let out = generated.prefix;
    let out = qualify_types(&out);
    let target = out_dir.join("generated.rs");
    fs::write(target, out)?;
    Ok(())
}

fn default_entry_scripts(root: &toml::Table) -> Result<EntryScripts, BoxError> {
    let mut result = Vec::new();
    for &kind in ENTRY_KINDS {
        let Some(value) = root.get(kind) else {
            continue;
        };
        for item in array(value, kind)? {
            let row = table(item, kind)?;
            if row.contains_key("handler") {
                continue;
            }
            let id = text(row.get("id").ok_or("missing entry id")?, "id")?;
            let script = row
                .get("entry")
                .map(|value| text(value, "entry"))
                .transpose()?
                .unwrap_or(id);
            let script = script.strip_suffix(".luau").unwrap_or(script);
            result.push((id.to_owned(), script.to_owned()));
        }
    }
    Ok(result)
}

fn quote(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}

fn qualify_types(source: &str) -> String {
    let replacements = [
        ("MetaBuilder::", "noctalia_rs::MetaBuilder::"),
        ("Plugin::", "noctalia_rs::Plugin::"),
        ("-> Plugin", "-> noctalia_rs::Plugin"),
        ("Setting::", "noctalia_rs::Setting::"),
        ("Entry::configured", "noctalia_rs::Entry::configured"),
        ("EntryKind::", "noctalia_rs::EntryKind::"),
        ("EntryConfig::", "noctalia_rs::EntryConfig::"),
        ("LauncherCategory::", "noctalia_rs::LauncherCategory::"),
        ("LayerConfig::", "noctalia_rs::LayerConfig::"),
        ("ctx: &Context", "ctx: &noctalia_rs::Context"),
        (
            "Result<Vec<Op>>",
            "noctalia_rs::Result<Vec<noctalia_rs::Op>>",
        ),
    ];
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    loop {
        let Some(index) = rest.find('"') else {
            out.push_str(&qualify_code(rest, &replacements));
            break;
        };
        let (code, tail) = rest.split_at(index);
        out.push_str(&qualify_code(code, &replacements));
        let mut quoted = String::from("\"");
        let mut chars = tail[1..].chars();
        let mut consumed = 1usize;
        let mut escaped = false;
        for character in chars.by_ref() {
            consumed += character.len_utf8();
            quoted.push(character);
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                break;
            }
        }
        out.push_str(&quoted);
        rest = &tail[consumed..];
    }
    out
}

fn qualify_code(code: &str, replacements: &[(&str, &str)]) -> String {
    let mut result = code.to_owned();
    for (from, to) in replacements {
        result = result.replace(from, to);
    }
    result
}

fn map_pairs(map: &toml::Table, label: &str) -> Result<String, BoxError> {
    let mut pairs = Vec::new();
    for (key, value) in map {
        let value = text(value, label)?;
        pairs.push(format!("({}, {})", quote(key), quote(value)));
    }
    Ok(pairs.join(", "))
}

fn integer_map_pairs(
    map: &toml::Table,
    label: &str,
) -> Result<String, BoxError> {
    let mut pairs = Vec::new();
    for (key, value) in map {
        let value = integer(value, label)?;
        pairs.push(format!("({}, {})", quote(key), value));
    }
    Ok(pairs.join(", "))
}

fn valid_translation(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".json")
        && !name.contains('/')
        && !name.contains("..")
}

fn optional_string_list(
    root: &toml::Table,
    key: &str,
) -> Result<Vec<String>, BoxError> {
    match root.get(key) {
        Some(value) => string_list(value, key),
        None => Ok(Vec::new()),
    }
}

fn safe_relative_path(value: &str, label: &str) -> Result<PathBuf, BoxError> {
    let normalized = value.replace('\\', "/");
    let path = Path::new(&normalized);
    let safe = !normalized.is_empty()
        && path
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    if safe {
        Ok(path.to_path_buf())
    } else {
        Err(format!("{label} must be a relative path: {value}").into())
    }
}

fn reserved_asset_path(value: &str) -> bool {
    value == "plugin.toml"
        || value == "lib"
        || value == "translations"
        || value.starts_with("lib/")
        || value.starts_with("translations/")
        || value.to_ascii_lowercase().ends_with(".luau")
}
fn text<'a>(value: &'a Value, label: &str) -> Result<&'a str, BoxError> {
    value
        .as_str()
        .ok_or_else(|| format!("{label} must be a string").into())
}

fn bool_value(value: &Value, label: &str) -> Result<bool, BoxError> {
    value
        .as_bool()
        .ok_or_else(|| format!("{label} must be a boolean").into())
}

fn integer(value: &Value, label: &str) -> Result<i64, BoxError> {
    value
        .as_integer()
        .ok_or_else(|| format!("{label} must be an integer").into())
}

fn number(value: &Value, label: &str) -> Result<f64, BoxError> {
    if let Some(inner) = value.as_integer() {
        return Ok(inner as f64);
    }
    value
        .as_float()
        .ok_or_else(|| format!("{label} must be a number").into())
}

fn table<'a>(
    value: &'a Value,
    label: &str,
) -> Result<&'a toml::Table, BoxError> {
    value
        .as_table()
        .ok_or_else(|| format!("{label} must be a table").into())
}

fn array<'a>(value: &'a Value, label: &str) -> Result<&'a [Value], BoxError> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| format!("{label} must be an array").into())
}

fn string_list(value: &Value, label: &str) -> Result<Vec<String>, BoxError> {
    array(value, label)?
        .iter()
        .enumerate()
        .map(|(index, item)| {
            text(item, &format!("{label}[{index}]")).map(str::to_owned)
        })
        .collect()
}

fn rust_list(values: &[String]) -> String {
    let items = values.iter().map(|value| quote(value)).collect::<Vec<_>>();
    format!("&[{}]", items.join(", "))
}

fn f64_literal(value: f64) -> String {
    let formatted = format!("{value:?}");
    let Some((whole, fraction)) = formatted.split_once('.') else {
        return formatted;
    };
    let negative = whole.starts_with('-');
    let digits = if negative { &whole[1..] } else { whole };
    let mut reversed = String::new();
    for (index, digit) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            reversed.push('_');
        }
        reversed.push(digit);
    }
    let grouped = reversed.chars().rev().collect::<String>();
    if negative {
        format!("-{grouped}.{fraction}")
    } else {
        format!("{grouped}.{fraction}")
    }
}

fn write_meta(out: &mut String, root: &toml::Table) -> Result<(), BoxError> {
    let id = text(root.get("id").ok_or("missing id")?, "id")?;
    let name = text(root.get("name").ok_or("missing name")?, "name")?;
    let _ = writeln!(out, "pub fn plugin() -> Plugin {{");
    let _ = writeln!(
        out,
        "    let meta = MetaBuilder::new().id({}).name({}).version(env!(\"CARGO_PKG_VERSION\"))",
        quote(id),
        quote(name)
    );

    if let Some(value) = root.get("author") {
        let _ =
            writeln!(out, "        .author({})", quote(text(value, "author")?));
    }
    if let Some(value) = root.get("plugin_api") {
        let _ =
            writeln!(out, "        .api({})", integer(value, "plugin_api")?);
    }
    if let Some(value) = root.get("description") {
        let _ = writeln!(
            out,
            "        .description({})",
            quote(text(value, "description")?)
        );
    }
    if let Some(value) = root.get("tags") {
        let tags = string_list(value, "tags")?;
        let _ = writeln!(out, "        .tags({})", rust_list(&tags));
    }
    if let Some(value) = root.get("icon") {
        let _ = writeln!(out, "        .icon({})", quote(text(value, "icon")?));
    }
    if let Some(value) = root.get("license") {
        let _ = writeln!(
            out,
            "        .license({})",
            quote(text(value, "license")?)
        );
    }
    if let Some(value) = root.get("dependencies") {
        let deps = string_list(value, "dependencies")?;
        let _ = writeln!(out, "        .dependencies({})", rust_list(&deps));
    }
    if let Some(value) = root.get("deprecated") {
        let _ = writeln!(
            out,
            "        .deprecated({})",
            bool_value(value, "deprecated")?
        );
    }
    let _ = writeln!(out, "        .build();");
    let _ = writeln!(out, "    let mut plugin = Plugin::new(meta);");
    Ok(())
}

fn write_settings(
    out: &mut String,
    root: &toml::Table,
    label: &str,
) -> Result<(), BoxError> {
    let Some(value) = root.get("setting") else {
        return Ok(());
    };
    let settings = array(value, label)?;
    for (index, value) in settings.iter().enumerate() {
        let item = table(value, &format!("{label}[{index}]"))?;
        let expression =
            setting_expression(item, &format!("{label}[{index}]"))?;
        let _ = writeln!(out, "    plugin = plugin.setting({expression});");
    }
    Ok(())
}

fn setting_expression(
    item: &toml::Table,
    label: &str,
) -> Result<String, BoxError> {
    let key = text(item.get("key").ok_or("missing setting key")?, "key")?;
    let label_key = text(
        item.get("label_key").ok_or("missing label_key")?,
        "label_key",
    )?;
    let ty = text(item.get("type").ok_or("missing type")?, "type")?;
    let default = item.get("default");
    let mut expression = match ty {
        "bool" => format!(
            "Setting::bool({}, {}, {})",
            quote(key),
            quote(label_key),
            default
                .map(|value| bool_value(value, "default"))
                .transpose()?
                .unwrap_or(false)
        ),
        "string" | "folder" | "glyph" | "color" => {
            let value = quote(
                default
                    .map(|value| text(value, "default"))
                    .transpose()?
                    .unwrap_or(""),
            );
            format!(
                "Setting::{ty}({}, {}, {value})",
                quote(key),
                quote(label_key)
            )
        }
        "string_list" => {
            let values = default
                .map(|value| string_list(value, "default"))
                .transpose()?
                .unwrap_or_default();
            format!(
                "Setting::string_list({}, {}, {})",
                quote(key),
                quote(label_key),
                rust_list(&values)
            )
        }
        "string_map" => {
            let pairs = match default {
                Some(value) => {
                    map_pairs(table(value, "default")?, "string_map value")?
                }
                None => String::new(),
            };
            format!(
                "Setting::string_map({}, {}, &[{pairs}])",
                quote(key),
                quote(label_key)
            )
        }
        "int" | "double" => {
            let value = default
                .map(|value| number(value, "default"))
                .transpose()?
                .unwrap_or(0.0);
            let minimum = optional_number(item, "min")?;
            let maximum = optional_number(item, "max")?;
            let config =
                format!("({}, {}, {})", f64_literal(value), minimum, maximum);
            let name = if ty == "int" { "int" } else { "double" };
            format!(
                "Setting::{name}({}, {}, {config})",
                quote(key),
                quote(label_key)
            )
        }
        "select" => {
            let value = quote(
                default
                    .map(|value| text(value, "default"))
                    .transpose()?
                    .unwrap_or(""),
            );
            let options = array(
                item.get("options").ok_or("missing options")?,
                "options",
            )?;
            let pairs = options
                .iter()
                .enumerate()
                .map(|(index, option)| {
                    let row = table(option, &format!("options[{index}]"))?;
                    let option_value = text(
                        row.get("value").ok_or("missing option value")?,
                        "option value",
                    )?;
                    let label = text(
                        row.get("label_key").ok_or("missing label_key")?,
                        "option label_key",
                    )?;
                    Ok(format!("({}, {})", quote(option_value), quote(label)))
                })
                .collect::<Result<Vec<_>, BoxError>>()?
                .join(", ");
            format!(
                "Setting::select({}, {}, ({value}, &[{pairs}]))",
                quote(key),
                quote(label_key)
            )
        }
        "file" => {
            let value = quote(
                default
                    .map(|value| text(value, "default"))
                    .transpose()?
                    .unwrap_or(""),
            );
            let extensions = item
                .get("extensions")
                .map(|value| string_list(value, "extensions"))
                .transpose()?
                .unwrap_or_default();
            format!(
                "Setting::file({}, {}, ({value}, {}))",
                quote(key),
                quote(label_key),
                rust_list(&extensions)
            )
        }
        other => {
            return Err(
                format!("{label}: unsupported setting type {other}").into()
            );
        }
    };

    if let Some(value) = item.get("description_key") {
        let value = quote(text(value, "description_key")?);
        let _ = write!(expression, ".description({value})");
    }
    if let Some(value) = item.get("visible_when") {
        let rule = table(value, "visible_when")?;
        let key = quote(text(rule.get("key").ok_or("missing key")?, "key")?);
        let values =
            string_list(rule.get("values").ok_or("missing values")?, "values")?;
        let _ =
            write!(expression, ".visible_when({key}, {})", rust_list(&values));
    }
    if let Some(value) = item.get("step") {
        let _ = write!(
            expression,
            ".step({})",
            f64_literal(number(value, "step")?)
        );
    }
    if let Some(value) = item.get("advanced") {
        let _ =
            write!(expression, ".advanced({})", bool_value(value, "advanced")?);
    }
    if let Some(value) = item.get("secret") {
        let _ = write!(expression, ".secret({})", bool_value(value, "secret")?);
    }
    Ok(expression)
}

fn optional_number(item: &toml::Table, key: &str) -> Result<String, BoxError> {
    let Some(value) = item.get(key) else {
        return Ok("None".to_owned());
    };
    Ok(format!("Some({})", f64_literal(number(value, key)?)))
}

fn write_entries(out: &mut String, root: &toml::Table) -> Result<(), BoxError> {
    for &kind in ENTRY_KINDS {
        let Some(value) = root.get(kind) else {
            continue;
        };
        for (index, item) in array(value, kind)?.iter().enumerate() {
            let label = format!("{kind}[{index}]");
            let row = table(item, &label)?;
            let expression = entry_expression(row, kind, &label)?;
            let _ = writeln!(out, "    plugin = plugin.entry({expression});");
        }
    }
    Ok(())
}

fn entry_expression(
    row: &toml::Table,
    kind: &str,
    label: &str,
) -> Result<String, BoxError> {
    let id = text(row.get("id").ok_or("missing entry id")?, "id")?;
    let handler = row
        .get("handler")
        .map(|value| text(value, "handler"))
        .transpose()?
        .unwrap_or("dispatch");
    let handler = identifier(handler);
    let script = row
        .get("entry")
        .map(|value| text(value, "entry"))
        .transpose()?
        .map(|value| value.strip_suffix(".luau").unwrap_or(value));
    let entry_kind = match kind {
        "service" => "EntryKind::Service".to_owned(),
        "widget" => "EntryKind::Widget".to_owned(),
        "shortcut" => "EntryKind::Shortcut".to_owned(),
        "desktop_widget" => "EntryKind::DesktopWidget".to_owned(),
        "launcher_provider" => {
            let prefix =
                text(row.get("prefix").ok_or("missing prefix")?, "prefix")?;
            let glyph = row
                .get("glyph")
                .map(|value| text(value, "glyph"))
                .transpose()?
                .unwrap_or("");
            format!(
                "EntryKind::LauncherProvider {{ prefix: {}.to_owned(), glyph: {}.to_owned() }}",
                quote(prefix),
                quote(glyph)
            )
        }
        "panel" => {
            let width = numeric_or_text(row.get("width"), "width", 320)?;
            let height = numeric_or_text(row.get("height"), "height", 240)?;
            format!("EntryKind::Panel {{ width: {width}, height: {height} }}")
        }
        "layer" => {
            let config = layer_config(row)?;
            format!("EntryKind::Layer {{ config: Box::new({config}) }}")
        }
        other => {
            return Err(
                format!("{label}: unsupported entry kind {other}").into()
            );
        }
    };

    let settings = row
        .get("setting")
        .map(|value| {
            array(value, "setting")?
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    setting_expression(
                        table(item, &format!("setting[{index}]"))?,
                        label,
                    )
                })
                .collect::<Result<Vec<_>, BoxError>>()
        })
        .transpose()?
        .unwrap_or_default();
    let settings = format!("vec![{}]", settings.join(", "));
    let config = entry_config(row, script)?;
    Ok(format!(
        "Entry::configured(({}.to_owned(), {entry_kind}, {settings}, {config}), {handler})",
        quote(id)
    ))
}

fn numeric_or_text(
    value: Option<&Value>,
    label: &str,
    fallback: u32,
) -> Result<String, BoxError> {
    let Some(value) = value else {
        return Ok(fallback.to_string());
    };
    if let Some(inner) = value.as_integer() {
        return Ok(inner.to_string());
    }
    Ok(format!(
        "0u32 /* {} */",
        text(value, label)?.replace("*/", "* /")
    ))
}

fn entry_config(
    row: &toml::Table,
    script: Option<&str>,
) -> Result<String, BoxError> {
    let mut chain = String::from("EntryConfig::new()");
    if let Some(name) = script {
        let _ = write!(chain, ".script({})", quote(name));
    }
    if let Some(value) = row.get("actions") {
        if value.as_array().is_some() {
            let actions = string_list(value, "actions")?;
            let _ = write!(chain, ".actions({})", rust_list(&actions));
        } else {
            let map = table(value, "actions")?;
            let pairs = map_pairs(map, "action value")?;
            let _ = write!(chain, ".action_map(&[{pairs}])");
        }
    } else if let Some(value) = row.get("action_map") {
        let map = table(value, "action_map")?;
        let pairs = map_pairs(map, "action_map value")?;
        let _ = write!(chain, ".action_map(&[{pairs}])");
    }
    let bool_fields = [
        "hover_highlight",
        "open_near_click",
        "dismiss_on_outside_click",
        "persistent",
        "include_in_global_search",
    ];
    for field in bool_fields {
        if let Some(value) = row.get(field) {
            let _ = write!(chain, ".{field}({})", bool_value(value, field)?);
        }
    }
    if let Some(value) = row.get("keyboard_focus") {
        if let Some(inner) = value.as_str() {
            let _ = write!(chain, ".keyboard_focus_text({})", quote(inner));
        } else {
            let _ = write!(
                chain,
                ".keyboard_focus({})",
                bool_value(value, "keyboard_focus")?
            );
        }
    }
    let text_fields = ["placement", "position", "category", "layer"];
    for field in text_fields {
        if let Some(inner) = row.get(field).and_then(Value::as_str) {
            let method = if field == "layer" {
                "panel_layer"
            } else {
                field
            };
            let _ = write!(chain, ".{method}({})", quote(inner));
        }
    }
    let number_fields = ["max_height", "debounce_ms"];
    for field in number_fields {
        if let Some(value) = row.get(field) {
            let _ = write!(chain, ".{field}({})", integer(value, field)?);
        }
    }
    for field in ["width", "height"] {
        if let Some(inner) = row.get(field).and_then(Value::as_str) {
            let method = if field == "width" {
                "width_text"
            } else {
                "height_text"
            };
            let _ = write!(chain, ".{method}({})", quote(inner));
        }
    }
    if let Some(items) = row.get("capture_keys").and_then(Value::as_array) {
        let keys = items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                text(item, &format!("capture_keys[{index}]")).map(str::to_owned)
            })
            .collect::<Result<Vec<_>, BoxError>>()?;
        let _ = write!(chain, ".capture_keys_list({})", rust_list(&keys));
    }
    if let Some(items) = row.get("category").and_then(Value::as_array) {
        let categories = items
            .iter()
            .map(|item| {
                let row = table(item, "category item")?;
                let label =
                    text(row.get("label").ok_or("missing label")?, "label")?;
                let glyph = row
                    .get("glyph")
                    .map(|value| text(value, "glyph"))
                    .transpose()?
                    .unwrap_or("");
                Ok(format!(
                    "LauncherCategory::new({}, {})",
                    quote(label),
                    quote(glyph)
                ))
            })
            .collect::<Result<Vec<_>, BoxError>>()?
            .join(", ");
        let _ = write!(chain, ".categories(vec![{categories}])");
    }
    Ok(chain)
}

fn layer_config(row: &toml::Table) -> Result<String, BoxError> {
    let mut chain = String::from("LayerConfig::new()");
    for field in ["layer", "keyboard", "output"] {
        if let Some(value) = row.get(field) {
            let _ = write!(chain, ".{field}({})", quote(text(value, field)?));
        }
    }
    if let Some(value) = row.get("anchors") {
        let anchors = string_list(value, "anchors")?;
        let _ = write!(chain, ".anchors({})", rust_list(&anchors));
    }
    for field in ["visible", "click_through"] {
        if let Some(value) = row.get(field) {
            let _ = write!(chain, ".{field}({})", bool_value(value, field)?);
        }
    }
    if let Some(value) = row.get("exclusive_zone") {
        if let Some(text) = value.as_str() {
            if text != "auto" {
                return Err(
                    "exclusive_zone must be \"auto\" or an integer".into()
                );
            }
            let _ = write!(chain, ".exclusive_zone_auto()");
        } else {
            let _ = write!(
                chain,
                ".exclusive_zone({})",
                integer(value, "exclusive_zone")?
            );
        }
    }
    if let Some(value) = row.get("margins") {
        let map = table(value, "margins")?;
        let pairs = integer_map_pairs(map, "margin")?;
        let _ = write!(chain, ".margins(&[{pairs}])");
    }
    for field in ["width", "height"] {
        if let Some(value) = row.get(field) {
            let _ = write!(chain, ".{field}({})", integer(value, field)?);
        }
    }
    Ok(chain)
}

fn write_files(context: &mut FileContext<'_>) -> Result<(), BoxError> {
    write_translations(context)?;
    write_assets(context)?;
    let sources = declared_luau_sources(context.root)?;
    write_luau_sources(context, &sources)
}

fn write_translations(context: &mut FileContext<'_>) -> Result<(), BoxError> {
    let translations = context.source.join("translations");
    if !translations.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&translations)
        .map_err(|error| format!("read translations: {error}"))?
    {
        let path = entry
            .map_err(|error| format!("read translation entry: {error}"))?
            .path();
        let Some(name) = path.file_name().and_then(|value| value.to_str())
        else {
            continue;
        };
        if !valid_translation(name) {
            continue;
        }
        let language = name.trim_end_matches(".json");
        let target_dir = context.out_dir.join("translations");
        fs::create_dir_all(&target_dir)
            .map_err(|error| format!("create translations dir: {error}"))?;
        fs::copy(&path, target_dir.join(name))
            .map_err(|error| format!("copy translation {name}: {error}"))?;
        let _ = writeln!(
            context.generated.body,
            "    plugin = plugin.translation_file({}, include_str!(concat!(env!(\"OUT_DIR\"), \"/translations/{name}\")));",
            quote(language)
        );
    }
    Ok(())
}

fn declared_luau_sources(root: &toml::Table) -> Result<Vec<String>, BoxError> {
    let mut sources = Vec::new();

    for &kind in ENTRY_KINDS {
        let Some(value) = root.get(kind) else {
            continue;
        };
        for item in array(value, kind)? {
            let row = table(item, kind)?;
            if row.contains_key("handler") {
                continue;
            }
            let id = text(row.get("id").ok_or("missing entry id")?, "id")?;
            let name = row
                .get("entry")
                .map(|value| text(value, "entry"))
                .transpose()?
                .unwrap_or(id);
            let name = if name.to_ascii_lowercase().ends_with(".luau") {
                name.to_owned()
            } else {
                format!("{name}.luau")
            };
            if !sources.contains(&name) {
                sources.push(name);
            }
        }
    }

    for name in optional_string_list(root, "luau_sources")? {
        if !name.to_ascii_lowercase().ends_with(".luau") {
            return Err(format!(
                "luau_sources entry must end in .luau: {name}"
            )
            .into());
        }
        if !sources.contains(&name) {
            sources.push(name);
        }
    }

    Ok(sources)
}

fn write_assets(context: &mut FileContext<'_>) -> Result<(), BoxError> {
    for name in optional_string_list(context.root, "assets")? {
        let relative = safe_relative_path(&name, "asset")?;
        let key = relative.to_string_lossy().replace('\\', "/");
        if reserved_asset_path(&key) {
            return Err(format!("reserved asset path: {name}").into());
        }
        let target = context.out_dir.join("assets").join(Path::new(&key));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(context.source.join(relative), &target)
            .map_err(|error| format!("copy asset {key}: {error}"))?;
        let _ = writeln!(
            context.generated.body,
            "    plugin = plugin.asset({}, include_bytes!(concat!(env!(\"OUT_DIR\"), \"/assets/{key}\")));",
            quote(&key)
        );
    }
    Ok(())
}

fn write_luau_sources(
    context: &mut FileContext<'_>,
    names: &[String],
) -> Result<(), BoxError> {
    if names.is_empty() {
        return Ok(());
    }

    let mut keys = Vec::new();
    for name in names {
        let relative = safe_relative_path(name, "luau source")?;
        let key = relative.to_string_lossy().replace('\\', "/");
        keys.push(key.clone());
        let target = context.out_dir.join("luau").join(Path::new(&key));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(context.source.join(relative), &target)
            .map_err(|error| format!("copy luau {key}: {error}"))?;
    }

    let _ = writeln!(
        context.generated.prefix,
        "pub const LUAU_SOURCES: &[(&str, &str)] = &["
    );
    for key in &keys {
        let _ = writeln!(
            context.generated.prefix,
            "    ({}, include_str!(concat!(env!(\"OUT_DIR\"), \"/luau/{key}\"))),",
            quote(key)
        );
    }
    let _ = writeln!(context.generated.prefix, "];");
    let _ = writeln!(context.generated.prefix);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn generate_to_reads_noctalia_toml() -> Result<(), BoxError> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir = std::env::temp_dir().join(format!("noctalia-build-{nanos}"));
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("noctalia.toml"),
            "id = \"me/test\"\nname = \"Test\"\n\n[[service]]\nid = \"service\"\nentry = \"service.luau\"\nhandler = \"tick\"\n",
        )?;

        generate_to(&dir.join("noctalia.toml"), &dir)?;
        let generated = fs::read_to_string(dir.join("generated.rs"))?;
        fs::remove_dir_all(&dir)?;

        assert!(generated.contains("MetaBuilder::new()"));
        assert!(generated.contains("env!(\"CARGO_PKG_VERSION\")"));
        assert!(generated.contains("Entry::configured"));
        assert!(generated.contains("tick"));
        Ok(())
    }

    #[test]
    fn generate_to_copies_only_declared_files() -> Result<(), BoxError> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir =
            std::env::temp_dir().join(format!("noctalia-build-assets-{nanos}"));
        fs::create_dir_all(dir.join("icons"))?;
        fs::create_dir_all(dir.join("lib"))?;
        fs::create_dir_all(dir.join("src"))?;
        fs::write(
            dir.join("noctalia.toml"),
            "id = \"me/test\"\nname = \"Test\"\nassets = [\"icons/clock.svg\"]\nluau_sources = [\"lib/util.luau\"]\n\n[[service]]\nid = \"service\"\nentry = \"service.luau\"\n",
        )?;
        fs::write(dir.join("icons/clock.svg"), "<svg />")?;
        fs::write(dir.join("lib/util.luau"), "return {}")?;
        fs::write(dir.join("service.luau"), "return {}")?;
        fs::write(dir.join("src/main.rs"), "fn main() {}")?;
        fs::write(dir.join("README.md"), "readme")?;

        generate_to(&dir.join("noctalia.toml"), &dir)?;
        let generated = fs::read_to_string(dir.join("generated.rs"))?;
        let has_asset = dir.join("assets/icons/clock.svg").exists();
        let has_service = dir.join("luau/service.luau").exists();
        let has_util = dir.join("luau/lib/util.luau").exists();
        let leaked_source = dir.join("assets/src/main.rs").exists();
        let leaked_readme = dir.join("assets/README.md").exists();
        fs::remove_dir_all(&dir)?;

        assert!(has_asset);
        assert!(has_service);
        assert!(has_util);
        assert!(!leaked_source);
        assert!(!leaked_readme);
        assert!(generated.contains("plugin.asset(\"icons/clock.svg\""));
        assert!(generated.contains("\"lib/util.luau\""));
        assert!(generated.contains("\"service.luau\""));
        assert!(!generated.contains("src/main.rs"));
        assert!(!generated.contains("README.md"));
        Ok(())
    }

    #[test]
    fn generate_to_rejects_reserved_assets() -> Result<(), BoxError> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        for (index, path) in ["plugin.toml", "lib", "translations"]
            .into_iter()
            .enumerate()
        {
            let dir = std::env::temp_dir()
                .join(format!("noctalia-build-reserved-{nanos}-{index}"));
            fs::create_dir_all(&dir)?;
            fs::write(
                dir.join("noctalia.toml"),
                format!(
                    "id = \"me/test\"\nname = \"Test\"\nassets = [\"{path}\"]\n"
                ),
            )?;

            let result = generate_to(&dir.join("noctalia.toml"), &dir);
            fs::remove_dir_all(&dir)?;

            assert!(result.is_err(), "{path} was accepted");
            assert!(
                result.err().is_some_and(|error| error
                    .to_string()
                    .contains("reserved asset")),
                "{path} was not rejected as reserved"
            );
        }
        Ok(())
    }

    #[test]
    fn generate_to_normalizes_declared_paths() -> Result<(), BoxError> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir =
            std::env::temp_dir().join(format!("noctalia-build-paths-{nanos}"));
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("noctalia.toml"),
            "id = \"me/test\"\nname = \"Test\"\nassets = [\"icons\\\\clock.svg\"]\nluau_sources = [\"lib\\\\util.luau\"]\n",
        )?;
        for (source, contents) in [
            (dir.join("icons/clock.svg"), "<svg />"),
            (dir.join("lib/util.luau"), "return {}"),
        ] {
            if let Some(parent) = source.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(source, contents)?;
        }

        generate_to(&dir.join("noctalia.toml"), &dir)?;
        let generated = fs::read_to_string(dir.join("generated.rs"))?;
        let asset_copied = dir.join("assets/icons/clock.svg").exists();
        let luau_copied = dir.join("luau/lib/util.luau").exists();
        fs::remove_dir_all(&dir)?;

        assert!(asset_copied);
        assert!(luau_copied);
        assert!(generated.contains("\"/assets/icons/clock.svg\""));
        assert!(generated.contains("plugin.asset(\"icons/clock.svg\""));
        assert!(generated.contains("\"/luau/lib/util.luau\""));
        assert!(!generated.contains("/luau/lib\\\\util.luau"));
        Ok(())
    }

    #[test]
    fn generate_to_rejects_traversal_paths() -> Result<(), BoxError> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let cases = [
            ("assets", "..\\escape.svg"),
            ("assets", "icons/../../escape.svg"),
            ("luau_sources", "../escape.luau"),
            ("luau_sources", "..\\escape.luau"),
        ];
        for (index, (key, path)) in cases.into_iter().enumerate() {
            let dir = std::env::temp_dir()
                .join(format!("noctalia-build-traversal-{nanos}-{index}"));
            fs::create_dir_all(&dir)?;
            let toml_path = path.replace('\\', "\\\\");
            fs::write(
                dir.join("noctalia.toml"),
                format!(
                    "id = \"me/test\"\nname = \"Test\"\n{key} = [\"{toml_path}\"]\n"
                ),
            )?;

            let result = generate_to(&dir.join("noctalia.toml"), &dir);
            fs::remove_dir_all(&dir)?;

            assert!(result.is_err(), "{key} {path} was accepted");
            assert!(
                result.err().is_some_and(|error| error
                    .to_string()
                    .contains("relative path")),
                "{key} {path} was not rejected as relative"
            );
        }
        Ok(())
    }
}
