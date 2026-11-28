//! Manifest validation for structs that derive `Config`.

use std::collections::BTreeMap;
use std::error::Error;
use std::path::Path;

use toml::Value;

use noctalia_rs_compiler::config::{ConfigField, ConfigKind, ConfigLiteral};
type BoxError = Box<dyn Error>;

#[derive(Clone, Debug)]
struct SettingInfo {
    kind: String,
    default: Option<Value>,
}

/// Validates derived config fields against the plugin-level manifest settings.
///
/// Entry-level settings are deliberately excluded: a strict entry's generated
/// `CONFIG` table only carries keys its own entry declares. A derived field
/// must therefore be declared once at plugin level so every entry can read it.
pub fn validate(root: &toml::Table, source: &Path) -> Result<(), BoxError> {
    let mut settings = BTreeMap::new();
    collect_settings(root.get("setting"), &mut settings);
    for file in crate::walk_all(source)? {
        let parsed = syn::parse_file(&file.source).map_err(|error| {
            syn_error(
                &crate::source_relative(&file.file, source),
                &error,
                "parse config struct",
            )
        })?;
        let defs = noctalia_rs_compiler::config::parse_file(&parsed).map_err(
            |error| {
                syn_error(
                    &crate::source_relative(&file.file, source),
                    &error,
                    "parse config struct",
                )
            },
        )?;
        for def in defs {
            for field in def.fields {
                let Some(setting) = settings.get(&field.key) else {
                    return Err(field_error(
                        &crate::source_relative(&file.file, source),
                        &field,
                        format!(
                            "config key `{}` is not declared in noctalia.toml",
                            field.key
                        ),
                    ));
                };
                if !field.kind.setting_types().contains(&setting.kind.as_str())
                {
                    return Err(field_error(
                        &crate::source_relative(&file.file, source),
                        &field,
                        format!(
                            "config field `{}` is {}, but setting `{}` is {}",
                            field.name,
                            kind_name(field.kind),
                            field.key,
                            setting.kind
                        ),
                    ));
                }
                let Some(default) = &setting.default else {
                    return Err(field_error(
                        &crate::source_relative(&file.file, source),
                        &field,
                        format!(
                            "setting `{}` needs a default for config field `{}`",
                            field.key, field.name
                        ),
                    ));
                };
                if !default_matches(default, &field) {
                    return Err(field_error(
                        &crate::source_relative(&file.file, source),
                        &field,
                        format!(
                            "default for `{}` does not match noctalia.toml",
                            field.key
                        ),
                    ));
                }
            }
        }
    }
    Ok(())
}

fn collect_settings(
    value: Option<&Value>,
    settings: &mut BTreeMap<String, SettingInfo>,
) {
    let Some(items) = value.and_then(Value::as_array) else {
        return;
    };
    for item in items {
        let Some(table) = item.as_table() else {
            continue;
        };
        let (Some(key), Some(kind)) = (
            table.get("key").and_then(Value::as_str),
            table.get("type").and_then(Value::as_str),
        ) else {
            continue;
        };
        settings.insert(
            key.to_owned(),
            SettingInfo {
                kind: kind.to_owned(),
                default: table.get("default").cloned(),
            },
        );
    }
}

fn default_matches(value: &Value, field: &ConfigField) -> bool {
    match &field.default {
        ConfigLiteral::Str(expected) => {
            value.as_str() == Some(expected.as_str())
        }
        ConfigLiteral::Bool(expected) => value.as_bool() == Some(*expected),
        ConfigLiteral::Int(expected) => {
            value.as_integer().map(i128::from) == Some(*expected)
        }
        ConfigLiteral::Float(expected) => {
            let found = value
                .as_float()
                .or_else(|| value.as_integer().map(|value| value as f64));
            found.is_some_and(|found| found == *expected)
        }
    }
}

const fn kind_name(kind: ConfigKind) -> &'static str {
    match kind {
        ConfigKind::Str => "String",
        ConfigKind::Bool => "bool",
        ConfigKind::I32 => "i32",
        ConfigKind::U32 => "u32",
        ConfigKind::F64 => "f64",
    }
}

fn field_error(file: &Path, field: &ConfigField, message: String) -> BoxError {
    let span = field.span.start();
    format!(
        "{}:{}:{}: {}",
        file.display(),
        span.line,
        span.column.saturating_add(1),
        message
    )
    .into()
}

fn syn_error(file: &Path, error: &syn::Error, label: &str) -> BoxError {
    let span = error.span().start();
    format!(
        "{}:{}:{}: {label}: {}",
        file.display(),
        span.line,
        span.column.saturating_add(1),
        error
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture(
        label: &str,
        manifest: &str,
        source: &str,
    ) -> Result<(toml::Table, std::path::PathBuf), BoxError> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let dir = std::env::temp_dir()
            .join(format!("noctalia-config-{label}-{nanos}"));
        std::fs::create_dir_all(dir.join("src"))?;
        std::fs::write(dir.join("src/main.rs"), source)?;
        let table = toml::from_str(manifest)?;
        Ok((table, dir))
    }

    const MANIFEST: &str = r#"
id = "me/test"
name = "Test"

[[setting]]
key = "message"
type = "string"
label_key = "settings.message.label"
default = "Hello"

[[setting]]
key = "enabled"
type = "bool"
label_key = "settings.enabled.label"
default = true
"#;

    const SOURCE: &str = r#"
use noctalia_rs::Config;

#[derive(Config)]
struct Settings {
    #[config(key = "message", default = "Hello")]
    message: String,
    #[config(key = "enabled", default = true)]
    enabled: bool,
}
"#;

    #[test]
    fn validates_matching_defaults() -> Result<(), BoxError> {
        let (root, dir) = fixture("match", MANIFEST, SOURCE)?;
        validate(&root, &dir)?;
        std::fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn rejects_a_default_that_differs_from_the_manifest() -> Result<(), BoxError>
    {
        let source = SOURCE.replace("default = true", "default = false");
        let (root, dir) = fixture("default", MANIFEST, &source)?;
        let error = validate(&root, &dir).expect_err("mismatched default");
        std::fs::remove_dir_all(dir)?;
        assert!(error.to_string().contains("main.rs:"), "{error}");
        assert!(error.to_string().contains("does not match"), "{error}");
        Ok(())
    }

    #[test]
    fn rejects_an_undeclared_config_key() -> Result<(), BoxError> {
        let source = SOURCE.replace("key = \"enabled\"", "key = \"missing\"");
        let (root, dir) = fixture("missing", MANIFEST, &source)?;
        let error = validate(&root, &dir).expect_err("missing key");
        std::fs::remove_dir_all(dir)?;
        assert!(error.to_string().contains("is not declared"), "{error}");
        Ok(())
    }

    #[test]
    fn rejects_a_setting_type_that_cannot_be_typed() -> Result<(), BoxError> {
        let source = SOURCE
            .replace("enabled: bool", "enabled: f64")
            .replace("default = true", "default = 1.0");
        let (root, dir) = fixture("type", MANIFEST, &source)?;
        let error = validate(&root, &dir).expect_err("type mismatch");
        std::fs::remove_dir_all(dir)?;
        assert!(error.to_string().contains("but setting"), "{error}");
        Ok(())
    }
}
