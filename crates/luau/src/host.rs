use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock, RwLock, RwLockReadGuard, RwLockWriteGuard};

use noctalia_rs::{Context, Op};
use serde_json::Value;

const MAX_TRANSLATION_KEYS: usize = 5000;

#[derive(Default)]
pub struct Collector {
    ops: Mutex<Vec<Op>>,
    callbacks: AtomicUsize,
    functions: Mutex<HashMap<String, mlua::Function>>,
}

impl Collector {
    pub fn emit(&self, op: Op) {
        let mut ops = self.ops.lock().unwrap_or_else(|err| err.into_inner());
        ops.push(op);
    }

    pub fn callback_id(&self) -> String {
        let index = self.callbacks.fetch_add(1, Ordering::Relaxed) + 1;
        format!("noctalia_rs_luau_{index}")
    }

    pub fn register(&self, function: &mlua::Function) -> String {
        let id = self.callback_id();
        let mut functions = crate::locked(&self.functions);
        functions.insert(id.clone(), function.clone());
        id
    }

    pub fn function(&self, id: &str) -> Option<mlua::Function> {
        crate::locked(&self.functions).get(id).cloned()
    }

    pub fn take_ops(&self) -> Vec<Op> {
        let mut ops = self.ops.lock().unwrap_or_else(|err| err.into_inner());
        std::mem::take(&mut ops)
    }
}

struct SnapshotData {
    payload: Value,
    config: Value,
    state: Value,
    host: Value,
    data_dir: PathBuf,
    now_ms: i64,
}

impl SnapshotData {
    fn new(ctx: &Context) -> Self {
        Self {
            payload: ctx.payload.clone(),
            config: ctx.config.clone(),
            state: ctx.state.clone(),
            host: ctx.host.clone(),
            data_dir: ctx.data_dir.clone(),
            now_ms: ctx.now_ms,
        }
    }
}

pub struct Snapshot {
    inner: RwLock<SnapshotData>,
}

impl Snapshot {
    fn new(ctx: &Context) -> Self {
        Self {
            inner: RwLock::new(SnapshotData::new(ctx)),
        }
    }

    pub fn update(&self, ctx: &Context) {
        let mut data = write(&self.inner);
        *data = SnapshotData::new(ctx);
    }

    pub fn payload(&self) -> Value {
        read(&self.inner).payload.clone()
    }

    pub fn config(&self, key: &str) -> Value {
        read(&self.inner)
            .config
            .get(key)
            .cloned()
            .unwrap_or(Value::Null)
    }

    pub fn state(&self, key: &str) -> Value {
        read(&self.inner)
            .state
            .get(key)
            .cloned()
            .unwrap_or(Value::Null)
    }

    pub fn has_state(&self, key: &str) -> bool {
        read(&self.inner).state.get(key).is_some()
    }

    pub fn setting(&self, key: &str) -> Value {
        read(&self.inner)
            .host
            .get("setting")
            .and_then(|value| value.get(key))
            .cloned()
            .unwrap_or(Value::Null)
    }

    pub fn color(&self, role: &str) -> Value {
        read(&self.inner)
            .host
            .get("colors")
            .and_then(|value| value.get(role))
            .cloned()
            .unwrap_or(Value::Null)
    }

    pub fn host_path(&self, path: &str) -> Value {
        let data = read(&self.inner);
        let mut value = &data.host;
        for part in path.split('.') {
            let Some(next) = value.get(part) else {
                return Value::Null;
            };
            value = next;
        }
        value.clone()
    }

    pub fn now_ms(&self) -> i64 {
        read(&self.inner).now_ms
    }

    pub fn data_dir(&self) -> PathBuf {
        read(&self.inner).data_dir.clone()
    }
}

fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(|error| error.into_inner())
}

fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(|error| error.into_inner())
}

#[derive(Default)]
struct TranslationData {
    templates: HashMap<String, String>,
    keys: Vec<String>,
    raw: Value,
}

#[derive(Default)]
pub struct Translations {
    inner: Mutex<TranslationData>,
}

impl Translations {
    fn new(ctx: &Context, sources: &[(&str, &str)]) -> Self {
        let keys = translation_keys(sources);
        let templates = build_templates(ctx, &keys);
        Self {
            inner: Mutex::new(TranslationData {
                templates,
                keys,
                raw: ctx.translations().clone(),
            }),
        }
    }

    pub fn update(&self, ctx: &Context) {
        let mut data = crate::locked(&self.inner);
        if data.raw != *ctx.translations() {
            data.raw = ctx.translations().clone();
            data.templates = build_templates(ctx, &data.keys);
        }
    }

    pub fn render(&self, key: &str, params: &[(String, String)]) -> String {
        let data = crate::locked(&self.inner);
        let template = data.templates.get(key).map_or(key, String::as_str);
        let mut result = template.to_owned();
        for (name, value) in params {
            result = result.replace(&format!("{{{name}}}"), value);
        }
        result
    }
}

fn translation_keys(sources: &[(&str, &str)]) -> Vec<String> {
    let mut keys = HashSet::new();
    for (_, source) in sources {
        for key in string_literals(source) {
            if keys.len() >= MAX_TRANSLATION_KEYS {
                break;
            }
            keys.insert(key);
        }
    }
    keys.into_iter().collect()
}

fn build_templates(ctx: &Context, keys: &[String]) -> HashMap<String, String> {
    let mut templates = HashMap::new();
    for key in keys {
        let template = ctx.tr(key, &[]);
        if template != key.as_str() {
            templates.insert(key.clone(), template);
        }
    }
    templates
}

pub struct Host {
    pub snapshot: Snapshot,
    pub collector: Collector,
    translations: Translations,
    query_dir: OnceLock<PathBuf>,
    miss_checked_ms: AtomicU64,
}

impl Host {
    pub fn new(ctx: &Context, sources: &[(&str, &str)]) -> Self {
        Self {
            snapshot: Snapshot::new(ctx),
            collector: Collector::default(),
            translations: Translations::new(ctx, sources),
            query_dir: OnceLock::new(),
            miss_checked_ms: AtomicU64::new(f64::NEG_INFINITY.to_bits()),
        }
    }

    pub fn update(&self, ctx: &Context) {
        self.snapshot.update(ctx);
        self.translations.update(ctx);
    }

    pub fn tr(&self, key: &str, params: &[(String, String)]) -> String {
        self.translations.render(key, params)
    }

    /// Returns the spool directory when a host is attached.
    ///
    /// A hit is cached for the runtime's life. A miss is re-checked at most
    /// once a second, so a host that attaches late is still found without
    /// asking the filesystem on every host read.
    pub fn query_dir(&self) -> Option<&std::path::Path> {
        if let Some(dir) = self.query_dir.get() {
            return Some(dir);
        }
        let now = noctalia_rs::os_clock();
        let last = f64::from_bits(self.miss_checked_ms.load(Ordering::Relaxed));
        if now - last < 1.0 {
            return None;
        }
        self.miss_checked_ms.store(now.to_bits(), Ordering::Relaxed);
        let spool = self.snapshot.data_dir().join(".noctalia-rs").join("spool");
        spool
            .is_dir()
            .then(|| self.query_dir.get_or_init(|| spool).as_path())
    }

    pub fn response_path(&self, id: &str) -> PathBuf {
        self.snapshot
            .data_dir()
            .join(".noctalia-rs")
            .join("query")
            .join(format!("{id}.json"))
    }

    pub fn read_response(&self, path: &std::path::Path) -> Option<Value> {
        use std::thread;
        use std::time::{Duration, Instant};

        let deadline = Instant::now() + Duration::from_millis(100);
        loop {
            if let Ok(text) = fs::read_to_string(path) {
                let _ = fs::remove_file(path);
                return serde_json::from_str(&text).ok();
            }
            if Instant::now() >= deadline {
                return None;
            }
            thread::sleep(Duration::from_millis(1));
        }
    }
}

fn string_literals(source: &str) -> Vec<String> {
    let bytes = source.as_bytes();
    let mut result = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let quote = bytes[index];
        if quote != b'\'' && quote != b'"' && quote != b'`' {
            index += 1;
            continue;
        }
        index += 1;
        let mut text = String::new();
        while index < bytes.len() {
            let byte = bytes[index];
            if byte == quote {
                index += 1;
                break;
            }
            if byte == b'\\' && index + 1 < bytes.len() {
                let next = bytes[index + 1];
                match next {
                    b'n' => text.push('\n'),
                    b't' => text.push('\t'),
                    b'r' => text.push('\r'),
                    other => text.push(char::from(other)),
                }
                index += 2;
                continue;
            }
            let ch = source[index..].chars().next().unwrap_or('\0');
            text.push(ch);
            index += ch.len_utf8();
        }
        if !text.is_empty() && text.len() < 160 && !text.contains('\n') {
            result.push(text);
        }
    }
    result
}
