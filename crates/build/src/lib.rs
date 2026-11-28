use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};

use noctalia_rs_compiler as compiler;
use toml::Value;

pub mod config;
pub mod strict;

type BoxError = Box<dyn Error>;
type EntryScripts = Vec<(String, String)>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CompileMode {
    Auto,
    Strict,
    Off,
}

fn compile_mode(
    root: &toml::Table,
    env_override: Option<&str>,
) -> Result<CompileMode, BoxError> {
    let (value, label) = match env_override {
        Some(value) => (value, "NOCTALIA_RS_COMPILE"),
        None => match root.get("compile") {
            Some(value) => (text(value, "compile")?, "compile"),
            None => ("auto", "compile"),
        },
    };
    match value {
        "auto" => Ok(CompileMode::Auto),
        "strict" => Ok(CompileMode::Strict),
        "off" => Ok(CompileMode::Off),
        other => Err(format!(
            "{label} must be `auto`, `strict`, or `off`: {other}"
        )
        .into()),
    }
}

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
    mode: CompileMode,
}

struct CompileContext<'a> {
    source: &'a Path,
    out_dir: &'a Path,
    root: &'a toml::Table,
    mode: CompileMode,
    state_key: &'a str,
    strings: &'a [&'a str],
    plugin_settings: &'a [String],
    constants: &'a mut String,
    warnings: &'a mut Vec<String>,
    natives: &'a mut std::collections::BTreeMap<String, compiler::NativeFn>,
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
    println!("cargo:rerun-if-env-changed=NOCTALIA_RS_COMPILE");
    watch_tree(Path::new("."))?;
    generate_to(manifest, Path::new(&out))
}

pub fn generate_to(manifest: &Path, out_dir: &Path) -> Result<(), BoxError> {
    let env_override = std::env::var("NOCTALIA_RS_COMPILE").ok();
    let warnings = generate_with(manifest, out_dir, env_override.as_deref())?;
    for warning in warnings {
        println!("cargo:warning={warning}");
    }
    Ok(())
}

fn generate_with(
    manifest: &Path,
    out_dir: &Path,
    env_override: Option<&str>,
) -> Result<Vec<String>, BoxError> {
    let text = fs::read_to_string(manifest)?;
    let value: Value = toml::from_str(&text)?;
    let root = value.as_table().ok_or("manifest is not a table")?;
    let source = manifest
        .parent()
        .filter(|value| !value.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mode = compile_mode(root, env_override)?;
    config::validate(root, source)
        .map_err(|error| format!("config: {error}"))?;
    let mut warnings = Vec::new();

    let mut generated = Generated::default();
    write_meta(&mut generated.body, root)
        .map_err(|error| format!("meta: {error}"))?;
    write_settings(&mut generated.body, root, "plugin")
        .map_err(|error| format!("settings: {error}"))?;
    let state_key = state_key(root)?;
    let string_keys = string_settings(root);
    let strings = string_keys.iter().map(String::as_str).collect::<Vec<_>>();
    let plugin_settings = setting_keys(root.get("setting"));
    let mut constants = String::new();
    let mut natives = std::collections::BTreeMap::new();
    write_entries(
        &mut generated.body,
        root,
        &mut CompileContext {
            source,
            out_dir,
            root,
            mode,
            state_key: &state_key,
            strings: &strings,
            plugin_settings: &plugin_settings,
            constants: &mut constants,
            warnings: &mut warnings,
            natives: &mut natives,
        },
    )
    .map_err(|error| format!("entries: {error}"))?;
    let native_file = out_dir.join("native.rs");
    if natives.is_empty() {
        if native_file.exists() {
            fs::remove_file(native_file)?;
        }
    } else {
        write_native(&native_file, &natives)?;
        generated.prefix.push_str(
            "include!(concat!(env!(\"OUT_DIR\"), \"/native.rs\"));\n\n",
        );
    }
    generated.prefix.push_str(&constants);
    {
        let mut context = FileContext {
            generated: &mut generated,
            source,
            out_dir,
            root,
            mode,
        };
        write_files(&mut context).map_err(|error| format!("files: {error}"))?;
    }
    let plugin = if natives.is_empty() {
        "    plugin\n}\n"
    } else {
        "    plugin.with_native(__noctalia_native::dispatch)\n}\n"
    };
    let _ = write!(generated.body, "{plugin}");
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
    Ok(warnings)
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

/// Inlines literal `include_str!` files into the source handed to the compiler.
///
/// The compiler only sees source strings, so it cannot resolve those paths itself.
fn resolve_includes(source: &str, file: &Path) -> Result<String, BoxError> {
    const NEEDLE: &str = "include_str!(\"";
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(index) = rest.find(NEEDLE) {
        out.push_str(&rest[..index]);
        let after = &rest[index + NEEDLE.len()..];
        let Some(end) = after.find('"') else {
            let before = &source[..source.len() - rest.len() + index];
            let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
            let column = before
                .rfind('\n')
                .map_or(before.len() + 1, |at| before.len() - at);
            return Err(format!(
                "{}:{line}:{column}: unterminated include_str! path",
                file.display()
            )
            .into());
        };
        let path = &after[..end];
        let target = file.parent().unwrap_or_else(|| Path::new(".")).join(path);
        let text = fs::read_to_string(&target).map_err(|error| {
            format!("{}: include_str!({path}): {error}", file.display())
        })?;
        out.push_str(&format!("{text:?}"));
        let remaining = &after[end + 1..];
        rest = remaining.strip_prefix(')').unwrap_or(remaining);
    }
    out.push_str(rest);
    Ok(out)
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

fn write_entries(
    out: &mut String,
    root: &toml::Table,
    context: &mut CompileContext<'_>,
) -> Result<(), BoxError> {
    for &kind in ENTRY_KINDS {
        let Some(value) = root.get(kind) else {
            continue;
        };
        for (index, item) in array(value, kind)?.iter().enumerate() {
            let label = format!("{kind}[{index}]");
            let row = table(item, &label)?;
            let expression = entry_expression(row, kind, &label, context)?;
            let _ = writeln!(out, "    plugin = plugin.entry({expression});");
        }
    }
    Ok(())
}

fn local_name(name: &str) -> String {
    format!("__p_{name}")
}

fn write_native(
    path: &Path,
    natives: &std::collections::BTreeMap<String, compiler::NativeFn>,
) -> Result<(), BoxError> {
    let needs_pathbuf = natives.values().any(|item| {
        item.params.iter().any(|param| {
            param
                .owned
                .as_deref()
                .is_some_and(|ty| ty.contains("PathBuf"))
        })
    });
    let needs_option_ref = natives.values().any(|item| {
        item.params
            .iter()
            .any(|param| param.pass == compiler::NativePass::AsRef)
    });
    let mut out = String::from(concat!(
        "// Native functions that run in the helper process.\n",
        "//\n",
        "// Every parameter type must implement `serde::de::DeserializeOwned` and\n",
        "// every success type must implement `serde::Serialize`. Error types must\n",
        "// implement `Display`.\n",
        "mod __noctalia_native {\n",
        "    use serde_json::Value;\n",
    ));
    if needs_pathbuf {
        out.push_str("    use std::path::PathBuf;\n");
    }
    if needs_option_ref {
        out.push_str(concat!(
            "\n",
            "    fn option_ref<T>(value: &Option<T>) -> Option<&T> {\n",
            "        value.as_ref()\n",
            "    }\n",
        ));
    }
    out.push_str(concat!(
        "\n",
        "    pub(crate) fn dispatch(\n",
        "        name: &str,\n",
        "        __args: Value,\n",
        "        __noctalia_context: Option<&noctalia_rs::Context>,\n",
        "    ) -> Result<Value, String> {\n",
        "        let _ = __noctalia_context;\n",
        "        match name {\n",
    ));
    for item in natives.values() {
        let mut rust_path = String::from("crate::");
        if item.module != "main" {
            rust_path.push_str(&item.module);
            rust_path.push_str("::");
        }
        if let Some(class) = &item.class {
            rust_path.push_str(class);
            rust_path.push_str("::");
        }
        rust_path.push_str(&item.function);
        let mut args = Vec::new();
        let mut needs_context = false;
        for param in &item.params {
            if param.kind == compiler::NativeParamKind::Context {
                needs_context = true;
                args.push("__handler_context".to_owned());
                continue;
            }
            let local = local_name(&param.name);
            args.push(match param.pass {
                compiler::NativePass::Value => local,
                compiler::NativePass::Ref => format!("&{local}"),
                compiler::NativePass::Slice => format!("&{local}[..]"),
                compiler::NativePass::AsRef => format!("option_ref(&{local})"),
                compiler::NativePass::AsDeref => format!("{local}.as_deref()"),
            });
        }
        let args = args.join(", ");
        let _ = writeln!(out, "            {} => {{", quote(&item.name));
        if needs_context {
            let _ = writeln!(
                out,
                "                let __handler_context = __noctalia_context.ok_or(\"native function `{}` needs the handler context\")?;",
                item.name
            );
        }
        for param in &item.params {
            if param.kind == compiler::NativeParamKind::Context {
                continue;
            }
            let name = &param.name;
            let local = local_name(name);
            let declared = param
                .owned
                .as_deref()
                .map_or_else(String::new, |ty| format!(": {ty} "));
            let _ = writeln!(
                out,
                "                let {local}{declared}= serde_json::from_value(__args.get({}).cloned().unwrap_or(Value::Null)).map_err(|error| format!(\"{}: {{error}}\"))?;",
                quote(name),
                name
            );
        }
        if item.result {
            let _ =
                writeln!(out, "                match {rust_path}({args}) {{");
            let _ = writeln!(
                out,
                "                    Ok(value) => serde_json::to_value(value).map_err(|error| error.to_string()),"
            );
            let _ = writeln!(
                out,
                "                    Err(error) => Err(error.to_string()),"
            );
            let _ = writeln!(out, "                }}");
        } else {
            let _ = writeln!(
                out,
                "                serde_json::to_value({rust_path}({args})).map_err(|error| error.to_string())"
            );
        }
        let _ = writeln!(out, "            }}");
    }
    out.push_str(concat!(
        "            _ => Err(format!(\"unknown native function `{name}`\")),\n",
        "        }\n",
        "    }\n",
        "}\n",
    ));
    fs::write(path, out)?;
    Ok(())
}

fn entry_expression(
    row: &toml::Table,
    kind: &str,
    label: &str,
    context: &mut CompileContext<'_>,
) -> Result<String, BoxError> {
    let id = text(row.get("id").ok_or("missing entry id")?, "id")?;
    let handler = row
        .get("handler")
        .map(|value| text(value, "handler"))
        .transpose()?
        .unwrap_or("dispatch");
    let handler_name = handler;
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
    let mut expression = format!(
        "Entry::configured(({}.to_owned(), {entry_kind}, {settings}, {config}), {handler})",
        quote(id)
    );
    if let Some(constant) =
        compile_entry(row, id, script, kind, handler_name, context)?
    {
        let _ = write!(expression, ".with_luau({constant})");
    }
    Ok(expression)
}

fn compiler_kind(kind: &str) -> Result<compiler::EntryKind, BoxError> {
    Ok(match kind {
        "service" => compiler::EntryKind::Service,
        "widget" => compiler::EntryKind::Widget,
        "shortcut" => compiler::EntryKind::Shortcut,
        "desktop_widget" => compiler::EntryKind::DesktopWidget,
        "launcher_provider" => compiler::EntryKind::Launcher,
        "panel" => compiler::EntryKind::Panel,
        "layer" => compiler::EntryKind::Layer,
        other => return Err(format!("unsupported entry kind {other}").into()),
    })
}

/// Setting keys declared for one entry, plugin level plus entry level.
fn setting_keys(value: Option<&Value>) -> Vec<String> {
    let mut keys = Vec::new();
    collect_setting_keys(value, &mut keys);
    keys
}

fn collect_setting_keys(value: Option<&Value>, keys: &mut Vec<String>) {
    let Some(items) = value.and_then(Value::as_array) else {
        return;
    };
    for item in items {
        let Some(table) = item.as_table() else {
            continue;
        };
        if let Some(key) = table.get("key").and_then(Value::as_str)
            && !keys.iter().any(|existing| existing == key)
        {
            keys.push(key.to_owned());
        }
    }
}

fn string_settings(root: &toml::Table) -> Vec<String> {
    let mut keys = Vec::new();
    collect_string_settings(root.get("setting"), &mut keys);
    for kind in ENTRY_KINDS {
        let Some(entries) = root.get(*kind).and_then(Value::as_array) else {
            continue;
        };
        for entry in entries {
            if let Some(table) = entry.as_table() {
                collect_string_settings(table.get("setting"), &mut keys);
            }
        }
    }
    keys
}

fn collect_string_settings(value: Option<&Value>, keys: &mut Vec<String>) {
    let Some(items) = value.and_then(Value::as_array) else {
        return;
    };
    for item in items {
        let Some(table) = item.as_table() else {
            continue;
        };
        if table.get("type").and_then(Value::as_str) != Some("string") {
            continue;
        }
        if let Some(key) = table.get("key").and_then(Value::as_str) {
            keys.push(key.to_owned());
        }
    }
}

fn state_key(root: &toml::Table) -> Result<String, BoxError> {
    let id = text(root.get("id").ok_or("missing id")?, "id")?;
    let last = id.split('/').next_back().unwrap_or(id);
    Ok(format!("{last}.state"))
}

pub(crate) struct ModuleFile {
    module: String,
    file: PathBuf,
    source: String,
}

/// Module path from a file path, used when the file is not reachable from
/// `src/main.rs` (a standalone `luau` file, for example).
fn module_path(relative: &Path) -> Option<String> {
    let relative = relative.strip_prefix("src").ok()?;
    let mut parts = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let file = parts.pop()?;
    let stem = file.strip_suffix(".rs")?;
    if stem == "main" || stem == "lib" {
        if parts.is_empty() {
            return Some("main".to_owned());
        }
        return Some(parts.join("::"));
    }
    if stem != "mod" {
        parts.push(stem.to_owned());
    }
    Some(parts.join("::"))
}

fn walk_sources(
    root: &Path,
    dir: &Path,
    out: &mut Vec<ModuleFile>,
) -> Result<(), BoxError> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    let mut entries = entries.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::path);
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            walk_sources(root, &path, out)?;
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("rs") {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let Some(module) = module_path(relative) else {
            continue;
        };
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        out.push(ModuleFile {
            module,
            file: path,
            source,
        });
    }
    Ok(())
}

/// Reads the crate's module tree from `src/main.rs` or `src/lib.rs`, following
/// `mod` declarations and `#[path]` attributes.
fn crate_modules(root: &Path) -> Result<Vec<ModuleFile>, BoxError> {
    let main = root.join("src/main.rs");
    let lib = root.join("src/lib.rs");
    let mut modules = Vec::new();
    if main.is_file() {
        module_files(&main, "main", true, false, &mut modules)?;
    } else if lib.is_file() {
        module_files(&lib, "main", true, false, &mut modules)?;
    }
    Ok(modules)
}

fn module_files(
    file: &Path,
    module: &str,
    is_root: bool,
    explicit_path: bool,
    out: &mut Vec<ModuleFile>,
) -> Result<(), BoxError> {
    let source = fs::read_to_string(file)
        .map_err(|error| format!("read {}: {error}", file.display()))?;
    let parsed = syn::parse_file(&source)
        .map_err(|error| format!("parse {}: {error}", file.display()))?;
    out.push(ModuleFile {
        module: module.to_owned(),
        file: file.to_path_buf(),
        source,
    });
    let parent = file.parent().unwrap_or_else(|| Path::new("."));
    let stem = file.file_stem().and_then(|value| value.to_str());
    let directory = match stem {
        Some("main" | "lib" | "mod") | None => parent.to_path_buf(),
        Some(_) if explicit_path => parent.to_path_buf(),
        Some(name) => parent.join(name),
    };
    module_tree(&directory, parent, module, is_root, &parsed.items, out)
}

fn module_tree(
    directory: &Path,
    declaring: &Path,
    module: &str,
    is_root: bool,
    items: &[syn::Item],
    out: &mut Vec<ModuleFile>,
) -> Result<(), BoxError> {
    for item in items {
        let syn::Item::Mod(item_mod) = item else {
            continue;
        };
        let name = item_mod.ident.to_string();
        let child = if is_root {
            name.clone()
        } else {
            format!("{module}::{name}")
        };
        if let Some((_, inner)) = &item_mod.content {
            let nested = directory.join(&name);
            module_tree(&nested, &nested, &child, false, inner, out)?;
            continue;
        }
        let attribute = item_mod.attrs.iter().find_map(|attr| {
            if !attr.path().is_ident("path") {
                return None;
            }
            let syn::Meta::NameValue(value) = &attr.meta else {
                return None;
            };
            let syn::Expr::Lit(literal) = &value.value else {
                return None;
            };
            let syn::Lit::Str(path) = &literal.lit else {
                return None;
            };
            Some(path.value())
        });
        let explicit = attribute.is_some();
        let candidate = match attribute {
            Some(path) => declaring.join(path),
            None => {
                let file = directory.join(format!("{name}.rs"));
                let mod_file = directory.join(&name).join("mod.rs");
                if file.is_file() {
                    file
                } else if mod_file.is_file() {
                    mod_file
                } else {
                    continue;
                }
            }
        };
        module_files(&candidate, &child, false, explicit, out)?;
    }
    Ok(())
}

struct HandlerSite {
    file: PathBuf,
    source: String,
    module: String,
    modules: Vec<ModuleFile>,
}

impl HandlerSite {
    fn new(
        file: PathBuf,
        source: String,
        module: String,
        modules: Vec<ModuleFile>,
    ) -> Self {
        let modules = modules
            .into_iter()
            .filter(|item| item.module != module)
            .collect();
        Self {
            file,
            source,
            module,
            modules,
        }
    }
}

fn source_relative(file: &Path, root: &Path) -> PathBuf {
    file.strip_prefix(root).unwrap_or(file).to_path_buf()
}

pub(crate) fn walk_all(root: &Path) -> Result<Vec<ModuleFile>, BoxError> {
    let mut files = Vec::new();
    walk_sources(root, &root.join("src"), &mut files)?;
    Ok(files)
}

fn find_handler(
    items: &[syn::Item],
    module: &str,
    is_root: bool,
    handler: &str,
) -> Option<String> {
    for item in items {
        if let syn::Item::Fn(function) = item
            && function.sig.ident == handler
        {
            return Some(module.to_owned());
        }
        if let syn::Item::Mod(item_mod) = item
            && let Some((_, inner)) = &item_mod.content
        {
            let name = item_mod.ident.to_string();
            let child = if is_root {
                name
            } else {
                format!("{module}::{name}")
            };
            if let Some(found) = find_handler(inner, &child, false, handler) {
                return Some(found);
            }
        }
    }
    None
}

fn handler_module(
    file: &ModuleFile,
    handler: &str,
) -> Result<Option<String>, BoxError> {
    let parsed = syn::parse_file(&file.source)
        .map_err(|error| format!("parse {}: {error}", file.file.display()))?;
    Ok(find_handler(&parsed.items, &file.module, true, handler))
}

fn handler_site(
    context: &CompileContext<'_>,
    row: &toml::Table,
    handler: &str,
) -> Result<Option<HandlerSite>, BoxError> {
    let mut files = crate_modules(context.source)?;
    if let Some(value) = row.get("luau") {
        let relative = safe_relative_path(text(value, "luau")?, "luau")?;
        let target = context.source.join(&relative);
        let source = fs::read_to_string(&target)
            .map_err(|error| format!("read {}: {error}", relative.display()))?;
        let module = if let Some(found) =
            files.iter().find(|file| file.file == target)
        {
            found.module.clone()
        } else {
            files = walk_all(context.source)?;
            module_path(&relative).ok_or_else(|| {
                format!("luau file must live in src/: {}", relative.display())
            })?
        };
        return Ok(Some(HandlerSite::new(relative, source, module, files)));
    }

    let mut found = None;
    for (index, file) in files.iter().enumerate() {
        if let Some(module) = handler_module(file, handler)? {
            found = Some((index, module));
            break;
        }
    }
    let Some((index, module)) = found else {
        return Ok(None);
    };
    let file = files.remove(index);
    let relative = source_relative(&file.file, context.source);
    Ok(Some(HandlerSite::new(relative, file.source, module, files)))
}

fn compile_failure(
    mode: CompileMode,
    warnings: &mut Vec<String>,
    id: &str,
    rendered: String,
    fatal: bool,
) -> Result<Option<String>, BoxError> {
    let message = format!("entry {id}: {rendered}");
    if fatal || mode == CompileMode::Strict {
        return Err(message.into());
    }
    warnings.push(message);
    Ok(None)
}

/// Compiles the entry handler, writing the script next to `generated.rs` and
/// returning the constant that holds it.
fn compile_entry(
    row: &toml::Table,
    id: &str,
    script: Option<&str>,
    kind: &str,
    handler: &str,
    context: &mut CompileContext<'_>,
) -> Result<Option<String>, BoxError> {
    if context.mode == CompileMode::Strict && !row.contains_key("handler") {
        return Err(format!(
            "entry {id}: noctalia.toml:1:1: no handler to compile in strict mode"
        )
        .into());
    }
    if context.mode == CompileMode::Off || !row.contains_key("handler") {
        return Ok(None);
    }
    let Some(site) = handler_site(context, row, handler)? else {
        let file = if context.source.join("src/main.rs").is_file() {
            "src/main.rs"
        } else if context.source.join("src/lib.rs").is_file() {
            "src/lib.rs"
        } else {
            "noctalia.toml"
        };
        let rendered = format!(
            "{file}:1:1: handler `{handler}` not found in crate source"
        );
        return compile_failure(
            context.mode,
            context.warnings,
            id,
            rendered,
            false,
        );
    };
    let module_sources = site
        .modules
        .iter()
        .map(|file| {
            resolve_includes(&file.source, &context.source.join(&file.file))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let owned = site
        .modules
        .iter()
        .zip(&module_sources)
        .map(|(file, source)| compiler::Module {
            path: &file.module,
            source,
        })
        .collect::<Vec<_>>();
    let source =
        resolve_includes(&site.source, &context.source.join(&site.file))?;
    let display = site.file.to_string_lossy();
    let mut entry_settings_owned = context.plugin_settings.to_vec();
    collect_setting_keys(row.get("setting"), &mut entry_settings_owned);
    let entry_settings = entry_settings_owned
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let strict_context = if context.mode == CompileMode::Strict {
        Some(strict::StrictContext::build(
            context.root,
            context.source,
            &entry_settings_owned,
        )?)
    } else {
        None
    };
    let strict_config = strict_context
        .as_ref()
        .map(strict::StrictContext::config_source)
        .unwrap_or_default();
    let strict_translations = strict_context
        .as_ref()
        .map(strict::StrictContext::translations_source)
        .unwrap_or_default();
    let strict_assets = strict_context
        .as_ref()
        .map(|data| {
            data.assets
                .iter()
                .map(|asset| {
                    (asset.path.as_str(), asset.mode.unwrap_or(0o644) & 0o777)
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let strict_options = (context.mode == CompileMode::Strict).then_some(
        compiler::StrictOptions {
            config: &strict_config,
            translations: &strict_translations,
            assets: &strict_assets,
        },
    );
    let result = compiler::compile(
        &source,
        &compiler::Options {
            handler,
            entry: id,
            state_key: context.state_key,
            kind: compiler_kind(kind)?,
            strings: context.strings,
            settings: &entry_settings,
            module: &site.module,
            modules: &owned,
            native: match context.mode {
                CompileMode::Strict => compiler::NativeMode::Deny,
                _ => compiler::NativeMode::Allow,
            },
            strict: strict_options,
        },
    );
    let compiled = match result {
        Ok(compiled) => compiled,
        Err(error) => {
            let rendered = error.render(&display);
            let fatal = error.fatal;
            return compile_failure(
                context.mode,
                context.warnings,
                id,
                rendered,
                fatal,
            );
        }
    };

    let key = script.unwrap_or(id);
    let name = format!("{key}.luau");
    let target = context.out_dir.join("luau").join(&name);
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(target, compiled.source)?;

    for item in compiled.native {
        let line = format!(
            "entry {id}: native `{}` (cache {} ms): {}",
            item.name, item.cache_ms, item.reason
        );
        if let std::collections::btree_map::Entry::Vacant(slot) =
            context.natives.entry(item.name.clone())
        {
            slot.insert(item);
        }
        context.warnings.push(line);
    }

    let constant = format!("LUAU_{}", identifier(key).to_uppercase());
    let _ = writeln!(
        context.constants,
        "pub const {constant}: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/luau/{name}\"));"
    );
    Ok(Some(constant))
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
    let modes = if context.mode == CompileMode::Strict {
        strict::asset_modes(context.root, context.source)?
    } else {
        Vec::new()
    };
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
        fs::copy(context.source.join(&relative), &target)
            .map_err(|error| format!("copy asset {key}: {error}"))?;
        let mode = modes
            .iter()
            .find(|item| item.path == key)
            .and_then(|item| strict::mode_differs(item.mode));
        match mode {
            Some(mode) => {
                let _ = writeln!(
                    context.generated.body,
                    "    plugin = plugin.asset_with_mode({}, include_bytes!(concat!(env!(\"OUT_DIR\"), \"/assets/{key}\")), {mode});",
                    quote(&key)
                );
            }
            None => {
                let _ = writeln!(
                    context.generated.body,
                    "    plugin = plugin.asset({}, include_bytes!(concat!(env!(\"OUT_DIR\"), \"/assets/{key}\")));",
                    quote(&key)
                );
            }
        }
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

    #[cfg(unix)]
    #[test]
    fn generate_to_keeps_an_executable_asset_mode() -> Result<(), BoxError> {
        use std::os::unix::fs::PermissionsExt;

        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir =
            std::env::temp_dir().join(format!("noctalia-build-mode-{nanos}"));
        fs::create_dir_all(dir.join("tools"))?;
        fs::write(
            dir.join("noctalia.toml"),
            "id = \"me/test\"\nname = \"Test\"\nassets = [\"tools/run.sh\"]\n",
        )?;
        fs::write(dir.join("tools/run.sh"), "#!/bin/sh\n")?;
        fs::set_permissions(
            dir.join("tools/run.sh"),
            fs::Permissions::from_mode(0o755),
        )?;

        generate_with(&dir.join("noctalia.toml"), &dir, Some("strict"))?;
        let generated = fs::read_to_string(dir.join("generated.rs"))?;
        fs::remove_dir_all(&dir)?;

        assert!(
            generated.contains(
                "plugin.asset_with_mode(\"tools/run.sh\", include_bytes!"
            ),
            "{generated}"
        );
        assert!(generated.contains(", 493);"), "{generated}");
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

    const COMPILED_HANDLER: &str = "use noctalia_rs::{Context, Op, Result};\n\nfn handler(ctx: &Context) -> Result<Vec<Op>> {\n    if ctx.event == \"render\" {\n        Ok(vec![Op::BarText { text: \"hi\".to_owned() }])\n    } else {\n        Ok(Vec::new())\n    }\n}\n";

    #[test]
    fn generate_to_compiles_luau_entries() -> Result<(), BoxError> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir =
            std::env::temp_dir().join(format!("noctalia-build-luau-{nanos}"));
        fs::create_dir_all(dir.join("src"))?;
        fs::write(
            dir.join("noctalia.toml"),
            "id = \"me/test\"\nname = \"Test\"\n\n[[widget]]\nid = \"widget\"\nentry = \"widget.luau\"\nhandler = \"handler\"\nluau = \"src/handler.rs\"\n",
        )?;
        fs::write(dir.join("src/handler.rs"), COMPILED_HANDLER)?;

        generate_with(&dir.join("noctalia.toml"), &dir, None)?;
        let generated = fs::read_to_string(dir.join("generated.rs"))?;
        let script = fs::read_to_string(dir.join("luau/widget.luau"))?;
        fs::remove_dir_all(&dir)?;

        assert!(generated.contains(
            "pub const LUAU_WIDGET: &str = include_str!(concat!(env!(\"OUT_DIR\"), \"/luau/widget.luau\"));"
        ));
        assert!(generated.contains(".with_luau(LUAU_WIDGET)"));
        assert!(!generated.contains("src/handler.rs"));
        assert!(script.contains("barWidget.setText"));
        Ok(())
    }

    #[test]
    fn generate_to_renders_luau_compile_errors() -> Result<(), BoxError> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir = std::env::temp_dir()
            .join(format!("noctalia-build-luau-error-{nanos}"));
        fs::create_dir_all(dir.join("src"))?;
        fs::write(
            dir.join("noctalia.toml"),
            "id = \"me/test\"\nname = \"Test\"\ncompile = \"strict\"\n\n[[widget]]\nid = \"widget\"\nhandler = \"handler\"\nluau = \"src/handler.rs\"\n",
        )?;
        fs::write(
            dir.join("src/handler.rs"),
            "use noctalia_rs::{Context, Op, Result};\n\nfn handler(ctx: &Context) -> Result<Vec<Op>> {\n    Ok(vec![Op::Nope])\n}\n",
        )?;

        let result = generate_with(&dir.join("noctalia.toml"), &dir, None);
        fs::remove_dir_all(&dir)?;

        let message = result.expect_err("compile error").to_string();
        assert!(message.contains("src/handler.rs:4:"), "{message}");
        assert!(message.contains("op `Nope` is not supported"), "{message}");
        Ok(())
    }

    #[test]
    fn generate_to_rejects_traversal_luau_paths() -> Result<(), BoxError> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir = std::env::temp_dir()
            .join(format!("noctalia-build-luau-path-{nanos}"));
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("noctalia.toml"),
            "id = \"me/test\"\nname = \"Test\"\n\n[[widget]]\nid = \"widget\"\nhandler = \"handler\"\nluau = \"../escape.rs\"\n",
        )?;

        let result = generate_to(&dir.join("noctalia.toml"), &dir);
        fs::remove_dir_all(&dir)?;

        assert!(
            result.err().is_some_and(|error| error
                .to_string()
                .contains("relative path")),
            "luau path was not rejected"
        );
        Ok(())
    }

    const UNTRANSLATABLE_HANDLER: &str = "use noctalia_rs::{Context, Op, Result};\n\nfn handler(ctx: &Context) -> Result<Vec<Op>> {\n    Ok(vec![Op::Nope])\n}\n";

    const DISCOVERED_MANIFEST: &str = "id = \"me/test\"\nname = \"Test\"\n\n[[widget]]\nid = \"widget\"\nentry = \"widget.luau\"\nhandler = \"handler\"\n";

    fn compile_fixture(
        label: &str,
        manifest: &str,
        source: &str,
    ) -> Result<(PathBuf, PathBuf), BoxError> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir = std::env::temp_dir()
            .join(format!("noctalia-build-{label}-{nanos}"));
        fs::create_dir_all(dir.join("src"))?;
        fs::write(dir.join("noctalia.toml"), manifest)?;
        fs::write(dir.join("src/main.rs"), source)?;
        Ok((dir.join("noctalia.toml"), dir))
    }

    #[test]
    fn generate_with_auto_compiles_discovered_handler() -> Result<(), BoxError>
    {
        let (manifest, dir) =
            compile_fixture("auto", DISCOVERED_MANIFEST, COMPILED_HANDLER)?;
        let warnings = generate_with(&manifest, &dir, None)?;
        let generated = fs::read_to_string(dir.join("generated.rs"))?;
        let script = fs::read_to_string(dir.join("luau/widget.luau"))?;
        fs::remove_dir_all(&dir)?;

        assert!(generated.contains(".with_luau(LUAU_WIDGET)"));
        assert!(script.contains("barWidget.setText"));
        assert!(warnings.is_empty());
        assert!(!generated.contains("compile ="));
        assert!(!generated.contains("luau ="));
        Ok(())
    }

    #[test]
    fn generate_with_auto_falls_back_to_bridge() -> Result<(), BoxError> {
        let (manifest, dir) = compile_fixture(
            "auto-fallback",
            DISCOVERED_MANIFEST,
            UNTRANSLATABLE_HANDLER,
        )?;
        let warnings = generate_with(&manifest, &dir, None)?;
        let generated = fs::read_to_string(dir.join("generated.rs"))?;
        fs::remove_dir_all(&dir)?;

        assert!(generated.contains("Entry::configured"));
        assert!(!generated.contains(".with_luau("));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("widget"), "{}", warnings[0]);
        assert!(warnings[0].contains("src/main.rs:4:"), "{}", warnings[0]);
        assert!(
            warnings[0].contains("op `Nope` is not supported"),
            "{}",
            warnings[0]
        );
        Ok(())
    }

    #[test]
    fn generate_with_strict_rejects_untranslatable_handler()
    -> Result<(), BoxError> {
        let manifest_text =
            format!("compile = \"strict\"\n{DISCOVERED_MANIFEST}");
        let (manifest, dir) =
            compile_fixture("strict", &manifest_text, UNTRANSLATABLE_HANDLER)?;
        let result = generate_with(&manifest, &dir, None);
        let generated = dir.join("generated.rs").exists();
        fs::remove_dir_all(&dir)?;

        let message = result.expect_err("compile error").to_string();
        assert!(message.contains("widget"), "{message}");
        assert!(message.contains("src/main.rs:4:"), "{message}");
        assert!(message.contains("op `Nope` is not supported"), "{message}");
        assert!(!generated);
        Ok(())
    }

    #[test]
    fn generate_with_strict_rejects_entries_without_handler()
    -> Result<(), BoxError> {
        let manifest = "id = \"me/test\"\nname = \"Test\"\ncompile = \"strict\"\n\n[[service]]\nid = \"service\"\nentry = \"service.luau\"\n";
        let (manifest, dir) =
            compile_fixture("strict-no-handler", manifest, COMPILED_HANDLER)?;
        let result = generate_with(&manifest, &dir, None);
        fs::remove_dir_all(&dir)?;

        let message = result.expect_err("strict error").to_string();
        assert!(message.contains("service"), "{message}");
        assert!(
            message.contains("noctalia.toml:1:1: no handler to compile"),
            "{message}"
        );
        Ok(())
    }

    #[test]
    fn generate_with_strict_uses_the_config_table_for_derived_fields()
    -> Result<(), BoxError> {
        let manifest = "id = \"me/test\"\nname = \"Test\"\n\n[[setting]]\nkey = \"message\"\ntype = \"string\"\nlabel_key = \"settings.message.label\"\ndefault = \"Hello\"\n\n[[widget]]\nid = \"widget\"\nentry = \"widget.luau\"\nhandler = \"handler\"\n";
        let source = r#"
use noctalia_rs::{Config, Context, Op, Result};

#[derive(Config)]
struct Settings {
    #[config(key = "message", default = "Hello")]
    message: String,
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    let settings = Settings::from_ctx(ctx);
    Ok(vec![Op::log(settings.message)])
}
"#;
        let (manifest, dir) =
            compile_fixture("strict-derived-config", manifest, source)?;
        let _warnings = generate_with(&manifest, &dir, Some("strict"))?;
        let script = fs::read_to_string(dir.join("luau/widget.luau"))?;
        fs::remove_dir_all(&dir)?;

        assert!(script.contains("local CONFIG = {"), "{script}");
        assert!(script.contains("CONFIG[\"message\"]"), "{script}");
        assert!(script.contains("message = \"Hello\""), "{script}");
        Ok(())
    }

    #[test]
    fn generate_with_off_skips_compiler() -> Result<(), BoxError> {
        let manifest_text = format!("compile = \"off\"\n{DISCOVERED_MANIFEST}");
        let (manifest, dir) =
            compile_fixture("off", &manifest_text, COMPILED_HANDLER)?;
        let warnings = generate_with(&manifest, &dir, None)?;
        let generated = fs::read_to_string(dir.join("generated.rs"))?;
        let script = dir.join("luau/widget.luau").exists();
        fs::remove_dir_all(&dir)?;

        assert!(!generated.contains(".with_luau("));
        assert!(!script);
        assert!(warnings.is_empty());
        Ok(())
    }

    #[test]
    fn generate_with_env_override_wins_over_manifest() -> Result<(), BoxError> {
        let manifest_text =
            format!("compile = \"strict\"\n{DISCOVERED_MANIFEST}");
        let (manifest, dir) =
            compile_fixture("env-off", &manifest_text, UNTRANSLATABLE_HANDLER)?;
        let warnings = generate_with(&manifest, &dir, Some("off"))?;
        let generated = fs::read_to_string(dir.join("generated.rs"))?;
        fs::remove_dir_all(&dir)?;

        assert!(!generated.contains(".with_luau("));
        assert!(warnings.is_empty());
        Ok(())
    }

    #[test]
    fn crate_modules_resolves_children_of_a_path_loaded_module()
    -> Result<(), BoxError> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir =
            std::env::temp_dir().join(format!("noctalia-path-child-{nanos}"));
        fs::create_dir_all(dir.join("src/custom"))?;
        fs::write(
            dir.join("src/main.rs"),
            "#[path = \"custom/thing.rs\"]\nmod handlers;\nfn main() {}\n",
        )?;
        fs::write(dir.join("src/custom/thing.rs"), "mod child;\n")?;
        fs::write(dir.join("src/custom/child.rs"), "pub const X: u32 = 1;\n")?;

        let modules = crate_modules(&dir)?;
        fs::remove_dir_all(&dir)?;

        assert!(modules.iter().any(|item| item.module == "handlers"));
        assert!(modules.iter().any(|item| item.module == "handlers::child"
            && item.file.ends_with("child.rs")));
        Ok(())
    }

    #[test]
    fn crate_modules_resolves_path_relative_to_the_declaring_file()
    -> Result<(), BoxError> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir =
            std::env::temp_dir().join(format!("noctalia-module-path-{nanos}"));
        fs::create_dir_all(dir.join("src"))?;
        fs::write(dir.join("src/main.rs"), "mod panel;\nfn main() {}\n")?;
        fs::write(
            dir.join("src/panel.rs"),
            "#[path = \"panel_config.rs\"]\nmod panel_config;\n",
        )?;
        fs::write(
            dir.join("src/panel_config.rs"),
            "pub const GAP: u32 = 4;\n",
        )?;

        let modules = crate_modules(&dir)?;
        fs::remove_dir_all(&dir)?;

        assert!(modules.iter().any(|item| item.module == "panel"));
        assert!(
            modules
                .iter()
                .any(|item| item.module == "panel::panel_config"
                    && item.file.ends_with("panel_config.rs"))
        );
        Ok(())
    }

    #[test]
    fn generate_with_rejects_unknown_compile_modes() -> Result<(), BoxError> {
        let manifest_text =
            format!("compile = \"fast\"\n{DISCOVERED_MANIFEST}");
        let (manifest, dir) =
            compile_fixture("bad-mode", &manifest_text, COMPILED_HANDLER)?;
        let manifest_error = generate_with(&manifest, &dir, None)
            .expect_err("invalid manifest mode")
            .to_string();
        let env_error = generate_with(&manifest, &dir, Some("fast"))
            .expect_err("invalid env mode")
            .to_string();
        fs::remove_dir_all(&dir)?;

        assert!(
            manifest_error.contains("compile must be"),
            "{manifest_error}"
        );
        assert!(
            env_error.contains("NOCTALIA_RS_COMPILE must be"),
            "{env_error}"
        );
        Ok(())
    }

    const NATIVE_RESULT_SOURCE: &str = r#"use noctalia_rs::{Context, Op, Result};

fn fetch_prs(limit: usize) -> Result<Vec<String>> {
    let text = std::fs::read_link("/tmp/x")?.to_string_lossy().into_owned();
    Ok(vec![format!("{limit} {text}")])
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let rows = fetch_prs(5)?;
    Ok(vec![Op::log(rows.join(","))])
}
"#;

    const LUAU_NATIVE_SOURCE: &str = "use noctalia_rs::{Context, Op, Result};\n\n#[luau]\nfn fetch_prs() -> Result<Vec<String>> {\n    let text = std::fs::read_link(\"/tmp/x\")?.to_string_lossy().into_owned();\n    Ok(vec![text])\n}\n\nfn handler(_ctx: &Context) -> Result<Vec<Op>> {\n    let rows = fetch_prs()?;\n    Ok(vec![Op::log(rows.join(\",\"))])\n}\n";

    const NATIVE_CONTEXT_MANIFEST: &str = "id = \"me/test\"\nname = \"Test\"\n\n[[setting]]\nkey = \"message\"\ntype = \"string\"\nlabel_key = \"settings.message.label\"\ndefault = \"hi\"\n\n[[widget]]\nid = \"widget\"\nentry = \"widget.luau\"\nhandler = \"handler\"\n";

    const NATIVE_CONTEXT_SOURCE: &str = r#"use noctalia_rs::{Context, Op, Result};

fn describe(ctx: &Context) -> Result<String> {
    let text = std::fs::read_link("/tmp/x")?.to_string_lossy().into_owned();
    Ok(format!("{}:{text}", ctx.event))
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(describe(ctx)?)])
}
"#;

    const PLAIN_NATIVE_SOURCE: &str = r#"use noctalia_rs::{Context, Op, Result};

#[native]
fn now() -> i64 {
    1
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let value = now();
    Ok(vec![Op::log(format!("{value}"))])
}
"#;

    #[test]
    fn generate_with_auto_emits_native_dispatch() -> Result<(), BoxError> {
        let (manifest, dir) = compile_fixture(
            "native-auto",
            DISCOVERED_MANIFEST,
            NATIVE_RESULT_SOURCE,
        )?;
        let warnings = generate_with(&manifest, &dir, None)?;
        let script = fs::read_to_string(dir.join("luau/widget.luau"))?;
        let native = fs::read_to_string(dir.join("native.rs"))?;
        let generated = fs::read_to_string(dir.join("generated.rs"))?;
        fs::remove_dir_all(&dir)?;

        assert!(script.contains("__native.call("), "{script}");
        assert!(native.contains("\"main::fetch_prs\" => {"), "{native}");
        assert!(native.contains("serde::de::DeserializeOwned"), "{native}");
        assert!(
            native.contains("match crate::fetch_prs(__p_limit)"),
            "{native}"
        );
        assert!(
            generated.contains(
                "include!(concat!(env!(\"OUT_DIR\"), \"/native.rs\"));"
            ),
            "{generated}"
        );
        assert!(
            generated.contains(".with_native(__noctalia_native::dispatch)"),
            "{generated}"
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with(
                "entry widget: native `main::fetch_prs` (cache 0 ms): "
            ),
            "{}",
            warnings[0]
        );
        Ok(())
    }

    #[test]
    fn generate_with_strict_rejects_native_functions() -> Result<(), BoxError> {
        let manifest = format!("compile = \"strict\"\n{DISCOVERED_MANIFEST}");
        let (manifest, dir) =
            compile_fixture("native-strict", &manifest, NATIVE_RESULT_SOURCE)?;
        let result = generate_with(&manifest, &dir, None);
        let native_exists = dir.join("native.rs").exists();
        fs::remove_dir_all(&dir)?;

        let message = result.expect_err("strict rejects native").to_string();
        assert!(message.contains("src/main.rs:"), "{message}");
        assert!(!native_exists);
        Ok(())
    }

    #[test]
    fn generate_with_fatal_luau_native_fails_auto() -> Result<(), BoxError> {
        let (manifest, dir) = compile_fixture(
            "native-luau",
            DISCOVERED_MANIFEST,
            LUAU_NATIVE_SOURCE,
        )?;
        let result = generate_with(&manifest, &dir, None);
        let native_exists = dir.join("native.rs").exists();
        fs::remove_dir_all(&dir)?;

        let message = result.expect_err("`#[luau]` is fatal").to_string();
        assert!(message.contains("#[luau]"), "{message}");
        assert!(message.contains("src/main.rs:"), "{message}");
        assert!(!native_exists);
        Ok(())
    }

    #[test]
    fn generate_with_fully_compiled_entry_stays_silent() -> Result<(), BoxError>
    {
        let (manifest, dir) = compile_fixture(
            "native-none",
            DISCOVERED_MANIFEST,
            COMPILED_HANDLER,
        )?;
        fs::write(dir.join("native.rs"), "stale")?;
        let warnings = generate_with(&manifest, &dir, None)?;
        let generated = fs::read_to_string(dir.join("generated.rs"))?;
        let native_exists = dir.join("native.rs").exists();
        fs::remove_dir_all(&dir)?;

        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!native_exists);
        assert!(!generated.contains("native.rs"), "{generated}");
        assert!(!generated.contains(".with_native("), "{generated}");
        assert!(generated.contains("    plugin\n}\n"), "{generated}");
        Ok(())
    }

    #[test]
    fn generate_with_plain_native_uses_direct_value() -> Result<(), BoxError> {
        let (manifest, dir) = compile_fixture(
            "native-plain",
            DISCOVERED_MANIFEST,
            PLAIN_NATIVE_SOURCE,
        )?;
        let warnings = generate_with(&manifest, &dir, None)?;
        let native = fs::read_to_string(dir.join("native.rs"))?;
        fs::remove_dir_all(&dir)?;

        assert!(
            native.contains("serde_json::to_value(crate::now())"),
            "{native}"
        );
        assert!(!native.contains("match crate::now("), "{native}");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        Ok(())
    }

    #[test]
    fn generate_with_native_context_builds_the_snapshot() -> Result<(), BoxError>
    {
        let (manifest, dir) = compile_fixture(
            "native-context",
            NATIVE_CONTEXT_MANIFEST,
            NATIVE_CONTEXT_SOURCE,
        )?;
        let warnings = generate_with(&manifest, &dir, None)?;
        let native = fs::read_to_string(dir.join("native.rs"))?;
        let script = fs::read_to_string(dir.join("luau/widget.luau"))?;
        fs::remove_dir_all(&dir)?;

        assert!(
            native.contains("context: Option<&noctalia_rs::Context>,"),
            "{native}"
        );
        assert!(
            native
                .contains("let __handler_context = __noctalia_context.ok_or("),
            "{native}"
        );
        assert!(
            native.contains("crate::describe(__handler_context)"),
            "{native}"
        );
        assert!(script.contains("CONFIG_KEYS = { \"message\" }"), "{script}");
        assert!(
            script.contains(
                "__native.call(\"main::describe\", {}, 0, true, true)"
            ),
            "{script}"
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        Ok(())
    }

    const NATIVE_PATH_SOURCE: &str = r#"use noctalia_rs::{Context, Op, Result, native};

#[native]
fn inspect(path: Option<&Path>) -> bool {
    path.is_none()
}

fn handler(_ctx: &Context) -> Result<Vec<Op>> {
    let missing = inspect(None);
    Ok(vec![Op::log(format!("{missing}"))])
}
"#;

    const NATIVE_ENTRY_SETTINGS_MANIFEST: &str = "id = \"me/test\"\nname = \"Test\"\n\n[[setting]]\nkey = \"shared\"\ntype = \"string\"\nlabel_key = \"settings.shared.label\"\ndefault = \"s\"\n\n[[widget]]\nid = \"widget\"\nentry = \"widget.luau\"\nhandler = \"handler\"\n\n[[widget.setting]]\nkey = \"own\"\ntype = \"string\"\nlabel_key = \"settings.own.label\"\ndefault = \"o\"\n\n[[panel]]\nid = \"panel\"\nentry = \"panel.luau\"\nhandler = \"handler\"\n\n[[panel.setting]]\nkey = \"other\"\ntype = \"string\"\nlabel_key = \"settings.other.label\"\ndefault = \"x\"\n";

    const NATIVE_ENTRY_SETTINGS_SOURCE: &str = r#"use noctalia_rs::{Context, Op, Result, native};

#[native]
fn describe(ctx: &Context) -> String {
    ctx.entry.clone()
}

fn handler(ctx: &Context) -> Result<Vec<Op>> {
    Ok(vec![Op::log(describe(ctx))])
}
"#;

    #[test]
    fn generate_with_native_path_parameters_import_pathbuf()
    -> Result<(), BoxError> {
        let (manifest, dir) = compile_fixture(
            "native-path",
            DISCOVERED_MANIFEST,
            NATIVE_PATH_SOURCE,
        )?;
        let _warnings = generate_with(&manifest, &dir, None)?;
        let native = fs::read_to_string(dir.join("native.rs"))?;
        fs::remove_dir_all(&dir)?;

        assert!(native.contains("use std::path::PathBuf;"), "{native}");
        assert!(
            native.contains("let __p_path: Option<PathBuf> ="),
            "{native}"
        );
        assert!(
            native.contains("crate::inspect(__p_path.as_deref())"),
            "{native}"
        );
        Ok(())
    }

    #[test]
    fn generate_with_native_context_uses_only_entry_settings()
    -> Result<(), BoxError> {
        let (manifest, dir) = compile_fixture(
            "native-entry-settings",
            NATIVE_ENTRY_SETTINGS_MANIFEST,
            NATIVE_ENTRY_SETTINGS_SOURCE,
        )?;
        let _warnings = generate_with(&manifest, &dir, None)?;
        let script = fs::read_to_string(dir.join("luau/widget.luau"))?;
        fs::remove_dir_all(&dir)?;

        assert!(
            script.contains("CONFIG_KEYS = { \"shared\", \"own\" }"),
            "{script}"
        );
        assert!(!script.contains("\"other\""), "{script}");
        Ok(())
    }
}
