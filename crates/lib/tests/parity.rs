use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::error::Error;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::result::Result as StdResult;
use std::time::{SystemTime, UNIX_EPOCH};

use noctalia_rs::{
    Context, Entry, EntryConfig, EntryKind, LauncherCategory, LayerConfig,
    Meta, Op, Plugin, Setting,
};
use toml::Value;

const ENTRY_KINDS: [&str; 7] = [
    "widget",
    "panel",
    "shortcut",
    "desktop_widget",
    "launcher_provider",
    "service",
    "layer",
];

const EMBEDDED_FIXTURE: &str = r#"
id = "me/parity"
name = "Parity"
version = "0.1.0"
plugin_api = 57
author = "Noctalia"
license = "MIT"
description = "Embedded parity fixture"
tags = ["parity"]
icon = "puzzle"
dependencies = ["jq"]
deprecated = true

[[service]]
id = "service"
entry = "service.luau"
actions = ["tick"]
hover_highlight = true
open_near_click = true
dismiss_on_outside_click = true
keyboard_focus = true
max_height = 400
capture_keys = true
persistent = true
debounce_ms = 50
category = "Tools"

[[widget]]
id = "widget"
entry = "widget.luau"
actions = ["click"]
hover_highlight = true
open_near_click = true
dismiss_on_outside_click = true
keyboard_focus = true
max_height = 400
capture_keys = true
persistent = true
debounce_ms = 50
category = "Tools"

[[widget.setting]]
key = "bool"
type = "bool"
label_key = "settings.bool.label"
default = true

[[widget.setting]]
key = "string"
type = "string"
label_key = "settings.string.label"
default = "value"

[[widget.setting]]
key = "string_list"
type = "string_list"
label_key = "settings.string_list.label"
default = ["a", "b"]

[[widget.setting]]
key = "string_map"
type = "string_map"
label_key = "settings.string_map.label"
default = { a = "b" }

[[widget.setting]]
key = "int"
type = "int"
label_key = "settings.int.label"
default = 1
min = 0
max = 5

[[widget.setting]]
key = "double"
type = "double"
label_key = "settings.double.label"
default = 1.5
min = 0.5
max = 2.5

[[widget.setting]]
key = "select"
type = "select"
label_key = "settings.select.label"
default = "a"

[[widget.setting.options]]
value = "a"
label_key = "settings.select.a"

[[widget.setting]]
key = "file"
type = "file"
label_key = "settings.file.label"
default = "notes.txt"
extensions = [".txt"]

[[widget.setting]]
key = "folder"
type = "folder"
label_key = "settings.folder.label"
default = "/tmp"

[[widget.setting]]
key = "glyph"
type = "glyph"
label_key = "settings.glyph.label"
default = "clock"

[[widget.setting]]
key = "color"
type = "color"
label_key = "settings.color.label"
default = "primary"

[[widget]]
id = "widget-actions"
entry = "widget-actions.luau"

[widget.actions]
right = "plugin me/parity:service refresh"

[[shortcut]]
id = "shortcut"
entry = "shortcut.luau"

[[launcher_provider]]
id = "launcher"
entry = "launcher.luau"
prefix = "parity"
glyph = "search"
include_in_global_search = true

[[launcher_provider.category]]
label = "Programming"
glyph = "code"

[[launcher_provider.category]]
label = "Text"
glyph = "text-caption"

[[launcher_provider]]
id = "launcher-fields"
entry = "launcher-fields.luau"
prefix = "fields"
glyph = "list"
include_in_global_search = true
debounce_ms = 50
category = "Tools"

[[desktop_widget]]
id = "desktop"
entry = "desktop.luau"

[[panel]]
id = "panel"
entry = "panel.luau"
placement = "attached"
position = "center"
width = 320
height = 240
open_near_click = true
dismiss_on_outside_click = true
keyboard_focus = true
max_height = 480
capture_keys = true
persistent = true
debounce_ms = 50
category = "Tools"

[[panel]]
id = "panel-text"
entry = "panel-text.luau"
placement = "floating"
position = "center"
width = "content"
height = "fill"
layer = "overlay"
open_near_click = true
dismiss_on_outside_click = true
keyboard_focus = "exclusive"
max_height = 480
capture_keys = ["Up", "Down"]
persistent = true
debounce_ms = 50
category = "Tools"

[[layer]]
id = "layer"
entry = "layer.luau"
layer = "overlay"
anchors = ["top", "bottom"]
visible = true
keyboard = "exclusive"
output = "all"
click_through = true
exclusive_zone = 0
margins = { top = 12, right = 8 }
width = 320
height = 240
"#;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Shape {
    Array,
    Boolean,
    Datetime,
    Float,
    Integer,
    String,
    Table,
}

impl Shape {
    const fn of(value: &Value) -> Self {
        match value {
            Value::Array(_) => Self::Array,
            Value::Boolean(_) => Self::Boolean,
            Value::Datetime(_) => Self::Datetime,
            Value::Float(_) => Self::Float,
            Value::Integer(_) => Self::Integer,
            Value::String(_) => Self::String,
            Value::Table(_) => Self::Table,
        }
    }
}

impl fmt::Display for Shape {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Array => "array",
            Self::Boolean => "boolean",
            Self::Datetime => "datetime",
            Self::Float => "float",
            Self::Integer => "integer",
            Self::String => "string",
            Self::Table => "table",
        };
        formatter.write_str(name)
    }
}

struct Fixture {
    label: String,
    text: String,
}

struct RootField<'a> {
    key: &'a str,
    value: &'a Value,
    label: &'a str,
}

struct EntryScope<'a> {
    kind: &'a str,
    label: String,
}

struct Field<'a> {
    name: &'a str,
    value: &'a Value,
}

#[derive(Default)]
struct ManifestFacts {
    entry_kinds: BTreeSet<String>,
    setting_types: BTreeSet<String>,
    root_fields: BTreeSet<String>,
    entry_fields: BTreeMap<String, BTreeSet<Shape>>,
}

impl ManifestFacts {
    fn observe(
        &mut self,
        value: &Value,
        label: &str,
    ) -> Result<(), Box<dyn Error>> {
        for (key, value) in as_table(value, label)? {
            let field = RootField { key, value, label };
            self.observe_root_field(&field)?;
        }
        Ok(())
    }

    fn observe_root_field(
        &mut self,
        field: &RootField<'_>,
    ) -> Result<(), Box<dyn Error>> {
        if field.key == "setting" {
            return self.observe_settings(field.value, field.label);
        }
        if ENTRY_KINDS.contains(&field.key) {
            let scope = EntryScope {
                kind: field.key,
                label: format!("{}.{}", field.label, field.key),
            };
            return self.observe_entries(&scope, field.value);
        }
        self.root_fields.insert(field.key.to_owned());
        Ok(())
    }

    fn observe_entries(
        &mut self,
        scope: &EntryScope<'_>,
        value: &Value,
    ) -> Result<(), Box<dyn Error>> {
        let entries = as_array(value, &scope.label)?;
        self.entry_kinds.insert(scope.kind.to_owned());
        for (index, value) in entries.iter().enumerate() {
            let entry_scope = EntryScope {
                kind: scope.kind,
                label: format!("{}[{index}]", scope.label),
            };
            self.observe_entry(&entry_scope, value)?;
        }
        Ok(())
    }

    fn observe_entry(
        &mut self,
        scope: &EntryScope<'_>,
        entry: &Value,
    ) -> Result<(), Box<dyn Error>> {
        for (name, value) in as_table(entry, &scope.label)? {
            let field = Field { name, value };
            self.observe_entry_field(scope, &field)?;
        }
        Ok(())
    }

    fn observe_entry_field(
        &mut self,
        scope: &EntryScope<'_>,
        field: &Field<'_>,
    ) -> Result<(), Box<dyn Error>> {
        if field.name == "setting" {
            self.observe_settings(field.value, &scope.label)?;
        }
        self.entry_fields
            .entry(format!("{}.{}", scope.kind, field.name))
            .or_default()
            .insert(Shape::of(field.value));
        Ok(())
    }

    fn observe_settings(
        &mut self,
        value: &Value,
        label: &str,
    ) -> Result<(), Box<dyn Error>> {
        for setting in as_array(value, label)? {
            let setting_label = format!("{label}.setting");
            let table = as_table(setting, &setting_label)?;
            let kind = setting_kind(table, &setting_label)?;
            self.setting_types.insert(kind.to_owned());
        }
        Ok(())
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Result<Self, Box<dyn Error>> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let path = env::temp_dir()
            .join(format!("noctalia-parity-{}-{nanos}", process::id()));
        fs::create_dir(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn error(message: String) -> Box<dyn Error> {
    io::Error::other(message).into()
}

fn as_table<'a>(
    value: &'a Value,
    label: &str,
) -> Result<&'a toml::Table, Box<dyn Error>> {
    value
        .as_table()
        .ok_or_else(|| error(format!("{label}: expected a table")))
}

fn as_array<'a>(
    value: &'a Value,
    label: &str,
) -> Result<&'a [Value], Box<dyn Error>> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| error(format!("{label}: expected an array")))
}

fn setting_kind<'a>(
    table: &'a toml::Table,
    label: &str,
) -> Result<&'a str, Box<dyn Error>> {
    table
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| error(format!("{label}: no string type")))
}

fn string_field<'a>(
    table: &'a toml::Table,
    key: &str,
    label: &str,
) -> Result<&'a str, Box<dyn Error>> {
    table
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| error(format!("{label}: no string {key}")))
}

fn bool_field(
    table: &toml::Table,
    key: &str,
    label: &str,
) -> Result<bool, Box<dyn Error>> {
    table
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| error(format!("{label}: no boolean {key}")))
}

fn owner_table<'a>(
    manifest: &'a toml::Table,
    owner: &str,
    label: &str,
) -> Result<&'a toml::Table, Box<dyn Error>> {
    let value = manifest
        .get(owner)
        .ok_or_else(|| error(format!("{label}: no {owner}")))?;
    let entries = as_array(value, &format!("{label}.{owner}"))?;
    let first = entries
        .first()
        .ok_or_else(|| error(format!("{label}.{owner}: empty")))?;
    as_table(first, &format!("{label}.{owner}"))
}

fn setting_keys(
    manifest: &toml::Table,
    owner: Option<&str>,
    label: &str,
) -> Result<BTreeSet<String>, Box<dyn Error>> {
    let scope = match owner {
        Some(name) => owner_table(manifest, name, label)?,
        None => manifest,
    };
    let Some(value) = scope.get("setting") else {
        return Ok(BTreeSet::new());
    };
    let settings = as_array(value, &format!("{label}.setting"))?;
    settings
        .iter()
        .map(|setting| {
            let table = as_table(setting, &format!("{label}.setting"))?;
            string_field(table, "key", &format!("{label}.setting"))
                .map(str::to_owned)
        })
        .collect()
}

fn collect_manifests(dir: &Path, paths: &mut Vec<PathBuf>) -> io::Result<()> {
    for dir_entry in fs::read_dir(dir)? {
        let entry = dir_entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_manifests(&path, paths)?;
        } else if file_type.is_file()
            && path.file_name().and_then(|name| name.to_str())
                == Some("plugin.toml")
        {
            paths.push(path);
        }
    }
    Ok(())
}

fn load_fixtures() -> Result<Vec<Fixture>, Box<dyn Error>> {
    let Some(directory) = env::var_os("NOCTALIA_PARITY_FIXTURES") else {
        return Ok(vec![Fixture {
            label: "embedded fixture".to_owned(),
            text: EMBEDDED_FIXTURE.to_owned(),
        }]);
    };

    let root = PathBuf::from(directory);
    let mut paths = Vec::new();
    collect_manifests(&root, &mut paths)?;
    if paths.is_empty() {
        return Err(error(format!(
            "no plugin.toml files under {}",
            root.display()
        )));
    }
    paths.sort();

    paths
        .into_iter()
        .map(|path| {
            let label = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .display()
                .to_string();
            Ok(Fixture {
                label,
                text: fs::read_to_string(&path)?,
            })
        })
        .collect()
}

fn handler(ctx: &Context) -> noctalia_rs::Result<Vec<Op>> {
    let _ = ctx.payload_str("ignored").ok_or("missing payload")?;
    Ok(Vec::new())
}

fn settings() -> Vec<Setting> {
    vec![
        Setting::bool("bool", "settings.bool.label", true),
        Setting::string("string", "settings.string.label", "value"),
        Setting::string_list(
            "string_list",
            "settings.string_list.label",
            &["a", "b"],
        ),
        Setting::string_map(
            "string_map",
            "settings.string_map.label",
            &[("a", "b")],
        ),
        Setting::int("int", "settings.int.label", (1.0, Some(0.0), Some(5.0))),
        Setting::double(
            "double",
            "settings.double.label",
            (1.5, Some(0.5), Some(2.5)),
        ),
        Setting::select(
            "select",
            "settings.select.label",
            ("a", &[("a", "A")]),
        ),
        Setting::file("file", "settings.file.label", ("notes.txt", &[".txt"])),
        Setting::folder("folder", "settings.folder.label", "/tmp"),
        Setting::glyph("glyph", "settings.glyph.label", "clock"),
        Setting::color("color", "settings.color.label", "primary"),
    ]
}

fn entry_config() -> EntryConfig {
    EntryConfig::new()
        .actions(&["click"])
        .hover_highlight(true)
        .placement("attached")
        .position("top")
        .open_near_click(true)
        .dismiss_on_outside_click(true)
        .keyboard_focus(true)
        .max_height(400)
        .capture_keys(true)
        .persistent(true)
        .include_in_global_search(true)
        .debounce_ms(50)
        .category("Tools")
}

fn widget_config() -> EntryConfig {
    entry_config().script("widget")
}

fn widget_table_config() -> EntryConfig {
    EntryConfig::new()
        .script("widget-actions")
        .action_map(&[("right", "plugin me/parity:service refresh")])
}

fn launcher_category_config() -> EntryConfig {
    EntryConfig::new()
        .script("launcher")
        .include_in_global_search(true)
        .categories(vec![
            LauncherCategory::new("Programming", "code"),
            LauncherCategory::new("Text", "text-caption"),
        ])
}

fn panel_text_config() -> EntryConfig {
    EntryConfig::new()
        .script("panel-text")
        .actions(&["click"])
        .hover_highlight(true)
        .placement("floating")
        .position("center")
        .open_near_click(true)
        .dismiss_on_outside_click(true)
        .keyboard_focus_text("exclusive")
        .max_height(480)
        .capture_keys_list(&["Up", "Down"])
        .persistent(true)
        .include_in_global_search(true)
        .debounce_ms(50)
        .category("Tools")
        .width_text("content")
        .height_text("fill")
        .panel_layer("overlay")
}

fn layer_config() -> LayerConfig {
    LayerConfig::new()
        .layer("overlay")
        .anchors(&["top", "bottom"])
        .visible(true)
        .keyboard("exclusive")
        .output("all")
        .click_through(true)
        .exclusive_zone(0)
        .margins(&[("top", 12), ("right", 8)])
        .width(320)
        .height(240)
}

fn maximal_plugin() -> Plugin {
    let meta = Meta::new("me/parity", "Parity", "0.1.0")
        .author("Noctalia")
        .plugin_api(57)
        .description("Embedded parity fixture")
        .tags(&["parity"])
        .icon("puzzle")
        .dependencies(&["jq"])
        .deprecated(true);
    let kinds = [
        ("service", EntryKind::Service),
        ("widget", EntryKind::Widget),
        ("shortcut", EntryKind::Shortcut),
        (
            "launcher-fields",
            EntryKind::LauncherProvider {
                prefix: "fields".to_owned(),
                glyph: "list".to_owned(),
            },
        ),
        ("desktop", EntryKind::DesktopWidget),
        (
            "panel",
            EntryKind::Panel {
                width: 320,
                height: 240,
            },
        ),
        (
            "layer",
            EntryKind::Layer {
                config: Box::new(layer_config()),
            },
        ),
    ];
    let extras = [
        ("widget-actions", EntryKind::Widget, widget_table_config()),
        (
            "launcher",
            EntryKind::LauncherProvider {
                prefix: "parity".to_owned(),
                glyph: "search".to_owned(),
            },
            launcher_category_config(),
        ),
        (
            "panel-text",
            EntryKind::Panel {
                width: 480,
                height: 320,
            },
            panel_text_config(),
        ),
    ];
    let mut plugin = Plugin::new(meta);
    for (id, kind) in kinds {
        let config = if id == "widget" {
            widget_config()
        } else {
            entry_config()
        };
        plugin = plugin.entry(Entry::configured(
            (id.to_owned(), kind, settings(), config),
            handler,
        ));
    }
    for (id, kind, config) in extras {
        plugin = plugin.entry(Entry::configured(
            (id.to_owned(), kind, Vec::new(), config),
            handler,
        ));
    }
    plugin
}

fn shape_gaps(
    field: &str,
    shapes: &BTreeSet<Shape>,
    available: &BTreeSet<Shape>,
) -> Vec<String> {
    shapes
        .iter()
        .filter(|shape| !available.contains(*shape))
        .map(|shape| format!("entry field {field} with {shape} value"))
        .collect()
}

fn compare(observed: &ManifestFacts, generated: &ManifestFacts) -> Vec<String> {
    let mut gaps = Vec::new();

    for kind in &observed.entry_kinds {
        if !generated.entry_kinds.contains(kind) {
            gaps.push(format!("entry kind {kind}"));
        }
    }
    for kind in &observed.setting_types {
        if !generated.setting_types.contains(kind) {
            gaps.push(format!("setting type {kind}"));
        }
    }
    for field in &observed.root_fields {
        if !generated.root_fields.contains(field) {
            gaps.push(format!("root field {field}"));
        }
    }
    for (field, shapes) in &observed.entry_fields {
        let Some(available) = generated.entry_fields.get(field) else {
            gaps.push(format!("entry field {field}"));
            continue;
        };
        gaps.extend(shape_gaps(field, shapes, available));
    }

    gaps
}

#[test]
fn codegen_covers_every_manifest_feature() -> StdResult<(), Box<dyn Error>> {
    let fixtures = load_fixtures()?;
    let output = TempDir::new()?;
    maximal_plugin().emit(output.path(), "helper")?;
    let generated_text = fs::read_to_string(output.path().join("plugin.toml"))?;
    let generated_value: Value = toml::from_str(&generated_text)?;
    let mut generated = ManifestFacts::default();
    generated.observe(&generated_value, "generated plugin.toml")?;

    let mut gaps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for fixture in &fixtures {
        let value: Value = toml::from_str(&fixture.text)
            .map_err(|err| error(format!("{}: {err}", fixture.label)))?;
        let mut observed = ManifestFacts::default();
        observed.observe(&value, &fixture.label)?;
        for gap in compare(&observed, &generated) {
            gaps.entry(gap).or_default().push(fixture.label.clone());
        }
    }

    let lines = gaps
        .iter()
        .map(|(gap, sources)| {
            let first = sources.first().map_or("unknown", String::as_str);
            if sources.len() == 1 {
                format!("{gap} ({first})")
            } else {
                format!("{gap} ({} manifests, first {first})", sources.len())
            }
        })
        .collect::<Vec<_>>();
    assert!(
        gaps.is_empty(),
        "codegen parity gaps:\n{}",
        lines.join("\n")
    );

    Ok(())
}

#[test]
fn manifest_separates_root_settings_from_entry_settings()
-> StdResult<(), Box<dyn Error>> {
    const LABEL: &str = "generated plugin.toml";
    let plugin = Plugin::new(Meta::new("me/parity", "Parity", "0.1.0"))
        .setting(Setting::string(
            "plugin_key",
            "settings.plugin.label",
            "plugin",
        ))
        .entry(Entry::new(
            (
                "widget".to_owned(),
                EntryKind::Widget,
                vec![Setting::string(
                    "entry_key",
                    "settings.entry.label",
                    "entry",
                )],
            ),
            handler,
        ));
    let output = TempDir::new()?;
    plugin.emit(output.path(), "helper")?;
    let text = fs::read_to_string(output.path().join("plugin.toml"))?;
    let value: Value = toml::from_str(&text)?;
    let manifest = as_table(&value, LABEL)?;
    let root = setting_keys(manifest, None, LABEL)?;
    let widget = setting_keys(manifest, Some("widget"), LABEL)?;

    assert_eq!(root, BTreeSet::from(["plugin_key".to_owned()]));
    assert_eq!(widget, BTreeSet::from(["entry_key".to_owned()]));

    let script = fs::read_to_string(output.path().join("widget.luau"))?;
    assert!(script.contains("\"plugin_key\", \"entry_key\""));
    Ok(())
}

#[test]
fn manifest_writes_launcher_and_panel_defaults() -> StdResult<(), Box<dyn Error>>
{
    const LABEL: &str = "generated plugin.toml";
    let plugin = Plugin::new(Meta::new("me/parity", "Parity", "0.1.0"))
        .entry(Entry::new(
            (
                "launcher".to_owned(),
                EntryKind::LauncherProvider {
                    prefix: "parity".to_owned(),
                    glyph: "search".to_owned(),
                },
                Vec::new(),
            ),
            handler,
        ))
        .entry(Entry::new(
            (
                "panel".to_owned(),
                EntryKind::Panel {
                    width: 320,
                    height: 240,
                },
                Vec::new(),
            ),
            handler,
        ));
    let output = TempDir::new()?;
    plugin.emit(output.path(), "helper")?;
    let text = fs::read_to_string(output.path().join("plugin.toml"))?;
    let value: Value = toml::from_str(&text)?;
    let manifest = as_table(&value, LABEL)?;
    let launcher = owner_table(manifest, "launcher_provider", LABEL)?;
    let panel = owner_table(manifest, "panel", LABEL)?;

    assert!(!bool_field(launcher, "include_in_global_search", LABEL)?);
    assert_eq!(string_field(panel, "placement", LABEL)?, "floating");
    assert_eq!(string_field(panel, "position", LABEL)?, "center");
    Ok(())
}

#[test]
fn entry_scripts_map_update_to_expected_event() -> StdResult<(), Box<dyn Error>>
{
    let output = TempDir::new()?;
    maximal_plugin().emit(output.path(), "helper")?;

    for (id, event) in [
        ("service", "update"),
        ("launcher", "update"),
        ("widget", "render"),
        ("shortcut", "render"),
        ("desktop", "render"),
        ("panel", "render"),
        ("layer", "render"),
    ] {
        let script =
            fs::read_to_string(output.path().join(format!("{id}.luau")))?;
        let expected =
            format!("function update() bridge.call(\"{event}\") end");
        let other = if event == "render" {
            "update"
        } else {
            "render"
        };
        let wrong = format!("function update() bridge.call(\"{other}\") end");
        assert!(script.contains(&expected), "{id}: missing {expected}");
        assert!(!script.contains(&wrong), "{id}: unexpected {wrong}");
    }
    Ok(())
}

#[test]
fn emit_rejects_reserved_asset_paths() -> StdResult<(), Box<dyn Error>> {
    for path in [
        "plugin.toml",
        "lib/bridge.luau",
        "translations/en.json",
        "widget.luau",
    ] {
        let output = TempDir::new()?;
        let plugin = Plugin::new(Meta::new("me/parity", "Parity", "0.1.0"))
            .asset(path, b"reserved");
        let result = plugin.emit(output.path(), "helper");
        assert!(
            matches!(&result, Err(err) if err.kind() == io::ErrorKind::InvalidInput),
            "{path} was accepted"
        );
    }
    Ok(())
}
