use chrono::Local;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Number, Value, from_str, from_value, json};
use std::borrow::Cow;
use std::env;
use std::fs;
use std::io;
use std::mem;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::Result;
use crate::bytes::{ByteBuf, decode_json_bytes};
use crate::callback::Callback;
use crate::clock::{os_clock, unix_seconds};
use crate::handles::{
    HttpHandle, HttpStreamHandle, RunHandle, StateWatchHandle,
};
use crate::ops::Op;
use crate::rng;
use crate::timer::{IntervalHandle, TimerHandle, next_id};
use crate::ui::callback;

pub struct Parts {
    pub entry: String,
    pub event: String,
    pub payload: Value,
    pub config: Value,
    pub state: Value,
    pub host: Value,
    pub data_dir: PathBuf,
}

/// Writes one op to the host as soon as a native query is issued.
pub type OpSink = Arc<dyn Fn(&Op) + Send + Sync>;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

const QUERY_TIMEOUT: Duration = Duration::from_millis(250);
static QUERY_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

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
    op_sink: Mutex<Option<OpSink>>,
    pending_ops: Mutex<Vec<Op>>,
    query_timeout: Duration,
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
            op_sink: Mutex::new(None),
            pending_ops: Mutex::new(Vec::new()),
            query_timeout: QUERY_TIMEOUT,
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

    /// Installs the immediate op writer used for synchronous host queries.
    #[doc(hidden)]
    pub fn set_op_sink(&mut self, sink: OpSink) {
        let mut current = lock(&self.op_sink);
        *current = Some(sink);
    }

    /// Drains ops buffered while no op sink is installed.
    #[doc(hidden)]
    #[must_use]
    pub fn take_pending_ops(&self) -> Vec<Op> {
        let mut pending = lock(&self.pending_ops);
        mem::take(&mut pending)
    }

    fn emit_op(&self, op: &Op) {
        let sink = lock(&self.op_sink);
        if let Some(writer) = sink.as_ref() {
            writer(op);
            return;
        }
        drop(sink);
        let mut pending = lock(&self.pending_ops);
        pending.push(op.clone());
    }

    fn response_path(&self, id: &str) -> PathBuf {
        self.data_dir
            .join(".noctalia-rs")
            .join("query")
            .join(format!("{id}.json"))
    }

    /// Sends a host query and waits for the bridge's response envelope.
    #[doc(hidden)]
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        clippy::excessive_nesting,
        reason = "one call carries the full host query envelope"
    )]
    pub fn query_response(
        &self,
        root: &str,
        path: &[&str],
        args: Vec<Value>,
    ) -> Option<Value> {
        let sequence = QUERY_SEQUENCE
            .fetch_add(1, Ordering::Relaxed)
            .saturating_add(1);
        let id = format!("noctalia_rs_{}_{sequence}", process::id());
        let response = self.response_path(&id);
        if response
            .parent()
            .is_some_and(|parent| fs::create_dir_all(parent).is_err())
        {
            return None;
        }
        self.emit_op(&Op::query(
            &id,
            root,
            path,
            args,
            response.to_string_lossy().to_string(),
        ));
        let now = Instant::now();
        let deadline = now.checked_add(self.query_timeout).unwrap_or(now);
        loop {
            if let Ok(text) = fs::read_to_string(&response) {
                let _ = fs::remove_file(&response);
                let value: Value = from_str(&text).ok()?;
                return Some(value);
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Returns a host query's value, if any.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "one call carries the full host query envelope"
    )]
    pub fn query_value(
        &self,
        root: &str,
        path: &[&str],
        args: Vec<Value>,
    ) -> Option<Value> {
        let response = self.query_response(root, path, args)?;
        let value = response.get("value")?;
        (!value.is_null()).then(|| value.clone())
    }

    /// Returns a host query's `(value, error)` pair.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "one call carries the full host query envelope"
    )]
    pub fn query_pair(
        &self,
        root: &str,
        path: &[&str],
        args: Vec<Value>,
    ) -> (Option<Value>, Option<String>) {
        let Some(response) = self.query_response(root, path, args) else {
            return (None, None);
        };
        let value = response.get("value").filter(|value| !value.is_null());
        let error = response
            .get("error")
            .filter(|error| !error.is_null())
            .and_then(Value::as_str)
            .map(str::to_owned);
        (value.cloned(), error)
    }

    /// Returns a host query's boolean result.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "one call carries the full host query envelope"
    )]
    pub fn query_bool(
        &self,
        root: &str,
        path: &[&str],
        args: Vec<Value>,
    ) -> bool {
        self.query_value(root, path, args)
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
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

    /// Returns the reason a render event was delivered: `load`, `update` or
    /// `state`.
    #[inline]
    #[must_use]
    pub fn render_source(&self) -> &str {
        self.payload
            .get("source")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// Whether this is the one-time load/render pass.
    #[inline]
    #[must_use]
    pub fn is_load_event(&self) -> bool {
        self.event == "load" || self.render_source() == "load"
    }

    /// Returns a setting by path, querying the host for full snapshot coverage.
    #[must_use]
    pub fn setting_value(&self, path: &str) -> Option<Value> {
        if let Some(value) = self.setting(path) {
            return Some(value.clone());
        }
        self.query_value(
            "noctalia",
            &["getSetting"],
            vec![Value::String(path.to_owned())],
        )
    }

    /// Reads a plugin or global state key through the host state service.
    #[must_use]
    pub fn global_state(&self, key: &str) -> Option<Value> {
        self.query_value(
            "noctalia",
            &["state", "get"],
            vec![Value::String(key.to_owned())],
        )
    }

    /// Returns the host's fuzzy score for a pattern and text.
    #[must_use]
    pub fn fuzzy_score(&self, pattern: &str, text: &str) -> Option<f64> {
        self.query_value(
            "noctalia",
            &["fuzzyScore"],
            vec![
                Value::String(pattern.to_owned()),
                Value::String(text.to_owned()),
            ],
        )
        .and_then(|value| value.as_f64())
    }
    /// Schedules one callback after `delay_ms`.
    ///
    /// The handle cancels the timer before it fires. The callback id is the
    /// same stable id the `host_callback` branch receives.
    #[must_use]
    pub fn set_timeout(&self, delay_ms: u64, callback_id: &str) -> TimerHandle {
        let id = next_id();
        self.emit_op(&Op::timer_start(id, delay_ms, callback_id, false));
        TimerHandle::from(id)
    }

    /// Schedules a callback on every `period_ms` tick.
    #[must_use]
    pub fn set_interval(
        &self,
        period_ms: u64,
        callback_id: &str,
    ) -> IntervalHandle {
        let id = next_id();
        self.emit_op(&Op::timer_start(id, period_ms, callback_id, true));
        IntervalHandle::from(id)
    }

    /// Cancels a timeout before it fires.
    pub fn cancel_timeout(&self, handle: TimerHandle) {
        self.emit_op(&Op::timer_cancel(handle.get()));
    }

    /// Cancels a repeating timer.
    pub fn cancel_interval(&self, handle: IntervalHandle) {
        self.emit_op(&Op::timer_cancel(handle.get()));
    }

    /// Starts a process and returns an opaque handle for strict-generated code.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "the strict run handle API keeps command, callback and timeout explicit"
    )]
    pub fn run<T>(
        &self,
        command: Value,
        callback: Option<&Callback<T>>,
        timeout_ms: Option<i64>,
    ) -> RunHandle {
        let mut args = match callback {
            Some(id) => vec![command, id.value()],
            None => vec![command, Value::Null],
        };
        args.push(
            timeout_ms
                .map_or(Value::Null, |ms| Value::Number(Number::from(ms))),
        );
        RunHandle::new(self.query_bool("noctalia", &["runAsync"], args))
    }

    /// Starts an HTTP request and returns an opaque handle for strict-generated code.
    #[must_use]
    pub fn http<T>(
        &self,
        request: Value,
        callback: &Callback<T>,
    ) -> HttpHandle {
        HttpHandle::new(self.query_bool(
            "noctalia",
            &["http"],
            vec![request, callback.value()],
        ))
    }

    /// Starts an HTTP stream and returns an opaque handle for strict-generated code.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "the strict stream handle API keeps both callback ids explicit"
    )]
    pub fn http_stream<Line, Close>(
        &self,
        request: Value,
        on_line: &Callback<Line>,
        on_close: &Callback<Close>,
    ) -> HttpStreamHandle {
        let (value, _error) = self.query_pair(
            "noctalia",
            &["httpStream"],
            vec![request, on_line.value(), on_close.value()],
        );
        HttpStreamHandle::new(
            value.and_then(|item| item.as_str().map(str::to_owned)),
        )
    }

    /// Registers a state watch and returns an opaque handle for strict-generated code.
    #[must_use]
    pub fn state_watch<T>(
        &self,
        key: &str,
        callback: &Callback<T>,
    ) -> StateWatchHandle {
        self.emit_op(&Op::state_watch(key, callback.id()));
        StateWatchHandle::new(key.to_owned(), callback.id().to_owned())
    }

    /// Starts a process and returns Luau's `runAsync` acceptance boolean.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "matches the host runAsync signature"
    )]
    pub fn run_async_start(
        &self,
        command: Value,
        callback_id: Option<&str>,
        timeout_ms: Option<f64>,
    ) -> bool {
        let mut args = match callback_id {
            Some(id) => vec![command, callback(id)],
            None => vec![command, Value::Null],
        };
        args.push(
            timeout_ms
                .and_then(Number::from_f64)
                .map_or(Value::Null, Value::Number),
        );
        self.query_bool("noctalia", &["runAsync"], args)
    }

    /// Starts a process stream and returns Luau's acceptance boolean.
    #[must_use]
    pub fn run_stream_start(&self, command: Value, callback_id: &str) -> bool {
        self.query_bool(
            "noctalia",
            &["runStream"],
            vec![command, callback(callback_id)],
        )
    }

    /// Starts an HTTP request and returns Luau's acceptance boolean.
    #[must_use]
    pub fn http_start(&self, request: Value, callback_id: &str) -> bool {
        self.query_bool(
            "noctalia",
            &["http"],
            vec![request, callback(callback_id)],
        )
    }

    /// Starts an HTTP request with a raw byte body.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "matches the host http signature"
    )]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "the request is mutated with a raw byte body"
    )]
    pub fn http_bytes_start(
        &self,
        request: Value,
        body: ByteBuf,
        callback_id: &str,
    ) -> bool {
        let mut request = request;
        if let Some(object) = request.as_object_mut() {
            let _ = object.insert("body".to_owned(), json!(body));
        }
        self.http_start(request, callback_id)
    }

    /// Reads clipboard contents, including raw byte buffers.
    #[must_use]
    pub fn clipboard_bytes(&self) -> Option<ByteBuf> {
        let value =
            self.query_value("noctalia", &["clipboardText"], Vec::new())?;
        Some(ByteBuf::new(decode_json_bytes(&value)))
    }

    /// Decodes a byte buffer from an event payload field.
    #[must_use]
    pub fn payload_bytes(&self, key: &str) -> Option<ByteBuf> {
        let value = self.payload.get(key)?;
        Some(ByteBuf::new(decode_json_bytes(value)))
    }

    /// Copies raw bytes to the clipboard and returns Luau's success boolean.
    #[must_use]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "bytes are serialized into the host query"
    )]
    pub fn copy_bytes(&self, bytes: ByteBuf, mime_type: &str) -> bool {
        self.query_bool(
            "noctalia",
            &["copyToClipboard"],
            vec![json!(bytes), Value::String(mime_type.to_owned())],
        )
    }

    /// Returns Luau's `timeFormat()` value.
    #[must_use]
    pub fn time_format(&self) -> Option<String> {
        self.setting_value("shell.time_format")
            .and_then(|value| value.as_str().map(str::to_owned))
    }

    /// Returns Luau's `dateFormat()` value.
    #[must_use]
    pub fn date_format(&self) -> Option<String> {
        self.setting_value("shell.date_format")
            .and_then(|value| value.as_str().map(str::to_owned))
    }

    /// Returns Luau's `isValidTimezone()` value.
    #[must_use]
    pub fn is_valid_timezone(&self, name: &str) -> bool {
        self.query_bool(
            "noctalia",
            &["isValidTimezone"],
            vec![Value::String(name.to_owned())],
        )
    }

    /// Returns Luau's monotonic `os.clock()` value in seconds.
    #[inline]
    #[must_use]
    pub fn os_clock(&self) -> f64 {
        os_clock()
    }

    /// Returns Luau's whole-second `os.time()` value.
    #[inline]
    #[must_use]
    pub fn os_time(&self) -> i64 {
        unix_seconds()
    }

    /// Applies Luau's `math.randomseed` to the process-wide RNG.
    #[inline]
    pub fn math_randomseed(&self, seed: i64) {
        rng::seed_global(seed);
    }

    /// Returns Luau's `math.random()` value.
    #[inline]
    #[must_use]
    pub fn math_random(&self) -> f64 {
        rng::global_f64()
    }

    /// Returns Luau's `math.random(n)` value.
    #[inline]
    #[must_use]
    pub fn math_random_below(&self, upper: i64) -> Option<i64> {
        rng::global_below(upper)
    }

    /// Returns Luau's `math.random(low, high)` value.
    #[inline]
    #[must_use]
    pub fn math_random_range(&self, low: i64, high: i64) -> Option<i64> {
        rng::global_range(low, high)
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
        let path = PathBuf::from(value);
        if path.is_absolute() {
            path
        } else {
            self.plugin_dir().join(path)
        }
    }

    #[inline]
    #[must_use]
    pub fn plugin_dir(&self) -> PathBuf {
        self.host("pluginDir")
            .and_then(Value::as_str)
            .map_or_else(|| self.data_dir.clone(), PathBuf::from)
    }

    /// Returns the host's plugin data directory, preserving a nil result.
    #[must_use]
    pub fn plugin_data_dir(&self) -> Option<PathBuf> {
        self.host("pluginDataDir")
            .and_then(Value::as_str)
            .map(PathBuf::from)
    }

    /// Queries the host's plugin data directory with Luau's `(value, err)`
    /// return shape.
    #[must_use]
    pub fn plugin_data_dir_result(&self) -> (Option<PathBuf>, Option<String>) {
        let (value, error) =
            self.query_pair("noctalia", &["pluginDataDir"], Vec::new());
        (
            value.as_ref().and_then(Value::as_str).map(PathBuf::from),
            error,
        )
    }

    /// Queries the host's plugin directory with a nil-preserving value.
    #[must_use]
    pub fn plugin_dir_result(&self) -> Option<PathBuf> {
        self.query_value("noctalia", &["pluginDir"], Vec::new())
            .and_then(|value| value.as_str().map(PathBuf::from))
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
        self.format_time_at(format, None, None)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "matches the host formatTime signature"
    )]
    #[must_use]
    pub fn format_time_at(
        &self,
        format: &str,
        unix_seconds: Option<i64>,
        timezone: Option<&str>,
    ) -> String {
        let pattern = format
            .strip_prefix("{:")
            .and_then(|value| value.strip_suffix('}'))
            .unwrap_or(format);
        let Some(seconds) = unix_seconds else {
            return Local::now().format(pattern).to_string();
        };
        let Some(stamp) = chrono::DateTime::from_timestamp(seconds, 0) else {
            return Local::now().format(pattern).to_string();
        };
        match timezone {
            None | Some("") => {
                stamp.with_timezone(&Local).format(pattern).to_string()
            }
            Some("UTC") => stamp.to_utc().format(pattern).to_string(),
            Some(name) => name
                .strip_prefix("UTC")
                .and_then(parse_utc_offset)
                .map_or_else(
                    || stamp.with_timezone(&Local).format(pattern).to_string(),
                    |offset| {
                        stamp.with_timezone(&offset).format(pattern).to_string()
                    },
                ),
        }
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

    #[expect(
        clippy::too_many_arguments,
        reason = "matches the host trp signature"
    )]
    #[must_use]
    pub fn trp(
        &self,
        key: &str,
        count: i64,
        params: &[(&str, String)],
    ) -> String {
        let suffix = if count == 1 { "one" } else { "other" };
        let template = lookup(&self.translations, &format!("{key}.{suffix}"))
            .and_then(Value::as_str)
            .or_else(|| lookup(&self.translations, key).and_then(Value::as_str))
            .unwrap_or(key);
        let mut result = template.replace("{count}", &count.to_string());
        for (name, value) in params {
            result = result.replace(&format!("{{{name}}}"), value);
        }
        result
    }

    #[inline]
    pub fn file_info(&self, path: &str) -> io::Result<FileInfo> {
        let metadata = fs::metadata(self.expand_path(path))?;
        let mtime = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |elapsed| elapsed.as_secs());
        Ok(FileInfo {
            size: metadata.len(),
            mtime,
            is_dir: metadata.is_dir(),
        })
    }

    /// Reads a file as raw bytes using native path resolution.
    #[inline]
    pub fn read_file_bytes(&self, path: &str) -> io::Result<ByteBuf> {
        fs::read(self.expand_path(path)).map(ByteBuf::new)
    }

    /// Writes raw bytes using native path resolution.
    pub fn write_file_bytes(&self, path: &str, bytes: &[u8]) -> io::Result<()> {
        let target = self.expand_path(path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, bytes)
    }

    /// Renames a file using native path resolution.
    #[inline]
    pub fn rename_file(&self, from: &str, to: &str) -> io::Result<()> {
        fs::rename(self.expand_path(from), self.expand_path(to))
    }

    /// Returns Luau's `readFile(path)` result pair.
    #[must_use]
    pub fn read_file_result(
        &self,
        path: &str,
    ) -> (Option<String>, Option<String>) {
        let (value, error) = self.query_pair(
            "noctalia",
            &["readFile"],
            vec![Value::String(path.to_owned())],
        );
        (
            value.as_ref().and_then(Value::as_str).map(str::to_owned),
            error,
        )
    }

    /// Returns Luau's `readFile(path)` result as raw bytes.
    #[must_use]
    pub fn read_file_bytes_result(
        &self,
        path: &str,
    ) -> (Option<ByteBuf>, Option<String>) {
        let (value, error) = self.query_pair(
            "noctalia",
            &["readFile"],
            vec![Value::String(path.to_owned())],
        );
        (
            value
                .as_ref()
                .map(|item| ByteBuf::new(decode_json_bytes(item))),
            error,
        )
    }

    /// Returns Luau's `writeFile(path, contents)` result pair.
    #[must_use]
    pub fn write_file_result(
        &self,
        path: &str,
        contents: &str,
    ) -> (bool, Option<String>) {
        let (value, error) = self.query_pair(
            "noctalia",
            &["writeFile"],
            vec![
                Value::String(path.to_owned()),
                Value::String(contents.to_owned()),
            ],
        );
        (
            value.as_ref().and_then(Value::as_bool).unwrap_or(false),
            error,
        )
    }

    /// Returns Luau's `writeFile(path, contents)` result for raw bytes.
    #[must_use]
    #[expect(
        clippy::needless_pass_by_value,
        reason = "bytes are serialized into the host query"
    )]
    pub fn write_file_bytes_result(
        &self,
        path: &str,
        bytes: ByteBuf,
    ) -> (bool, Option<String>) {
        let (value, error) = self.query_pair(
            "noctalia",
            &["writeFile"],
            vec![Value::String(path.to_owned()), json!(bytes)],
        );
        (
            value.as_ref().and_then(Value::as_bool).unwrap_or(false),
            error,
        )
    }

    /// Returns Luau's `mkdirAll(path)` result pair.
    #[must_use]
    pub fn mkdir_all_result(&self, path: &str) -> (bool, Option<String>) {
        let (value, error) = self.query_pair(
            "noctalia",
            &["mkdirAll"],
            vec![Value::String(path.to_owned())],
        );
        (
            value.as_ref().and_then(Value::as_bool).unwrap_or(false),
            error,
        )
    }

    /// Returns Luau's `removeFile(path)` result pair.
    #[must_use]
    pub fn remove_file_result(&self, path: &str) -> (bool, Option<String>) {
        let (value, error) = self.query_pair(
            "noctalia",
            &["removeFile"],
            vec![Value::String(path.to_owned())],
        );
        (
            value.as_ref().and_then(Value::as_bool).unwrap_or(false),
            error,
        )
    }

    /// Returns Luau's `renameFile(from, to)` result pair.
    #[must_use]
    pub fn rename_file_result(
        &self,
        from: &str,
        to: &str,
    ) -> (bool, Option<String>) {
        let (value, error) = self.query_pair(
            "noctalia",
            &["renameFile"],
            vec![Value::String(from.to_owned()), Value::String(to.to_owned())],
        );
        (
            value.as_ref().and_then(Value::as_bool).unwrap_or(false),
            error,
        )
    }

    /// Returns Luau's `fileInfo(path)` result pair.
    #[must_use]
    pub fn file_info_result(
        &self,
        path: &str,
    ) -> (Option<FileInfo>, Option<String>) {
        let (value, error) = self.query_pair(
            "noctalia",
            &["fileInfo"],
            vec![Value::String(path.to_owned())],
        );
        let info = value
            .as_ref()
            .and_then(|item| from_value(item.clone()).ok());
        (info, error)
    }

    /// Returns Luau's `listDir(path)` result pair.
    #[must_use]
    pub fn list_dir_result(
        &self,
        path: &str,
    ) -> (Option<Vec<String>>, Option<String>) {
        let (value, error) = self.query_pair(
            "noctalia",
            &["listDir"],
            vec![Value::String(path.to_owned())],
        );
        let entries = value.as_ref().and_then(Value::as_array).map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        });
        (entries, error)
    }

    /// Returns Luau's `loadFont(path)` result pair.
    #[must_use]
    pub fn load_font_result(
        &self,
        path: &str,
    ) -> (Option<String>, Option<String>) {
        let (value, error) = self.query_pair(
            "noctalia",
            &["loadFont"],
            vec![Value::String(path.to_owned())],
        );
        (
            value.as_ref().and_then(Value::as_str).map(str::to_owned),
            error,
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
pub struct FileInfo {
    pub size: u64,
    pub mtime: u64,
    pub is_dir: bool,
}

fn now_ms() -> i64 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
}

/// Parses a `+H` or `-H` offset that follows `UTC`.
fn parse_utc_offset(offset: &str) -> Option<chrono::FixedOffset> {
    let (sign, digits) = offset.split_at_checked(1)?;
    let hours: i32 = digits
        .bytes()
        .all(|byte| byte.is_ascii_digit())
        .then_some(digits)
        .and_then(|text| text.parse().ok())?;
    let seconds = match sign {
        "+" => hours.saturating_mul(3600),
        "-" => hours.saturating_neg().saturating_mul(3600),
        _ => return None,
    };
    chrono::FixedOffset::east_opt(seconds)
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
    use std::cell::RefCell;

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
    fn timer_methods_emit_start_and_cancel_ops() {
        let context = make_context();
        let timeout = context.set_timeout(1500, "finish");
        let interval = context.set_interval(250, "tick");
        context.cancel_timeout(timeout);
        context.cancel_interval(interval);

        let values = context
            .take_pending_ops()
            .iter()
            .map(|op| serde_json::to_value(op).expect("serialize op"))
            .collect::<Vec<_>>();
        assert_eq!(values.len(), 4);
        assert_eq!(values[0]["op"], "timer_start");
        assert_eq!(values[0]["ms"], 1500);
        assert_eq!(values[0]["recurring"], false);
        assert_eq!(
            values[0]["callback"],
            json!({ "__host_callback": "finish" })
        );
        assert_eq!(values[1]["op"], "timer_start");
        assert_eq!(values[1]["ms"], 250);
        assert_eq!(values[1]["recurring"], true);
        assert_ne!(values[0]["handle"], values[1]["handle"]);
        assert_eq!(values[2]["op"], "timer_cancel");
        assert_eq!(values[2]["handle"], values[0]["handle"]);
        assert_eq!(values[3]["op"], "timer_cancel");
        assert_eq!(values[3]["handle"], values[1]["handle"]);
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
    fn test_format_time_at_survives_a_malformed_timezone() {
        let context = make_context();

        for name in ["UTC\u{e9}", "UTC+", "UTC+ab", "UTC++2", "UT"] {
            let text = context.format_time_at("{:%H}", Some(0), Some(name));
            assert!(!text.is_empty(), "{name} produced no time");
        }
    }

    #[test]
    fn test_query_ids_do_not_repeat_across_contexts() {
        let mut first = make_context();
        let mut second = make_context();
        first.set_op_sink(Arc::new(record_response));
        second.set_op_sink(Arc::new(record_response));
        assert!(
            first
                .query_value("noctalia", &["outputs"], Vec::new())
                .is_none()
        );
        assert!(
            second
                .query_value("noctalia", &["outputs"], Vec::new())
                .is_none()
        );
        let seen = IDS.with(|ids| ids.borrow().clone());
        assert_eq!(seen.len(), 2, "expected two queries");
        assert_ne!(seen[0], seen[1], "query ids repeated");
    }

    fn record_response(op: &crate::Op) {
        let value = serde_json::to_value(op).expect("serialize");
        if let Some(response) = value.get("response").and_then(Value::as_str) {
            IDS.with(|ids| ids.borrow_mut().push(response.to_owned()));
        }
    }

    thread_local! {
        static IDS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    }

    #[test]
    fn test_format_time_accepts_noctalia_brace_syntax() {
        let context = make_context();

        let clock = context.format_time("{:%H:%M}");

        assert_eq!(clock.len(), 5);
        assert!(clock.contains(':'));
    }

    #[test]
    fn test_format_time_honors_seconds_and_timezone() {
        let context = make_context();

        assert_eq!(
            context.format_time_at("%Y-%m-%d", Some(0), Some("UTC")),
            "1970-01-01"
        );
        assert_eq!(
            context.format_time_at("%Y-%m-%d", Some(0), Some("UTC+2")),
            "1970-01-01"
        );
    }

    #[test]
    fn test_trp_uses_the_plural_form() {
        let parts = Parts {
            entry: "service".to_owned(),
            event: "update".to_owned(),
            payload: Value::Null,
            config: Value::Null,
            state: Value::Null,
            host: Value::Null,
            data_dir: PathBuf::from("/tmp/noctalia"),
        };
        let context = Context::new(
            parts,
            json!({
                "files": { "one": "{count} file", "other": "{count} files" }
            }),
        );

        assert_eq!(context.trp("files", 1, &[]), "1 file");
        assert_eq!(context.trp("files", 3, &[]), "3 files");
    }

    #[test]
    fn test_file_info_uses_host_names() {
        let context = make_context();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = env::temp_dir().join(format!("noctalia-info-{unique}"));
        fs::create_dir_all(&dir).expect("create");
        let file = dir.join("data.txt");
        fs::write(&file, "hello").expect("write");

        let info = context
            .file_info(file.to_str().expect("file"))
            .expect("info");

        assert_eq!(info.size, 5);
        assert!(!info.is_dir);
        assert!(info.mtime > 0);
        fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    #[expect(
        clippy::excessive_nesting,
        reason = "the op sink records a response and the emitted op in one place"
    )]
    fn test_async_handle_methods_emit_expected_host_work() {
        let mut context = make_context();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        context.set_op_sink(Arc::new(move |op| {
            let value = serde_json::to_value(op).unwrap_or(Value::Null);
            let body = if value.get("path") == Some(&json!(["httpStream"])) {
                r#"{"value":"stream-token"}"#
            } else {
                r#"{"value":true}"#
            };
            if let Some(response) =
                value.get("response").and_then(Value::as_str)
            {
                let _ = fs::write(response, body);
            }
            if let Ok(mut ops) = recorded.lock() {
                ops.push(value);
            }
        }));

        let timeout = i64::MAX;
        let run = context.run(
            json!(["echo"]),
            Some(&Callback::<Value>::named("done")),
            Some(timeout),
        );
        let http = context
            .http(json!({ "url": "x" }), &Callback::<Value>::named("response"));
        let stream = context.http_stream(
            json!({ "url": "y" }),
            &Callback::<Value>::named("line"),
            &Callback::<Value>::named("close"),
        );
        let watch = context
            .state_watch("wm.workspaces", &Callback::<Value>::named("changed"));

        assert!(run.started());
        assert!(http.started());
        assert!(stream.started());
        assert!(watch.started());

        let ops = lock(&seen);
        assert_eq!(ops.len(), 4);
        assert_eq!(
            ops.first().and_then(|op| op.get("path")),
            Some(&json!(["runAsync"]))
        );
        assert_eq!(
            ops.first().and_then(|op| op.get("args")),
            Some(&json!([
                ["echo"],
                { "__host_callback": "done" },
                timeout,
            ]))
        );
        assert_eq!(
            ops.get(1).and_then(|op| op.get("path")),
            Some(&json!(["http"]))
        );
        assert_eq!(
            ops.get(2).and_then(|op| op.get("args")),
            Some(&json!([
                { "url": "y" },
                { "__host_callback": "line" },
                { "__host_callback": "close" },
            ]))
        );
        assert_eq!(
            ops.get(3).and_then(|op| op.get("path")),
            Some(&json!(["state", "watch"]))
        );
    }
}
