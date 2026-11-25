//! Build-side synthesis for strict Luau entries.
//!
//! A strict entry has no bridge and no helper, so the pieces the bridge
//! assembles at runtime are produced here instead: the config table from the
//! keys declared in `noctalia.toml`, the translations table from
//! `translations/*.json`, the fixed host snapshot from direct plugin API 57
//! member calls, and the source mode of every declared asset.
//!
//! Only `strict` builds use this. Auto and off keep the bridge output.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use toml::Value;

type BoxError = Box<dyn Error>;

/// Translation templates per language, flattened to dotted keys.
pub type TranslationTables = BTreeMap<String, BTreeMap<String, String>>;

/// The host reads the bridge used to snapshot, now as direct member calls.
///
/// The names match `Context::host` paths, so the emitter can answer a
/// `ctx.host(path)` read without a runtime snapshot table.
pub const HOST_SNAPSHOT: &[(&str, &str)] = &[
    ("now_ms", "noctalia.nowMs()"),
    ("dark", "noctalia.isDarkMode()"),
    ("outputs", "noctalia.outputs()"),
    ("focused_output_name", "noctalia.focusedOutputName()"),
    ("pluginDir", "noctalia.pluginDir()"),
    ("pluginDataDir", "noctalia.pluginDataDir()"),
    ("clipboard", "noctalia.clipboardText()"),
    ("calendar", "noctalia.calendarEvents()"),
    ("media", "noctalia.mediaPlayer()"),
    ("stats", "noctalia.systemStats()"),
    ("nightlight", "noctalia.nightlight.state"),
    ("compositor", "{ name = noctalia.compositor.name }"),
    (
        "bar",
        "{ vertical = barWidget.isVertical(), output_name = barWidget.outputName() }",
    ),
];

/// One declared asset and the mode the build preserves for it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AssetMode {
    pub path: String,
    pub mode: Option<u32>,
}

/// What a strict entry needs in place of the bridge's runtime context.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StrictContext {
    /// Every config key declared for the entry, in manifest order.
    pub config: Vec<String>,
    /// Translation templates per language, flattened to dotted keys.
    pub translations: TranslationTables,
    /// Declared assets and the mode the plugin directory must carry.
    pub assets: Vec<AssetMode>,
}

impl StrictContext {
    /// Reads the manifest keys, translation files and asset modes.
    ///
    /// `config` is the entry's declared key list, plugin level plus entry
    /// level. Keys are deduplicated in place.
    pub fn build(
        root: &toml::Table,
        source: &Path,
        config: &[String],
    ) -> Result<Self, BoxError> {
        let mut keys: Vec<String> = Vec::new();
        for key in config {
            if !keys.iter().any(|existing| existing == key) {
                keys.push(key.clone());
            }
        }
        Ok(Self {
            config: keys,
            translations: translations(source)?,
            assets: asset_modes(root, source)?,
        })
    }

    /// The `ctx.config` table for a strict entry.
    ///
    /// The key list comes from the manifest, so the table has exactly the
    /// fields the Rust `Context` carries. Values are read once at load.
    pub fn config_source(&self) -> String {
        let mut out = String::from("local CONFIG = {\n");
        for key in &self.config {
            let _ = writeln!(
                out,
                "  [{}] = noctalia.getConfig({}),",
                lua_string(key),
                lua_string(key)
            );
        }
        out.push_str("}\n");
        out
    }

    /// The nested per-language tables behind `ctx.translations()`.
    fn languages_source(&self) -> String {
        let mut out = String::from("local LANGUAGES = {\n");
        for (language, templates) in &self.translations {
            let _ = writeln!(out, "  [{}] = {{", lua_string(language));
            let mut root = serde_json::Map::new();
            for (key, value) in templates {
                insert_nested(&mut root, key, value);
            }
            write_table(&mut out, &root, 4);
            out.push_str("  },\n");
        }
        out.push_str("}\n");
        out
    }

    /// The translations table embedded at build time.
    ///
    /// `Context::tr` looks a dotted path up in the language file, so each
    /// template is nested by its dotted key. Values that are not strings are
    /// already invisible to `tr` and are skipped here.
    pub fn translations_source(&self) -> String {
        let mut out = self.languages_source();
        out.push_str(
            "local TRANSLATIONS = (function()\n  local locale = noctalia.getSetting(\"shell.lang\") or noctalia.getSetting(\"shell.language\") or \"en\"\n  return LANGUAGES[locale] or LANGUAGES[\"en\"] or {}\nend)()\n",
        );
        out
    }
}

/// The fixed host snapshot, as direct member calls.
#[must_use]
pub fn host_snapshot_source() -> String {
    let mut out = String::from("local host = {\n");
    for (field, call) in HOST_SNAPSHOT {
        let _ = writeln!(out, "  [{}] = {call},", lua_string(field));
    }
    out.push_str("}\n");
    out
}

/// Declared assets with the mode their source file has.
pub fn asset_modes(
    root: &toml::Table,
    source: &Path,
) -> Result<Vec<AssetMode>, BoxError> {
    let mut result = Vec::new();
    let Some(items) = root.get("assets").and_then(Value::as_array) else {
        return Ok(result);
    };
    for item in items {
        let path = item
            .as_str()
            .ok_or_else(|| format!("asset must be a relative path: {item}"))?;
        result.push(AssetMode {
            path: path.replace('\\', "/"),
            mode: source_mode(&source.join(path)),
        });
    }
    Ok(result)
}

/// Whether the recorded mode needs emitting.
///
/// `0644` is what a plain `fs::write` produces, so only a different mode has
/// to travel from the build to the plugin directory.
#[must_use]
pub fn mode_differs(mode: Option<u32>) -> Option<u32> {
    mode.map(|value| value & 0o777)
        .filter(|value| *value != 0o644)
}

#[cfg(unix)]
fn source_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions().mode())
}

#[cfg(not(unix))]
fn source_mode(_path: &Path) -> Option<u32> {
    None
}

fn translations(source: &Path) -> Result<TranslationTables, BoxError> {
    let mut result = BTreeMap::new();
    let dir = source.join("translations");
    if !dir.is_dir() {
        return Ok(result);
    }
    for entry in fs::read_dir(&dir)
        .map_err(|error| format!("read translations: {error}"))?
    {
        let path = entry
            .map_err(|error| format!("read translation entry: {error}"))?
            .path();
        let Some(name) = path.file_name().and_then(|value| value.to_str())
        else {
            continue;
        };
        if !name.to_ascii_lowercase().ends_with(".json")
            || name.contains('/')
            || name.contains("..")
        {
            continue;
        }
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("read translation {name}: {error}"))?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| format!("parse translation {name}: {error}"))?;
        let mut templates = BTreeMap::new();
        flatten(&value, "", &mut templates);
        let language = name.trim_end_matches(".json").to_owned();
        result.insert(language, templates);
    }
    Ok(result)
}

fn flatten(
    value: &serde_json::Value,
    prefix: &str,
    out: &mut BTreeMap<String, String>,
) {
    match value {
        serde_json::Value::String(text) => {
            out.insert(prefix.to_owned(), text.clone());
        }
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(item, &path, out);
            }
        }
        _ => {}
    }
}

fn insert_nested(
    root: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: &str,
) {
    let mut parts = key.split('.').peekable();
    let mut cursor = root;
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            cursor.insert(
                part.to_owned(),
                serde_json::Value::String(value.to_owned()),
            );
            return;
        }
        let entry = cursor.entry(part.to_owned()).or_insert_with(|| {
            serde_json::Value::Object(serde_json::Map::new())
        });
        let serde_json::Value::Object(next) = entry else {
            return;
        };
        cursor = next;
    }
}

fn write_table(
    out: &mut String,
    map: &serde_json::Map<String, serde_json::Value>,
    indent: usize,
) {
    let pad = " ".repeat(indent);
    for (key, value) in map {
        match value {
            serde_json::Value::Object(inner) => {
                let _ = writeln!(out, "{pad}[{}] = {{", lua_string(key));
                write_table(out, inner, indent.saturating_add(2));
                let _ = writeln!(out, "{pad}}},");
            }
            serde_json::Value::String(text) => {
                let _ = writeln!(
                    out,
                    "{pad}[{}] = {},",
                    lua_string(key),
                    lua_string(text)
                );
            }
            _ => {}
        }
    }
}

fn lua_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch() -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |value| value.as_nanos());
        std::env::temp_dir().join(format!("noctalia-strict-{nanos}"))
    }

    #[test]
    fn config_source_uses_the_declared_keys_once() -> Result<(), BoxError> {
        let dir = scratch();
        fs::create_dir_all(&dir)?;
        let root: toml::Table = "id = \"me/test\"\n".parse()?;
        let context = StrictContext::build(
            &root,
            &dir,
            &["name".to_owned(), "name".to_owned(), "enabled".to_owned()],
        )?;
        fs::remove_dir_all(&dir)?;

        let source = context.config_source();
        assert_eq!(source.matches("name").count(), 2);
        assert!(
            source.contains("[\"enabled\"] = noctalia.getConfig(\"enabled\")")
        );
        Ok(())
    }

    #[test]
    fn translations_are_flattened_to_dotted_keys() -> Result<(), BoxError> {
        let dir = scratch();
        fs::create_dir_all(dir.join("translations"))?;
        fs::write(
            dir.join("translations/en.json"),
            r#"{"action":{"add":"Task added.","count":3},"plain":"Plain"}"#,
        )?;
        let root: toml::Table = "id = \"me/test\"\n".parse()?;
        let context = StrictContext::build(&root, &dir, &[])?;
        fs::remove_dir_all(&dir)?;

        let source = context.translations_source();
        assert!(source.contains("[\"action\"] = {"), "{source}");
        assert!(source.contains("[\"add\"] = \"Task added.\""), "{source}");
        assert!(source.contains("[\"plain\"] = \"Plain\""), "{source}");
        assert!(!source.contains("count"));
        Ok(())
    }

    #[test]
    fn host_snapshot_uses_direct_member_calls() {
        let source = host_snapshot_source();
        assert!(source.contains("[\"now_ms\"] = noctalia.nowMs()"));
        assert!(
            source.contains("[\"bar\"] = { vertical = barWidget.isVertical()")
        );
        assert!(!source.contains("invoke("));
    }

    #[test]
    fn asset_modes_record_the_source_mode() -> Result<(), BoxError> {
        let dir = scratch();
        fs::create_dir_all(dir.join("tools"))?;
        fs::write(dir.join("tools/run.sh"), "#!/bin/sh\n")?;
        fs::write(dir.join("readme.txt"), "text\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                dir.join("tools/run.sh"),
                fs::Permissions::from_mode(0o755),
            )?;
        }
        let root: toml::Table =
            "id = \"me/test\"\nassets = [\"tools/run.sh\", \"readme.txt\"]\n"
                .parse()?;
        let modes = asset_modes(&root, &dir)?;
        fs::remove_dir_all(&dir)?;

        assert_eq!(
            modes.first().map(|item| item.path.as_str()),
            Some("tools/run.sh")
        );
        #[cfg(unix)]
        {
            assert_eq!(
                mode_differs(modes.first().and_then(|item| item.mode)),
                Some(0o755)
            );
            assert_eq!(
                mode_differs(modes.get(1).and_then(|item| item.mode)),
                None
            );
        }
        Ok(())
    }
}
