use chrono::Local;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::borrow::Cow;
use std::env;
use std::fs;
use std::io;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::Result;

pub struct Parts {
    pub entry: String,
    pub event: String,
    pub payload: Value,
    pub config: Value,
    pub state: Value,
    pub host: Value,
    pub data_dir: PathBuf,
}

pub struct Context {
    pub entry: String,
    pub event: String,
    pub payload: Value,
    pub config: Value,
    pub state: Value,
    pub host: Value,
    pub data_dir: PathBuf,
    pub now_ms: i64,
    translations: Value,
}

impl Context {
    #[inline]
    #[must_use]
    pub fn new(parts: Parts, translations: Value) -> Self {
        let now_ms = parts
            .host
            .get("now_ms")
            .and_then(Value::as_i64)
            .unwrap_or_else(now_ms);

        Self {
            entry: parts.entry,
            event: parts.event,
            payload: parts.payload,
            config: parts.config,
            state: parts.state,
            host: parts.host,
            data_dir: parts.data_dir,
            now_ms,
            translations,
        }
    }

    #[inline]
    #[must_use]
    pub fn config_str(&self, key: &str) -> Option<Cow<'_, str>> {
        match object_get(&self.config, key)? {
            Value::String(value) => Some(Cow::Borrowed(value.as_str())),
            Value::Bool(value) => Some(Cow::Owned(value.to_string())),
            Value::Number(value) => Some(Cow::Owned(value.to_string())),
            Value::Array(_) | Value::Object(_) | Value::Null => None,
        }
    }

    #[inline]
    #[must_use]
    pub fn config_bool(&self, key: &str, default: bool) -> bool {
        object_get(&self.config, key)
            .and_then(Value::as_bool)
            .unwrap_or(default)
    }

    #[inline]
    #[must_use]
    pub fn payload_str(&self, key: &str) -> Option<String> {
        match self.payload.get(key)? {
            Value::String(value) => Some(value.clone()),
            Value::Number(value) => Some(value.to_string()),
            Value::Bool(_)
            | Value::Array(_)
            | Value::Object(_)
            | Value::Null => None,
        }
    }

    #[inline]
    #[must_use]
    pub fn payload_arg(&self, index: usize) -> Option<&Value> {
        self.payload.get("args")?.get(index)
    }

    pub fn state_value<T>(&self, key: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let value = self.state.get(key).unwrap_or(&Value::Null);
        Ok(T::deserialize(value)?)
    }

    #[inline]
    #[must_use]
    pub fn host(&self, path: &str) -> Option<&Value> {
        lookup(&self.host, path)
    }

    #[inline]
    #[must_use]
    pub fn setting(&self, path: &str) -> Option<&Value> {
        object_get(&self.host, "setting")?.get(path)
    }

    #[inline]
    #[must_use]
    pub fn color(&self, role: &str) -> Option<&Value> {
        object_get(&self.host, "colors")?.get(role)
    }

    #[inline]
    #[must_use]
    pub fn clipboard(&self) -> Option<&Value> {
        self.host.get("clipboard")
    }

    #[inline]
    #[must_use]
    pub fn calendar(&self) -> Option<&Value> {
        self.host.get("calendar")
    }

    #[inline]
    #[must_use]
    pub fn media(&self) -> Option<&Value> {
        self.host.get("media")
    }

    #[inline]
    #[must_use]
    pub fn stats(&self) -> Option<&Value> {
        self.host.get("stats")
    }

    #[inline]
    #[must_use]
    pub fn nightlight(&self) -> Option<&Value> {
        self.host.get("nightlight")
    }

    #[inline]
    #[must_use]
    pub fn compositor(&self) -> Option<&Value> {
        self.host.get("compositor")
    }

    #[inline]
    #[must_use]
    pub fn dark_mode(&self) -> bool {
        self.host
            .get("dark")
            .and_then(Value::as_bool)
            .unwrap_or_default()
    }
    #[inline]
    #[must_use]
    pub fn bar(&self) -> Option<&Value> {
        self.host.get("bar")
    }

    #[inline]
    #[must_use]
    pub fn bar_vertical(&self) -> bool {
        self.bar()
            .and_then(|value| value.get("vertical"))
            .and_then(Value::as_bool)
            .unwrap_or_default()
    }

    #[inline]
    #[must_use]
    pub fn bar_output_name(&self) -> Option<&str> {
        self.bar()?.get("output_name")?.as_str()
    }

    #[inline]
    #[must_use]
    pub fn env(&self, key: &str) -> Option<String> {
        env::var(key).ok()
    }

    #[inline]
    #[must_use]
    pub fn expand_path(&self, value: &str) -> PathBuf {
        if let (Some(rest), Some(home)) =
            (value.strip_prefix("~/"), env::var_os("HOME"))
        {
            return PathBuf::from(home).join(rest);
        }
        PathBuf::from(value)
    }

    #[inline]
    #[must_use]
    pub fn command_exists(&self, command: &str) -> bool {
        if command.contains(MAIN_SEPARATOR) {
            return Path::new(command).is_file();
        }
        env::var_os("PATH").is_some_and(|paths| {
            env::split_paths(&paths).any(|dir| dir.join(command).is_file())
        })
    }

    #[inline]
    #[must_use]
    pub fn format_time(&self, format: &str) -> String {
        let pattern = format
            .strip_prefix("{:")
            .and_then(|value| value.strip_suffix('}'))
            .unwrap_or(format);
        Local::now().format(pattern).to_string()
    }
    #[inline]
    #[must_use]
    pub fn file_exists(&self, path: &str) -> bool {
        self.expand_path(path).exists()
    }

    #[inline]
    pub fn read_file(&self, path: &str) -> io::Result<String> {
        fs::read_to_string(self.expand_path(path))
    }

    #[inline]
    pub fn write_file(&self, path: &str, contents: &str) -> io::Result<()> {
        let target = self.expand_path(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, contents)
    }

    #[inline]
    pub fn list_dir(&self, path: &str) -> io::Result<Vec<PathBuf>> {
        fs::read_dir(self.expand_path(path))?
            .map(|entry| entry.map(|value| value.path()))
            .collect()
    }

    #[inline]
    pub fn mkdir_all(&self, path: &str) -> io::Result<()> {
        fs::create_dir_all(self.expand_path(path))
    }

    #[inline]
    pub fn remove_file(&self, path: &str) -> io::Result<()> {
        fs::remove_file(self.expand_path(path))
    }

    #[inline]
    #[must_use]
    pub const fn translations(&self) -> &Value {
        &self.translations
    }
    pub fn tr(&self, key: &str, params: &[(&str, String)]) -> String {
        let mut result = lookup(&self.translations, key)
            .and_then(Value::as_str)
            .unwrap_or(key)
            .to_owned();

        for (name, value) in params {
            result = result.replace(&format!("{{{name}}}"), value);
        }

        result
    }
}

fn now_ms() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}

#[inline]
fn object_get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object()?.get(key)
}

#[inline]
fn lookup<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    let mut parts = path.split('.');
    let mut value = object_get(root, parts.next()?)?;
    for part in parts {
        value = object_get(value, part)?;
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_context() -> Context {
        let parts = Parts {
            entry: "service".to_owned(),
            event: "update".to_owned(),
            payload: json!({ "args": ["10m"] }),
            config: json!({ "enabled": true, "name": "Example" }),
            state: json!({ "timers": [1, 2, 3] }),
            host: json!({
                "now_ms": 42,
                "dark": true,
                "clipboard": "text",
                "colors": { "primary": "#fff" },
                "setting": { "shell.font_family": "sans" },
                "compositor": "niri",
                "bar": { "vertical": true, "output_name": "eDP-1" }
            }),
            data_dir: PathBuf::from("/tmp/noctalia"),
        };
        let translations = json!({
            "notify": { "title": "Timer {kind} done" }
        });
        Context::new(parts, translations)
    }

    #[test]
    fn test_now_ms_reads_host_snapshot() {
        let context = make_context();

        assert_eq!(context.now_ms, 42);
    }

    #[test]
    fn test_config_str_reads_string_and_bool_values() {
        let context = make_context();

        assert_eq!(context.config_str("name").as_deref(), Some("Example"));
        assert_eq!(context.config_str("enabled").as_deref(), Some("true"));
        assert!(matches!(
            context.config_str("name"),
            Some(Cow::Borrowed("Example"))
        ));
        assert_eq!(context.config_str("missing"), None);
    }

    #[test]
    fn test_config_bool_falls_back_to_default() {
        let context = make_context();

        assert!(context.config_bool("enabled", false));
        assert!(!context.config_bool("missing", false));
    }

    #[test]
    fn test_payload_arg_returns_nested_argument() {
        let context = make_context();

        assert_eq!(context.payload_arg(0), Some(&json!("10m")));
        assert_eq!(context.payload_arg(1), None);
    }

    #[test]
    fn test_state_value_deserializes_timers() {
        let context = make_context();

        let timers: Vec<u32> =
            context.state_value("timers").expect("valid timers");

        assert_eq!(timers, vec![1, 2, 3]);
    }

    #[test]
    fn test_host_reads_expose_snapshot_values() {
        let context = make_context();

        assert_eq!(context.setting("shell.font_family"), Some(&json!("sans")));
        assert_eq!(context.color("primary"), Some(&json!("#fff")));
        assert_eq!(context.compositor(), Some(&json!("niri")));
        assert_eq!(context.clipboard(), Some(&json!("text")));
        assert!(context.dark_mode());
        assert!(context.bar_vertical());
        assert_eq!(context.bar_output_name(), Some("eDP-1"));
        assert_eq!(context.calendar(), None);
        assert_eq!(context.host("bar.vertical"), Some(&json!(true)));
        assert_eq!(context.host("bar.missing"), None);
        assert_eq!(context.host("missing.key"), None);
    }

    #[test]
    fn test_tr_interpolates_dotted_keys() {
        let context = make_context();
        let params = [("kind", "countdown".to_owned())];

        assert_eq!(context.tr("notify.title", &params), "Timer countdown done");
        assert_eq!(context.tr("missing.key", &[]), "missing.key");
    }
    #[test]
    fn test_native_path_and_file_helpers() {
        let context = make_context();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = env::temp_dir().join(format!("noctalia-context-{unique}"));
        let file = dir.join("nested/file.txt");
        let path = file.to_string_lossy();

        context.write_file(&path, "hello").expect("write");
        assert!(context.file_exists(&path));
        assert_eq!(context.read_file(&path).expect("read"), "hello");
        assert_eq!(
            context
                .list_dir(dir.to_str().expect("dir"))
                .expect("list")
                .len(),
            1
        );
        context.remove_file(&path).expect("remove");
        fs::remove_dir_all(&dir).expect("cleanup");
        assert!(!context.file_exists(&path));
    }

    #[test]
    fn test_expand_path_and_command_lookup() {
        let context = make_context();
        let home = env::var("HOME").expect("home");

        assert_eq!(
            context.expand_path("~/projects"),
            PathBuf::from(home).join("projects")
        );
        assert!(!context.command_exists("noctalia-missing-command-xyz"));
    }
    #[test]
    fn test_format_time_accepts_noctalia_brace_syntax() {
        let context = make_context();

        let clock = context.format_time("{:%H:%M}");

        assert_eq!(clock.len(), 5);
        assert!(clock.contains(':'));
    }
}
