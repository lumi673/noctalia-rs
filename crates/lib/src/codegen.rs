use std::fmt::Write as _;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::bridge::SOURCE;
use crate::manifest::{
    Entry, EntryConfig, EntryKind, LayerConfig, NumberConfig, Plugin, Setting,
    SettingKind,
};

fn config(entry: &Entry) -> EntryConfig {
    entry.config.as_deref().cloned().unwrap_or_default()
}

pub fn emit(plugin: &Plugin, dir: &Path, helper: &str) -> io::Result<()> {
    fs::create_dir_all(dir.join("lib"))?;
    fs::create_dir_all(dir.join("translations"))?;

    fs::write(dir.join("plugin.toml"), manifest(plugin))?;

    let translations =
        serde_json::to_string_pretty(&plugin.translations_value())
            .map_err(io::Error::other)?;
    fs::write(
        dir.join("translations/en.json"),
        format!("{translations}\n"),
    )?;

    for (language, content) in &plugin.translation_files {
        safe_path(language)?;
        if language.contains('/')
            || language.to_ascii_lowercase().ends_with(".json")
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("invalid translation language: {language}"),
            ));
        }
        fs::write(
            dir.join(format!("translations/{language}.json")),
            format!("{content}\n"),
        )?;
    }

    for (path, content) in &plugin.assets {
        let relative = safe_path(path)?;
        let text = relative.to_string_lossy();
        let reserved = text == "plugin.toml"
            || text.starts_with("lib/")
            || text.starts_with("translations/")
            || text.ends_with(".luau");
        if reserved {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("reserved asset path: {path}"),
            ));
        }
        let target = dir.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, content)?;
    }
    let bridge = SOURCE
        .replace("{{HELPER}}", helper)
        .replace("{{PREFIX}}", &plugin.state_key());
    fs::write(dir.join("lib/bridge.luau"), bridge)?;

    for entry in &plugin.entries {
        let config = config(entry);
        let script = config.script.as_deref().unwrap_or(&entry.id);
        let name = format!("{script}.luau");
        let target = dir.join(name);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, entry_script(entry, &plugin.settings, script))?;
    }

    Ok(())
}

fn quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn toml_key(value: &str) -> String {
    let bare = !value.is_empty()
        && value.chars().all(|item| {
            item.is_ascii_alphanumeric() || item == '_' || item == '-'
        });
    if bare { value.to_owned() } else { quote(value) }
}

fn safe_path(value: &str) -> io::Result<PathBuf> {
    let path = Path::new(value);
    let safe = path
        .components()
        .all(|part| matches!(part, Component::Normal(_)));
    if safe {
        Ok(path.to_path_buf())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("unsafe asset path: {value}"),
        ))
    }
}

fn toml_list(values: &[String]) -> String {
    let items = values.iter().map(|value| quote(value)).collect::<Vec<_>>();
    format!("[{}]", items.join(", "))
}

fn toml_map(values: &[(String, String)]) -> String {
    let items = values
        .iter()
        .map(|(key, value)| format!("{} = {}", quote(key), quote(value)))
        .collect::<Vec<_>>();
    format!("{{ {} }}", items.join(", "))
}

fn manifest(plugin: &Plugin) -> String {
    let meta = &plugin.meta;
    let mut out = format!(
        "id = {}\nname = {}\nversion = {}\nplugin_api = {}\nauthor = {}\nlicense = {}\n",
        quote(&meta.id),
        quote(&meta.name),
        quote(&meta.version),
        meta.plugin_api,
        quote(&meta.author),
        quote(&meta.license)
    );

    if !meta.description.is_empty() {
        let _ = writeln!(out, "description = {}", quote(&meta.description));
    }
    if !meta.tags.is_empty() {
        let _ = writeln!(out, "tags = {}", toml_list(&meta.tags));
    }
    if !meta.icon.is_empty() {
        let _ = writeln!(out, "icon = {}", quote(&meta.icon));
    }
    let _ = writeln!(out, "dependencies = {}", toml_list(&meta.dependencies));
    if meta.deprecated {
        let _ = writeln!(out, "deprecated = true");
    }

    for setting in &plugin.settings {
        out.push('\n');
        write_setting(&mut out, setting, "");
    }

    for entry in &plugin.entries {
        out.push('\n');
        write_entry(&mut out, entry);
    }

    out
}

const fn entry_table(kind: &EntryKind) -> &'static str {
    match kind {
        EntryKind::Service => "service",
        EntryKind::Widget => "widget",
        EntryKind::Shortcut => "shortcut",
        EntryKind::LauncherProvider { .. } => "launcher_provider",
        EntryKind::DesktopWidget => "desktop_widget",
        EntryKind::Panel { .. } => "panel",
        EntryKind::Layer { .. } => "layer",
    }
}

fn write_entry(out: &mut String, entry: &Entry) {
    let config = config(entry);
    let name = config.script.as_deref().unwrap_or(&entry.id);
    let script = format!("{name}.luau");
    let table = entry_table(&entry.kind);
    let _ = write!(
        out,
        "[[{table}]]\nid = {}\nentry = {}\n",
        quote(&entry.id),
        quote(&script)
    );
    write_entry_config(out, entry);

    match &entry.kind {
        EntryKind::Layer { config } => write_layer_config(out, config),
        EntryKind::LauncherProvider { .. } => write_launcher(out, entry),
        EntryKind::Panel { .. } => write_panel(out, entry),
        EntryKind::Service
        | EntryKind::Widget
        | EntryKind::Shortcut
        | EntryKind::DesktopWidget => {}
    }
    if !config.action_map.is_empty() {
        let _ = writeln!(out, "[{table}.actions]");
        for (key, value) in &config.action_map {
            let _ = writeln!(out, "{key} = {}", quote(value));
        }
    }

    for setting in &entry.settings {
        write_setting(out, setting, table);
    }
}

fn write_launcher(out: &mut String, entry: &Entry) {
    let config = config(entry);
    let EntryKind::LauncherProvider { prefix, glyph } = &entry.kind else {
        return;
    };
    let _ = write!(
        out,
        "prefix = {}\nglyph = {}\n",
        quote(prefix),
        quote(glyph)
    );
    let include = config.include_in_global_search.unwrap_or(false);
    let _ = writeln!(out, "include_in_global_search = {include}");
    for category in &config.categories {
        let _ = writeln!(out, "\n[[launcher_provider.category]]");
        let _ = writeln!(out, "label = {}", quote(&category.label));
        let _ = writeln!(out, "glyph = {}", quote(&category.glyph));
    }
}

fn write_panel(out: &mut String, entry: &Entry) {
    let config = config(entry);
    let EntryKind::Panel { width, height } = entry.kind else {
        return;
    };
    if let Some(value) = &config.width_text {
        let _ = writeln!(out, "width = {}", quote(value));
    } else {
        let _ = writeln!(out, "width = {width}");
    }
    if let Some(value) = &config.height_text {
        let _ = writeln!(out, "height = {}", quote(value));
    } else {
        let _ = writeln!(out, "height = {height}");
    }
    let placement = config.placement.as_deref().unwrap_or("floating");
    let position = config.position.as_deref().unwrap_or("center");
    let _ = writeln!(out, "placement = {}", quote(placement));
    let _ = writeln!(out, "position = {}", quote(position));
    if let Some(value) = &config.panel_layer {
        let _ = writeln!(out, "layer = {}", quote(value));
    }
}

fn write_entry_config(out: &mut String, entry: &Entry) {
    let config = config(entry);
    if !config.actions.is_empty() {
        let _ = writeln!(out, "actions = {}", toml_list(&config.actions));
    }
    if !config.capture_keys_list.is_empty() {
        let _ = writeln!(
            out,
            "capture_keys = {}",
            toml_list(&config.capture_keys_list)
        );
    }
    if let Some(value) = &config.keyboard_focus_text {
        let _ = writeln!(out, "keyboard_focus = {}", quote(value));
    }
    let bools = [
        ("hover_highlight", config.hover_highlight),
        ("open_near_click", config.open_near_click),
        ("dismiss_on_outside_click", config.dismiss_on_outside_click),
        ("keyboard_focus", config.keyboard_focus),
        ("capture_keys", config.capture_keys),
        ("persistent", config.persistent),
    ];
    for (key, selected) in bools {
        if let Some(item) = selected {
            let _ = writeln!(out, "{key} = {item}");
        }
    }
    let strings = [("category", config.category.as_deref())];
    for (key, selected) in strings {
        if let Some(item) = selected {
            let _ = writeln!(out, "{key} = {}", quote(item));
        }
    }
    let numbers = [
        ("max_height", config.max_height),
        ("debounce_ms", config.debounce_ms),
    ];
    for (key, selected) in numbers {
        if let Some(item) = selected {
            let _ = writeln!(out, "{key} = {item}");
        }
    }
    if let EntryKind::Layer { config: layer } = &entry.kind {
        if let Some(value) = layer.click_through {
            let _ = writeln!(out, "click_through = {value}");
        }
        if let Some(value) = layer.exclusive_zone {
            let _ = writeln!(out, "exclusive_zone = {value}");
        }
        if !layer.margins.is_empty() {
            let pairs = layer
                .margins
                .iter()
                .map(|(key, value)| (key.clone(), value.to_string()))
                .collect::<Vec<_>>();
            let items = pairs
                .iter()
                .map(|(key, value)| format!("{} = {value}", toml_key(key)))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "margins = {{ {items} }}");
        }
        if let Some(value) = layer.width {
            let _ = writeln!(out, "width = {value}");
        }
        if let Some(value) = layer.height {
            let _ = writeln!(out, "height = {value}");
        }
    }
}

fn write_layer_config(out: &mut String, config: &LayerConfig) {
    let _ = writeln!(out, "layer = {}", quote(&config.layer));
    let _ = writeln!(out, "anchors = {}", toml_list(&config.anchors));
    let _ = writeln!(out, "visible = {}", config.visible);
    let _ = writeln!(out, "keyboard = {}", quote(&config.keyboard));
    let _ = writeln!(out, "output = {}", quote(&config.output));
}

fn setting_table(table: &str) -> String {
    if table.is_empty() {
        "setting".to_owned()
    } else {
        format!("{table}.setting")
    }
}
fn f64_literal_local(value: f64) -> String {
    format!("{value:?}")
}

fn write_setting(out: &mut String, setting: &Setting, table: &str) {
    let _ = write!(
        out,
        "\n  [[{}]]\nkey = {}\nlabel_key = {}\n",
        setting_table(table),
        quote(&setting.key),
        quote(&setting.label_key)
    );

    if let Some(key) = &setting.description_key {
        let _ = writeln!(out, "description_key = {}", quote(key));
    }
    if let Some(value) = setting.step {
        let _ = writeln!(out, "step = {}", f64_literal_local(value));
    }
    if let Some(value) = setting.advanced {
        let _ = writeln!(out, "advanced = {value}");
    }
    if let Some((key, values)) = &setting.visible_when {
        let _ = writeln!(
            out,
            "visible_when = {{ key = {}, values = {} }}",
            quote(key),
            toml_list(values)
        );
    }

    write_setting_kind(out, setting, table);
}

fn write_setting_kind(out: &mut String, setting: &Setting, table: &str) {
    match &setting.kind {
        SettingKind::Bool(value) => {
            let _ = write!(out, "type = \"bool\"\ndefault = {value}\n");
        }
        SettingKind::String(value)
        | SettingKind::Folder(value)
        | SettingKind::Glyph(value)
        | SettingKind::Color(value) => {
            let ty = match setting.kind {
                SettingKind::String(_) => "string",
                SettingKind::Folder(_) => "folder",
                SettingKind::Glyph(_) => "glyph",
                SettingKind::Color(_) => "color",
                SettingKind::Bool(_)
                | SettingKind::StringList(_)
                | SettingKind::StringMap(_)
                | SettingKind::Int(_)
                | SettingKind::Double(_)
                | SettingKind::Select { .. }
                | SettingKind::File { .. } => {
                    unreachable!("matched setting kind")
                }
            };
            let _ =
                write!(out, "type = \"{ty}\"\ndefault = {}\n", quote(value));
        }
        SettingKind::StringList(values) => {
            let _ = write!(
                out,
                "type = \"string_list\"\ndefault = {}\n",
                toml_list(values)
            );
        }
        SettingKind::StringMap(values) => {
            let pairs = values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>();
            let _ = write!(
                out,
                "type = \"string_map\"\ndefault = {}\n",
                toml_map(&pairs)
            );
        }
        SettingKind::Int(config) => write_number(out, config, false),
        SettingKind::Double(config) => write_number(out, config, true),
        SettingKind::Select { default, options } => {
            let _ = write!(
                out,
                "type = \"select\"\ndefault = {}\n",
                quote(default)
            );
            let options_table = format!("{}.options", setting_table(table));
            for (value, label_key) in options.iter() {
                let _ = write!(
                    out,
                    "\n  [[{options_table}]]\nvalue = {}\nlabel_key = {}\n",
                    quote(value),
                    quote(label_key)
                );
            }
        }
        SettingKind::File {
            default,
            extensions,
        } => {
            let _ =
                write!(out, "type = \"file\"\ndefault = {}\n", quote(default));
            let _ = writeln!(out, "extensions = {}", toml_list(extensions));
        }
    }
}

fn write_number(out: &mut String, config: &NumberConfig, is_double: bool) {
    let (default, minimum, maximum) = *config;
    let ty = if is_double { "double" } else { "int" };
    let _ = write!(out, "type = \"{ty}\"\ndefault = {default}\n");
    if let Some(value) = minimum {
        let _ = writeln!(out, "min = {value}");
    }
    if let Some(value) = maximum {
        let _ = writeln!(out, "max = {value}");
    }
}
const CALLBACKS: &str = r#"
function onClick() bridge.call("click") end
function onRightClick() bridge.call("right_click") end
function onMiddleClick() bridge.call("middle_click") end
function onDoubleClick() bridge.call("double_click") end
function onScroll(axis, steps, startsGesture) bridge.call("scroll", { axis = axis, steps = steps, starts_gesture = startsGesture }) end
function onHover(hovered) bridge.call("hover", { hovered = hovered }) end
function onKey(key, pressed) bridge.call("key", { key = key, pressed = pressed }) end
function onIpc(event, payload) bridge.call("ipc", { event = event, payload = payload }) end
function onConfigChanged(key, value) bridge.call("config_changed", { key = key, value = value }) end
function onOutputsChanged() bridge.call("outputs_changed") end
function onEnable() bridge.call("enable") end
function onDisable() bridge.call("disable") end
function onExit(signal, reason) bridge.call("exit", { signal = signal, reason = reason }) end
function onSuspend() bridge.call("suspend") end
function onResume() bridge.call("resume") end
function onRefresh() bridge.call("refresh") end
function onOpen() bridge.call("open") end
function onClose() bridge.call("close") end
function onCloseClicked() bridge.call("close_clicked") end
function onRefreshClicked() bridge.call("refresh_clicked") end
function onSettingsClicked() bridge.call("settings_clicked") end
function onOpenSettings() bridge.call("open_settings") end
function onClosePanel() bridge.call("close_panel") end
function onFrameTick(delta) bridge.call("frame", { delta = delta }) end
function onQuery(text) bridge.query(text) end
function onActivate(id) bridge.call("activate", { id = id }) end
function onSearch(text) bridge.call("search", { text = text }) end
function onSearchChange(text) bridge.call("search_change", { text = text }) end
"#;

fn entry_script(
    entry: &Entry,
    plugin_settings: &[Setting],
    script: &str,
) -> String {
    let keys = plugin_settings
        .iter()
        .chain(entry.settings.iter())
        .map(|setting| quote(&setting.key))
        .collect::<Vec<_>>()
        .join(", ");
    let depth = script.split('/').count().saturating_sub(1);
    let bridge_prefix = if depth == 0 {
        "./".to_owned()
    } else {
        "../".repeat(depth)
    };
    let mut out = format!(
        "--!nonstrict\n\nlocal bridge = require(\"{bridge_prefix}lib/bridge.luau\")\nbridge.setup({}, {{ {} }})\n\n",
        quote(&entry.id),
        keys
    );

    out.push_str(CALLBACKS);
    let update_event = match entry.kind {
        EntryKind::Service | EntryKind::LauncherProvider { .. } => "update",
        EntryKind::Widget
        | EntryKind::Shortcut
        | EntryKind::DesktopWidget
        | EntryKind::Panel { .. }
        | EntryKind::Layer { .. } => "render",
    };
    let _ =
        writeln!(out, "function update() bridge.call(\"{update_event}\") end");

    match &entry.kind {
        EntryKind::Service => {
            out.push_str("bridge.call(\"init\")\n");
        }
        EntryKind::Shortcut => {
            out.push_str(
                "bridge.watch_state(function() bridge.call(\"render\") end)\n",
            );
        }
        EntryKind::LauncherProvider { .. } => {
            out.push_str(
                "bridge.watch_state(function() bridge.rerender_query() end)\n",
            );
        }
        EntryKind::Widget
        | EntryKind::DesktopWidget
        | EntryKind::Layer { .. } => {
            out.push_str(
                "bridge.watch_state(function() bridge.call(\"render\") end)\n",
            );
            out.push_str("bridge.call(\"render\")\n");
        }
        EntryKind::Panel { .. } => {
            out.push_str(
                "bridge.watch_state(function() bridge.call(\"render\") end)\n",
            );
            out.push_str("panel.setWantsSecondTicks(true)\n");
            out.push_str("bridge.call(\"render\")\n");
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{
        Entry, EntryConfig, EntryKind, LauncherCategory, LayerConfig, Meta,
    };
    use crate::{Context, Op};
    use std::env;
    use std::process;

    fn handler(ctx: &Context) -> crate::Result<Vec<Op>> {
        let _ = ctx.payload_str("ignored").ok_or("missing payload")?;
        Ok(Vec::new())
    }

    #[test]
    fn test_manifest_places_setting_keys_before_options() {
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .widget(
                "widget",
                (
                    vec![Setting::select(
                        "color",
                        "settings.color.label",
                        ("primary", &[("primary", "Primary")]),
                    )],
                    handler,
                ),
            );

        let manifest = manifest(&plugin);
        let label = manifest.find("label_key").expect("label key");
        let options = manifest
            .find("[[widget.setting.options]]")
            .expect("options");

        assert!(label < options);
    }

    #[test]
    fn test_manifest_supports_all_entry_kinds() {
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .service("service", handler)
            .widget("widget", (Vec::new(), handler))
            .shortcut("shortcut", handler)
            .launcher("launcher", ("timer", "clock", handler))
            .desktop_widget("desktop", handler)
            .panel("panel", (320, 240, handler))
            .layer("layer", (LayerConfig::new(), handler));

        let manifest = manifest(&plugin);

        for table in [
            "[[service]]",
            "[[widget]]",
            "[[shortcut]]",
            "[[launcher_provider]]",
            "[[desktop_widget]]",
            "[[panel]]",
            "[[layer]]",
        ] {
            assert!(manifest.contains(table), "missing {table}");
        }
    }

    #[test]
    fn test_manifest_supports_all_setting_types() {
        let settings = vec![
            Setting::bool("bool", "label.bool", true),
            Setting::string("string", "label.string", "value"),
            Setting::string_list("list", "label.list", &["a", "b"]),
            Setting::string_map("map", "label.map", &[("a", "b")]),
            Setting::int("int", "label.int", (1.0, Some(0.0), Some(5.0))),
            Setting::double("double", "label.double", (1.5, None, None)),
            Setting::select("select", "label.select", ("a", &[("a", "A")])),
            Setting::file("file", "label.file", ("x", &[".txt"])),
            Setting::folder("folder", "label.folder", "/tmp"),
            Setting::glyph("glyph", "label.glyph", "clock"),
            Setting::color("color", "label.color", "primary"),
        ];
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .widget("widget", (settings, handler));
        let manifest = manifest(&plugin);

        for ty in [
            "bool",
            "string",
            "string_list",
            "string_map",
            "int",
            "double",
            "select",
            "file",
            "folder",
            "glyph",
            "color",
        ] {
            assert!(
                manifest.contains(&format!("type = \"{ty}\"")),
                "missing {ty}"
            );
        }
    }

    #[test]
    fn test_entry_script_defines_layer_callbacks() {
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .layer("layer", (LayerConfig::new(), handler));
        let script = entry_script(&plugin.entries[0], &[], "widget");

        assert!(script.contains("onFrameTick"));
        assert!(script.contains("bridge.call(\"render\")"));
    }
    #[test]
    fn test_manifest_includes_dependencies_and_deprecated() {
        let meta = Meta::new("me/hello", "Hello", "0.1.0")
            .dependencies(&["alacritty", "fzf"])
            .deprecated(true);
        let plugin = Plugin::new(meta).widget("widget", (Vec::new(), handler));

        let manifest = manifest(&plugin);

        assert!(manifest.contains("dependencies = [\"alacritty\", \"fzf\"]"));
        assert!(manifest.contains("deprecated = true"));
    }

    #[test]
    fn test_manifest_writes_entry_config_fields() {
        let layer = LayerConfig::new()
            .click_through(true)
            .exclusive_zone(-1)
            .margins(&[("top", 12)])
            .width(660)
            .height(220);
        let widget = Entry::configured(
            (
                "widget".to_owned(),
                EntryKind::Widget,
                Vec::new(),
                EntryConfig::new()
                    .actions(&["refresh", "settings"])
                    .hover_highlight(true),
            ),
            handler,
        );
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .entry(widget)
            .layer("layer", (layer, handler));

        let manifest = manifest(&plugin);

        assert!(manifest.contains("actions = [\"refresh\", \"settings\"]"));
        assert!(manifest.contains("hover_highlight = true"));
        assert!(manifest.contains("click_through = true"));
        assert!(manifest.contains("exclusive_zone = -1"));
        assert!(manifest.contains("top = 12"));
        assert!(manifest.contains("width = 660"));
        assert!(manifest.contains("height = 220"));
    }

    #[test]
    fn test_entry_script_covers_community_callbacks() {
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .widget("widget", (Vec::new(), handler));

        let script = entry_script(&plugin.entries[0], &[], "widget");

        for callback in [
            "onConfigChanged",
            "onRightClick",
            "onScroll",
            "onKey",
            "onOutputsChanged",
            "onRefreshClicked",
        ] {
            assert!(script.contains(callback), "missing {callback}");
        }
    }

    #[test]
    fn test_emit_writes_assets_and_translation_files() {
        let dir =
            env::temp_dir().join(format!("noctalia-parity-{}", process::id()));
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .translation_file("ru", "{\"hello\":\"Привет\"}")
            .asset("icons/clock.svg", b"<svg />");

        emit(&plugin, &dir, "helper").expect("emit");
        let russian =
            fs::read_to_string(dir.join("translations/ru.json")).expect("ru");
        let icon = fs::read(dir.join("icons/clock.svg")).expect("icon");
        fs::remove_dir_all(&dir).expect("cleanup");

        assert!(russian.contains("Привет"));
        assert_eq!(icon, b"<svg />");
    }
    #[test]
    fn test_plugin_level_settings_use_root_table() {
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .setting(Setting::bool("enabled", "settings.enabled", true))
            .widget("widget", (Vec::new(), handler));

        let manifest = manifest(&plugin);

        assert!(manifest.contains("[[setting]]"));
        assert!(!manifest.contains("[[widget.setting]]"));
    }

    #[test]
    fn test_update_event_matches_entry_kind() {
        let widget = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .widget("widget", (Vec::new(), handler));
        let service = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .service("service", handler);

        assert!(
            entry_script(&widget.entries[0], &[], "widget")
                .contains("function update() bridge.call(\"render\") end")
        );
        assert!(
            entry_script(&service.entries[0], &[], "service")
                .contains("function update() bridge.call(\"update\") end")
        );
    }

    #[test]
    fn test_emit_rejects_reserved_asset_paths() {
        let dir =
            env::temp_dir().join(format!("noctalia-assets-{}", process::id()));
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .asset("plugin.toml", b"spoofed");

        let error = emit(&plugin, &dir, "helper").expect_err("reserved path");
        fs::remove_dir_all(&dir).expect("cleanup");

        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }
    #[test]
    fn test_nested_script_uses_relative_bridge_path() {
        let entry = Entry::configured(
            (
                "widget".to_owned(),
                EntryKind::Widget,
                Vec::new(),
                EntryConfig::new().script("entries/widget"),
            ),
            handler,
        );

        let script = entry_script(&entry, &[], "entries/widget");

        assert!(script.contains("require(\"../lib/bridge.luau\")"));
        let top_level = entry_script(&entry, &[], "widget");

        assert!(top_level.contains("require(\"./lib/bridge.luau\")"));
    }

    #[test]
    fn test_launcher_scalars_precede_categories() {
        let entry = Entry::configured(
            (
                "launcher".to_owned(),
                EntryKind::LauncherProvider {
                    prefix: "case".to_owned(),
                    glyph: "text".to_owned(),
                },
                Vec::new(),
                EntryConfig::new()
                    .debounce_ms(60)
                    .script("launcher")
                    .categories(vec![LauncherCategory::new("Text", "glyph")]),
            ),
            handler,
        );
        let plugin =
            Plugin::new(Meta::new("me/hello", "Hello", "0.1.0")).entry(entry);

        let manifest = manifest(&plugin);
        let debounce = manifest.find("debounce_ms = 60").expect("debounce");
        let category = manifest
            .find("[[launcher_provider.category]]")
            .expect("category");

        assert!(debounce < category);
    }

    #[test]
    fn test_bridge_uses_persistent_helper() {
        assert!(SOURCE.contains("noctalia.runStream"));
        assert!(!SOURCE.contains("noctalia.runAsync"));
        assert!(SOURCE.contains("noctalia.writeFile"));
        assert!(SOURCE.contains("noctalia.renameFile"));
        assert!(SOURCE.contains("shellQuote"));
        assert!(SOURCE.contains(" serve "));
    }
}
