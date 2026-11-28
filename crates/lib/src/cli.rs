use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::env;
use std::fs;
use std::io::{self, Write};
use std::mem;
use std::path::{Path, PathBuf};
use std::process::{self, ExitCode};
use std::result::Result as StdResult;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use notify::event::EventKind;
use notify::{RecursiveMode, Watcher};
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::Result;
use crate::context::{Context, Parts};
use crate::manifest::Plugin;
use crate::ops::Op;

const POLL_INTERVAL: Duration = Duration::from_millis(1);
// Catch a request if the watcher misses an event.
const IDLE_WAIT: Duration = Duration::from_secs(1);

const NATIVE_CACHE_LIMIT: usize = 128;
// A request is written in place, so a read can catch a partial payload.
const REQUEST_READ_TIMEOUT: Duration = Duration::from_millis(2);
// An unparseable request older than this has no writer left to finish it.
const REQUEST_ABANDONED_AFTER: Duration = Duration::from_secs(2);
#[derive(Debug, Deserialize)]
struct Request {
    boot: String,
    entry: String,
    #[serde(default)]
    event: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    cache_ms: Option<u64>,
    #[serde(default)]
    context: Value,
    #[serde(default)]
    payload: Value,
    #[serde(default)]
    config: Value,
    #[serde(default)]
    state: Value,
    #[serde(default)]
    host: Value,
    #[serde(default)]
    host_fixed: Value,
    #[serde(default)]
    calls: Vec<NativeCall>,
}

#[derive(Debug, Deserialize)]
struct NativeCall {
    id: String,
    name: String,
    #[serde(default)]
    args: Value,
    #[serde(default)]
    cache_ms: u64,
    #[serde(default)]
    context: Value,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Processed {
    Entry,
    Native,
}

struct Serve<'a, W: Write> {
    plugin: &'a Plugin,
    data_dir: &'a Path,
    spool_dir: &'a Path,
    boot: &'a str,
    stale: BTreeSet<PathBuf>,
    native_cache: BTreeMap<String, (Instant, Value)>,
    native_cache_order: VecDeque<String>,
    host_fixed: Value,
    translations: &'a Value,
    out: &'a mut W,
}

impl<W: Write> Serve<'_, W> {
    fn run(&mut self) -> Result<()> {
        self.emit_ready()?;
        let watched = watch_spool(self.spool_dir);
        serve_loop(self, watched.as_ref())
    }

    fn emit_ready(&mut self) -> Result<()> {
        self.emit_json(&json!({
            "control": "ready",
            "boot": self.boot,
            "spool": self.spool_dir.to_string_lossy()
        }))
    }

    #[expect(
        clippy::excessive_nesting,
        reason = "a payload that is still being written ends the batch"
    )]
    fn process_batch(&mut self) -> Result<usize> {
        let mut processed = 0usize;
        for path in request_files(self.spool_dir)? {
            if self.process_one(&path)? {
                processed = processed.saturating_add(1);
            } else {
                // The next event for this name wakes the loop.
                return Ok(processed);
            }
        }
        Ok(processed)
    }

    /// Handles one request file. Returns false when the payload is incomplete.
    fn process_one(&mut self, path: &Path) -> Result<bool> {
        let name = request_name(path);
        if self.stale.contains(path) {
            self.emit_error(&name, "stale boot")?;
            let _ = fs::remove_file(path);
            return Ok(true);
        }

        let Some(request) = read_request(path)? else {
            return self.settle_unreadable(path, &name);
        };

        match self.process_request(path, request) {
            Ok(Processed::Entry) => self.emit_done(&name)?,
            Ok(Processed::Native) => {}
            Err(error) => self.emit_error(&name, &error.to_string())?,
        }
        let _ = fs::remove_file(path);
        Ok(true)
    }

    /// Decides what to do with a payload that did not parse. Returns false
    /// while its writer may still be running.
    fn settle_unreadable(&mut self, path: &Path, name: &str) -> Result<bool> {
        if !abandoned(path) {
            return Ok(false);
        }
        if path.exists() {
            self.emit_error(name, "request payload did not parse")?;
            let _ = fs::remove_file(path);
        }
        Ok(true)
    }

    fn process_request(
        &mut self,
        path: &Path,
        mut request: Request,
    ) -> Result<Processed> {
        if request.boot != self.boot {
            return Err(format!(
                "stale boot {} (expected {})",
                request.boot, self.boot
            )
            .into());
        }
        if request.kind.as_deref() == Some("native") {
            return self.process_native_request(request, path);
        }

        if request.host_fixed.is_object() {
            self.host_fixed = mem::take(&mut request.host_fixed);
        }
        let host = request_host(&self.host_fixed, request.host);
        let parts = Parts {
            entry: request.entry,
            event: request.event,
            payload: request.payload,
            config: request.config,
            state: request.state,
            host,
            data_dir: self.data_dir.to_path_buf(),
        };
        let mut context = Context::new(parts, self.translations.clone());
        let stdout = io::stdout();
        context.set_op_sink(Arc::new(move |op| {
            let text = serde_json::to_string(op).unwrap_or_default();
            let mut handle = stdout.lock();
            let _ = writeln!(handle, "{text}");
            let _ = handle.flush();
        }));
        for op in self.plugin.dispatch(&context)? {
            self.emit_op(&op)?;
        }
        Ok(Processed::Entry)
    }

    fn process_native(&mut self, request: Request, path: &Path) -> Result<()> {
        let id = request.id.clone().unwrap_or_else(|| request_name(path));
        let name = request.name.as_deref().unwrap_or_default();
        match self.native_value(
            &request.entry,
            name,
            request.args,
            request.cache_ms.unwrap_or(0),
            &request.context,
        ) {
            Ok(value) => self.emit_native_value(&id, &value),
            Err(error) => self.emit_native_error(&id, error.as_str()),
        }
    }

    fn process_native_request(
        &mut self,
        request: Request,
        path: &Path,
    ) -> Result<Processed> {
        if request.calls.is_empty() {
            self.process_native(request, path)?;
        } else {
            self.process_native_batch(&request)?;
        }
        Ok(Processed::Native)
    }

    fn process_native_batch(&mut self, request: &Request) -> Result<()> {
        let id = request.id.clone().unwrap_or_default();
        let mut results = Vec::with_capacity(request.calls.len());
        let mut failed = false;
        for call in &request.calls {
            let (entry, next) = self.native_batch_entry(request, call, failed);
            failed = next;
            results.push(entry);
        }
        self.emit_json(&json!({
            "control": "native_batch_result",
            "id": id,
            "results": results,
        }))
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one batch entry needs the request, call and failure state"
    )]
    fn native_batch_entry(
        &mut self,
        request: &Request,
        call: &NativeCall,
        failed: bool,
    ) -> (Value, bool) {
        if failed {
            return (
                json!({
                    "id": call.id,
                    "value": Value::Null,
                    "error": "skipped after an earlier native failure",
                }),
                true,
            );
        }
        match self.native_value(
            &request.entry,
            &call.name,
            call.args.clone(),
            call.cache_ms,
            &call.context,
        ) {
            Ok(value) => (
                json!({
                    "id": call.id,
                    "value": value,
                    "error": Value::Null,
                }),
                false,
            ),
            Err(error) => (
                json!({
                    "id": call.id,
                    "value": Value::Null,
                    "error": error,
                }),
                true,
            ),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "a native request carries the entry, name, args and cache policy"
    )]
    fn native_value(
        &mut self,
        entry: &str,
        name: &str,
        args: Value,
        cache_ms: u64,
        context_value: &Value,
    ) -> StdResult<Value, String> {
        if name.is_empty() {
            return Err("missing native function name".to_owned());
        }
        let context = self.native_context(entry, context_value);
        let key = if context.is_some() {
            format!(
                "{}\0{}",
                native_key(name, &args),
                canonical_json(&stable_context(context_value))
            )
        } else {
            native_key(name, &args)
        };
        let cached = if cache_ms > 0 {
            self.cached_native(&key, cache_ms)
        } else {
            None
        };
        let value = match cached {
            Some(value) => value,
            None => self.plugin.native(name, args, context.as_ref())?,
        };
        if cache_ms > 0 {
            self.remember_native(&key, value.clone());
        }
        Ok(value)
    }

    /// Rebuilds the invocation context a `&Context` native parameter sees.
    fn native_context(&self, entry: &str, context: &Value) -> Option<Context> {
        if !context.is_object() {
            return None;
        }
        let field =
            |key: &str| context.get(key).cloned().unwrap_or(Value::Null);
        Some(Context::new(
            Parts {
                entry: entry.to_owned(),
                event: field("event")
                    .as_str()
                    .map_or_else(String::new, str::to_owned),
                payload: field("payload"),
                config: field("config"),
                state: field("state"),
                host: field("host"),
                data_dir: self.data_dir.to_path_buf(),
            },
            self.translations.clone(),
        ))
    }

    fn cached_native(&self, key: &str, cache_ms: u64) -> Option<Value> {
        let (stored_at, value) = self.native_cache.get(key)?;
        (stored_at.elapsed() < Duration::from_millis(cache_ms))
            .then(|| value.clone())
    }

    fn remember_native(&mut self, key: &str, value: Value) {
        self.native_cache_order.retain(|item| item != key);
        self.native_cache_order.push_back(key.to_owned());
        self.native_cache
            .insert(key.to_owned(), (Instant::now(), value));
        if self.native_cache_order.len() > NATIVE_CACHE_LIMIT {
            self.evict_oldest_native();
        }
    }

    fn evict_oldest_native(&mut self) {
        let oldest = self.native_cache_order.pop_front();
        if let Some(oldest_key) = oldest {
            self.native_cache.remove(&oldest_key);
        }
    }

    fn emit_native_value(&mut self, id: &str, value: &Value) -> Result<()> {
        self.emit_json(&json!({
            "control": "native_result",
            "id": id,
            "value": value,
            "error": Value::Null,
        }))
    }

    fn emit_native_error(&mut self, id: &str, error: &str) -> Result<()> {
        self.emit_json(&json!({
            "control": "native_result",
            "id": id,
            "value": Value::Null,
            "error": error,
        }))
    }

    fn emit_json(&mut self, value: &Value) -> Result<()> {
        writeln!(self.out, "{}", serde_json::to_string(value)?)?;
        self.out.flush()?;
        Ok(())
    }

    fn emit_op(&mut self, op: &Op) -> Result<()> {
        writeln!(self.out, "{}", serde_json::to_string(op)?)?;
        self.out.flush()?;
        Ok(())
    }

    fn emit_done(&mut self, request: &str) -> Result<()> {
        self.emit_json(&json!({ "control": "done", "request": request }))
    }

    fn emit_error(&mut self, request: &str, message: &str) -> Result<()> {
        self.emit_json(
            &json!({ "control": "error", "request": request, "message": message }),
        )
    }
}

fn serve_loop<W: Write>(
    server: &mut Serve<'_, W>,
    watched: Option<&(Receiver<()>, notify::RecommendedWatcher)>,
) -> Result<()> {
    loop {
        while server.process_batch()? > 0 {}
        wait_for_work(watched);
    }
}

fn wait_for_work(watched: Option<&(Receiver<()>, notify::RecommendedWatcher)>) {
    let Some((receiver, _watcher)) = watched else {
        thread::sleep(POLL_INTERVAL);
        return;
    };
    match receiver.recv_timeout(IDLE_WAIT) {
        Ok(()) | Err(RecvTimeoutError::Timeout) => {}
        Err(RecvTimeoutError::Disconnected) => thread::sleep(POLL_INTERVAL),
    }
}

/// Wakes the serve loop only for work it can read.
///
/// Reading the spool emits access events and deleting a handled request emits
/// a removal. Waking for those costs a full wakeup cycle per request and never
/// finds work, so only a request name that was created, modified, or reported
/// as `Any` wakes the loop. Events without a path still wake it: that is the
/// rescan notify emits when it lost events.
fn wakes_serve(event: &notify::Event) -> bool {
    if event.paths.is_empty() {
        return true;
    }
    let request = matches!(
        event.kind,
        EventKind::Any | EventKind::Create(_) | EventKind::Modify(_)
    );
    request && event.paths.iter().any(|path| is_request_name(path))
}

fn watch_spool(
    spool_dir: &Path,
) -> Option<(Receiver<()>, notify::RecommendedWatcher)> {
    let (sender, receiver) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(
        move |event: notify::Result<notify::Event>| {
            let relevant = event.is_ok_and(|change| wakes_serve(&change));
            if relevant {
                let _ = sender.send(());
            }
        },
    )
    .ok()?;
    watcher.watch(spool_dir, RecursiveMode::NonRecursive).ok()?;
    Some((receiver, watcher))
}

#[must_use = "the exit code must be returned to the caller"]
pub fn run(plugin: &Plugin) -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    match execute(plugin, &args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("noctalia: {error}");
            ExitCode::FAILURE
        }
    }
}

fn execute(plugin: &Plugin, args: &[String]) -> Result<()> {
    if args.first().is_some_and(|arg| arg == "serve") {
        let usage = "usage: serve <data-dir> <spool-dir>";
        let data_dir = PathBuf::from(args.get(1).ok_or(usage)?);
        let spool_dir = PathBuf::from(args.get(2).ok_or(usage)?);
        return serve(plugin, &data_dir, &spool_dir);
    }

    if args.first().is_some_and(|arg| arg == "emit") {
        let dir =
            PathBuf::from(args.get(1).ok_or("usage: emit <dir> <helper>")?);
        let helper = args.get(2).ok_or("usage: emit <dir> <helper>")?;
        plugin.emit(&dir, helper)?;
        return Ok(());
    }

    let parts = Parts {
        entry: args.first().cloned().ok_or("missing entry")?,
        event: args.get(1).cloned().ok_or("missing event")?,
        payload: decode(args.get(3)),
        config: decode(args.get(4)),
        state: decode(args.get(5)),
        host: decode(args.get(6)),
        data_dir: PathBuf::from(args.get(2).cloned().unwrap_or_default()),
    };

    let context = Context::new(parts, plugin.translations_value());

    for op in plugin.dispatch(&context)? {
        println!("{}", serde_json::to_string(&op)?);
    }

    Ok(())
}

fn decode(raw: Option<&String>) -> Value {
    raw.map_or_else(
        || Value::Null,
        |value| serde_json::from_str(value).unwrap_or_default(),
    )
}

fn serve(plugin: &Plugin, data_dir: &Path, spool_dir: &Path) -> Result<()> {
    fs::create_dir_all(data_dir)?;
    fs::create_dir_all(spool_dir)?;
    let stdout = io::stdout();
    let mut out = stdout;
    let boot = boot_id();
    let run_dir = spool_dir.join(&boot);
    fs::create_dir_all(&run_dir)?;
    prune_query_dir(data_dir);
    let stale = request_files(&run_dir)?;
    let translations = plugin.translations_value();
    let mut server = Serve {
        plugin,
        data_dir,
        spool_dir: &run_dir,
        boot: &boot,
        stale,
        native_cache: BTreeMap::new(),
        native_cache_order: VecDeque::new(),
        host_fixed: Value::Object(Map::new()),
        translations: &translations,
        out: &mut out,
    };
    server.run()
}

fn prune_query_dir(data_dir: &Path) {
    let dir = data_dir.join(".noctalia-rs").join("query");
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let _ = fs::remove_file(entry.path());
    }
}

fn boot_id() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}-{:?}", process::id(), now)
}

fn request_files(spool_dir: &Path) -> Result<BTreeSet<PathBuf>> {
    let mut files = BTreeSet::new();
    for item in fs::read_dir(spool_dir)? {
        let path = item?.path();
        if is_request(&path) {
            files.insert(path);
        }
    }
    Ok(files)
}

fn is_request(path: &Path) -> bool {
    is_request_name(path) && path.is_file()
}

fn is_request_name(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

/// Builds the host snapshot a request carries.
///
/// The bridge sends the per-event values in `host` and the fixed values
/// (settings, colors, paths) in `host_fixed` only when they changed, so the
/// cached fixed part is merged under the live one. A request that sends one
/// complete `host` overrides every cached key.
fn request_host(fixed: &Value, live: Value) -> Value {
    let Some(mut host) = fixed.as_object().cloned() else {
        return live;
    };
    if let Some(values) = live.as_object() {
        host.extend(
            values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    Value::Object(host)
}

/// Reads a request, retrying while the writer is still filling it.
///
/// The bridge writes a request in place, so a wake can land mid-write. A
/// partial payload never parses as JSON, and bytes cut inside a multi-byte
/// character do not even decode as text, so any parse failure is retried until
/// the deadline. Returns None when the payload is still incomplete.
fn read_request(path: &Path) -> Result<Option<Request>> {
    read_request_within(path, REQUEST_READ_TIMEOUT)
}

fn read_request_within(
    path: &Path,
    timeout: Duration,
) -> Result<Option<Request>> {
    let deadline = Instant::now().checked_add(timeout);
    loop {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        match serde_json::from_slice(&bytes) {
            Ok(request) => return Ok(Some(request)),
            Err(_) if deadline.is_some_and(|end| Instant::now() < end) => {
                thread::yield_now();
            }
            Err(_) => return Ok(None),
        }
    }
}

/// A request file that stopped changing and still does not parse will never
/// parse: its writer is gone. A missing file counts as abandoned.
fn abandoned(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return true;
    };
    meta.modified()
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_none_or(|age| age > REQUEST_ABANDONED_AFTER)
}

fn request_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "unknown".to_owned(), str::to_owned)
}

/// Drops reads that change on every request, so a cached context call can hit.
fn stable_context(context: &Value) -> Value {
    let mut stable = context.clone();
    if let Some(host) = stable.get_mut("host").and_then(Value::as_object_mut) {
        host.remove("now_ms");
    }
    stable
}

fn native_key(name: &str, args: &Value) -> String {
    format!("{name}\0{}", canonical_json(args))
}

fn canonical_json(value: &Value) -> String {
    serde_json::to_string(&canonical_value(value)).unwrap_or_default()
}

fn canonical_value(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map
                .iter()
                .map(|(key, item)| (key.clone(), canonical_value(item)))
                .collect();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            Value::Object(entries.into_iter().collect())
        }
        Value::Array(items) => {
            Value::Array(items.iter().map(canonical_value).collect())
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            value.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Meta, Op};
    use std::process;
    use std::result::Result as StdResult;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use notify::event::{
        AccessKind, CreateKind, ModifyKind, RemoveKind, RenameMode,
    };

    fn event(kind: EventKind, paths: &[&str]) -> notify::Event {
        let mut event = notify::Event::new(kind);
        for path in paths {
            event = event.add_path(PathBuf::from(path));
        }
        event
    }

    #[test]
    fn test_wakes_serve_only_for_new_request_files() {
        let create = EventKind::Create(CreateKind::File);
        let modify = EventKind::Modify(ModifyKind::Name(RenameMode::To));
        let remove = EventKind::Remove(RemoveKind::File);
        let access = EventKind::Access(AccessKind::Read);

        assert!(wakes_serve(&event(create, &["/spool/000000000001.json"])));
        assert!(wakes_serve(&event(modify, &["/spool/000000000001.json"])));
        assert!(!wakes_serve(&event(remove, &["/spool/000000000001.json"])));
        assert!(!wakes_serve(&event(access, &["/spool/000000000001.json"])));
        assert!(!wakes_serve(&event(
            create,
            &["/spool/000000000001.pending"]
        )));
        assert!(!wakes_serve(&event(modify, &["/spool"])));
        assert!(wakes_serve(&event(EventKind::Any, &["/spool/1.json"])));
        assert!(!wakes_serve(&event(EventKind::Any, &["/spool"])));
        assert!(wakes_serve(&event(EventKind::Other, &[])));
    }

    fn finish_later(path: &Path, tail: &[u8]) -> thread::JoinHandle<()> {
        let writer = path.to_path_buf();
        let rest = tail.to_vec();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1));
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(writer)
                .expect("open request");
            file.write_all(&rest).expect("finish request");
        })
    }

    #[test]
    fn test_read_request_waits_for_an_in_place_write() {
        let spool = TestSpool::new("read-partial");
        let path = spool.spool.join("000000000001.json");
        let text = request("boot-1", "render").to_string();
        let split = text.len().div_euclid(2);
        fs::write(&path, &text[..split]).expect("partial write");
        let writer = finish_later(&path, &text.as_bytes()[split..]);

        let read = read_request_within(&path, Duration::from_secs(5))
            .expect("retried request")
            .expect("finished request");
        writer.join().expect("writer");
        spool.cleanup();

        assert_eq!(read.boot, "boot-1");
        assert_eq!(read.event, "render");
    }

    #[test]
    fn test_read_request_retries_a_character_cut_in_half() {
        let spool = TestSpool::new("read-utf8");
        let path = spool.spool.join("000000000001.json");
        let text = request("boot-1", "r\u{e9}nder").to_string();
        let cut = text.find('\u{e9}').expect("multi-byte event") + 1;
        fs::write(&path, &text.as_bytes()[..cut]).expect("partial write");
        let writer = finish_later(&path, &text.as_bytes()[cut..]);

        let read = read_request_within(&path, Duration::from_secs(5))
            .expect("retried request")
            .expect("finished request");
        writer.join().expect("writer");
        spool.cleanup();

        assert_eq!(read.event, "r\u{e9}nder");
    }

    #[test]
    fn test_serve_keeps_a_request_that_is_still_being_written() {
        let spool = TestSpool::new("serve-defer");
        let path = spool.spool.join("000000000001.json");
        let text = request("boot-1", "render").to_string();
        fs::write(&path, &text[..text.len().div_euclid(2)]).expect("partial");
        let plugin = plugin();
        let translations = Value::Null;
        let mut out: Vec<u8> = Vec::new();
        let processed = {
            let mut server = Serve {
                plugin: &plugin,
                data_dir: &spool.data,
                spool_dir: &spool.spool,
                boot: "boot-1",
                stale: BTreeSet::new(),
                native_cache: BTreeMap::new(),
                native_cache_order: VecDeque::new(),
                host_fixed: Value::Object(Map::new()),
                translations: &translations,
                out: &mut out,
            };
            server.process_batch().expect("batch")
        };
        let lines = String::from_utf8_lossy(&out).to_string();
        let kept = path.exists();
        spool.cleanup();

        assert_eq!(processed, 0, "{lines}");
        assert!(kept, "a partial request must stay for the next event");
        assert!(!lines.contains("\"error\""), "{lines}");
    }

    fn plugin() -> Plugin {
        fn handler(ctx: &Context) -> crate::Result<Vec<Op>> {
            let _ = serde_json::to_value(&ctx.entry)?;
            Ok(vec![Op::log(ctx.event.clone())])
        }

        Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .tr("notify.title", "Hello")
            .service("service", handler)
    }

    fn state_plugin() -> Plugin {
        fn handler(ctx: &Context) -> crate::Result<Vec<Op>> {
            let _ = serde_json::to_value(&ctx.entry)?;
            Ok(vec![Op::state(
                "served",
                json!({ "nested": [1, true, "x"] }),
            )])
        }

        Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .service("service", handler)
    }

    fn request(boot: &str, event: &str) -> Value {
        json!({
            "boot": boot,
            "entry": "service",
            "event": event,
            "payload": {},
            "config": {},
            "state": {},
            "host": {},
        })
    }

    #[expect(clippy::needless_pass_by_value, clippy::too_many_arguments)]
    fn native_request(
        boot: &str,
        name: &str,
        cache_ms: u64,
        id: &str,
        args: Value,
    ) -> Value {
        json!({
            "boot": boot,
            "kind": "native",
            "entry": "service",
            "id": id,
            "name": name,
            "args": args,
            "cache_ms": cache_ms,
        })
    }
    struct TestSpool {
        data: PathBuf,
        spool: PathBuf,
    }

    impl TestSpool {
        fn new(name: &str) -> Self {
            let root = env::temp_dir()
                .join(format!("noctalia-{name}-{}", process::id()));
            let data = root.join("data");
            let spool = root.join("spool");
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&spool).expect("spool dir");
            Self { data, spool }
        }

        fn write(&self, name: &str, request: &Value) {
            fs::write(self.spool.join(name), request.to_string())
                .expect("request");
        }

        fn cleanup(&self) {
            let root = self.spool.parent().expect("spool parent");
            fs::remove_dir_all(root).expect("cleanup");
        }
    }

    fn complete_lines(buffer: &[u8]) -> Vec<String> {
        let text = String::from_utf8_lossy(buffer);
        let head = text.rsplit_once('\n').map_or("", |(head, _)| head);
        head.split('\n')
            .filter(|part| !part.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn guard_write(buffer: &[u8], stop_after: usize) -> io::Result<()> {
        if complete_lines(buffer).len() >= stop_after {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "capture full",
            ));
        }
        Ok(())
    }

    struct CaptureWriter {
        buffer: Vec<u8>,
        flushes: usize,
        stop_after: usize,
    }

    impl CaptureWriter {
        fn new(stop_after: usize) -> Self {
            Self {
                buffer: Vec::new(),
                flushes: 0,
                stop_after,
            }
        }

        fn lines(&self) -> Vec<String> {
            complete_lines(&self.buffer)
        }
    }

    impl Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            guard_write(&self.buffer, self.stop_after)?;
            self.buffer.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.flushes = self.flushes.saturating_add(1);
            guard_write(&self.buffer, self.stop_after)
        }
    }

    struct Capture<'a> {
        plugin: &'a Plugin,
        data: &'a Path,
        spool: &'a Path,
        boot: &'a str,
        stale: BTreeSet<PathBuf>,
        stop_after: usize,
    }

    fn run_capture(
        config: Capture<'_>,
        writer: &mut CaptureWriter,
    ) -> Result<()> {
        let mut server = Serve {
            plugin: config.plugin,
            data_dir: config.data,
            spool_dir: config.spool,
            boot: config.boot,
            stale: config.stale,
            native_cache: BTreeMap::new(),
            native_cache_order: VecDeque::new(),
            host_fixed: Value::Object(Map::new()),
            translations: &Value::Null,
            out: writer,
        };
        server.run()
    }

    impl Capture<'_> {
        fn run(self) -> CaptureWriter {
            let mut writer = CaptureWriter::new(self.stop_after);
            let result = run_capture(self, &mut writer);
            assert!(result.is_err(), "capture writer must stop serve");
            writer
        }
    }

    #[test]
    fn test_decode_handles_missing_invalid_and_valid_json() {
        let valid = "{\"key\":1}".to_owned();
        let invalid = "not json".to_owned();

        assert_eq!(decode(None), Value::Null);
        assert_eq!(decode(Some(&invalid)), Value::Null);
        assert_eq!(decode(Some(&valid)), serde_json::json!({ "key": 1 }));
    }

    #[test]
    fn test_execute_emits_plugin_files() {
        let dir =
            env::temp_dir().join(format!("noctalia-emit-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        let path = dir.to_string_lossy().into_owned();
        let args = vec!["emit".to_owned(), path, "hello-helper".to_owned()];

        let result = execute(&plugin(), &args);

        assert!(result.is_ok());
        assert!(dir.join("plugin.toml").exists());
        assert!(dir.join("lib/bridge.luau").exists());
        assert!(dir.join("translations/en.json").exists());
        assert!(dir.join("service.luau").exists());
        fs::remove_dir_all(dir).expect("cleanup");
    }

    #[test]
    fn test_execute_reports_missing_arguments() {
        let args: Vec<String> = Vec::new();

        let error = execute(&plugin(), &args).expect_err("missing entry");

        assert_eq!(error.to_string(), "missing entry");
    }

    #[test]
    fn test_execute_dispatches_entry() {
        let args = vec![
            "service".to_owned(),
            "update".to_owned(),
            "/tmp".to_owned(),
            "{}".to_owned(),
            "{}".to_owned(),
            "{}".to_owned(),
            "{}".to_owned(),
        ];

        let result = execute(&plugin(), &args);

        assert!(result.is_ok());
    }

    #[test]
    fn test_serve_processes_requests_in_filename_order() {
        let spool = TestSpool::new("serve-order");
        for (name, event) in [
            ("000000000003.json", "third"),
            ("000000000001.json", "first"),
            ("000000000002.json", "second"),
        ] {
            spool.write(name, &request("boot-1", event));
        }
        let plugin = plugin();

        let writer = Capture {
            plugin: &plugin,
            data: &spool.data,
            spool: &spool.spool,
            boot: "boot-1",
            stale: BTreeSet::new(),
            stop_after: 6,
        }
        .run();
        let lines = writer.lines();
        let ops: Vec<&String> = lines
            .iter()
            .filter(|line| line.contains("\"op\""))
            .collect();

        assert_eq!(ops.len(), 3);
        assert!(ops.first().is_some_and(|line| line.contains("first")));
        assert!(ops.get(1).is_some_and(|line| line.contains("second")));
        assert!(ops.get(2).is_some_and(|line| line.contains("third")));

        spool.cleanup();
    }

    #[test]
    fn test_serve_merges_the_fixed_host_snapshot() {
        #[expect(clippy::unnecessary_wraps, reason = "handlers return Result")]
        fn handler(ctx: &Context) -> crate::Result<Vec<Op>> {
            let value =
                |path: &str| ctx.host(path).cloned().unwrap_or(Value::Null);
            Ok(vec![Op::log(format!(
                "{}|{}|{}",
                ctx.setting("shell.font")
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .unwrap_or_default(),
                value("colors.primary").as_str().unwrap_or(""),
                value("now_ms").as_i64().unwrap_or_default()
            ))])
        }

        let spool = TestSpool::new("serve-host-fixed");
        let boot = "boot-host";
        spool.write(
            "000000000001.json",
            &json!({
                "boot": boot,
                "entry": "service",
                "event": "render",
                "host": { "now_ms": 1 },
                "host_fixed": {
                    "setting": { "shell.font": "sans" },
                    "colors": { "primary": "#fff" },
                },
            }),
        );
        spool.write(
            "000000000002.json",
            &json!({
                "boot": boot,
                "entry": "service",
                "event": "render",
                "host": { "now_ms": 2, "clipboard": "clip" },
            }),
        );
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .service("service", handler);

        let writer = Capture {
            plugin: &plugin,
            data: &spool.data,
            spool: &spool.spool,
            boot,
            stale: BTreeSet::new(),
            stop_after: 4,
        }
        .run();
        let lines = writer.lines();
        spool.cleanup();

        let logs: Vec<&String> = lines
            .iter()
            .filter(|line| line.contains("\"log\""))
            .collect();
        assert_eq!(logs.len(), 2, "{lines:?}");
        assert!(logs[0].contains("sans|#fff|1"), "{}", logs[0]);
        assert!(logs[1].contains("sans|#fff|2"), "{}", logs[1]);
    }

    #[test]
    fn test_serve_wakes_on_new_request() {
        let spool = TestSpool::new("serve-wake");
        let boot = "boot-wake".to_owned();
        let path = spool.spool.join("000000000001.json");
        let body = request(&boot, "wake").to_string();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            let temporary = path.with_extension("tmp");
            fs::write(&temporary, body).expect("request");
            fs::rename(temporary, path).expect("publish");
        });
        let plugin = plugin();
        let start = Instant::now();
        let capture = Capture {
            plugin: &plugin,
            data: &spool.data,
            spool: &spool.spool,
            boot: &boot,
            stale: BTreeSet::new(),
            stop_after: 2,
        }
        .run();
        let elapsed = start.elapsed();
        writer.join().expect("writer");

        let lines = capture.lines();
        assert!(lines.iter().any(|line| line.contains("wake")));
        // The safety net only answers after IDLE_WAIT, so a quick answer
        // means the watcher woke the loop.
        assert!(
            elapsed < Duration::from_millis(750),
            "the safety net answered after {elapsed:?}"
        );

        spool.cleanup();
    }

    #[test]
    fn test_serve_rejects_stale_boot_requests() {
        let spool = TestSpool::new("serve-stale");
        spool.write("000000000001.json", &request("old-boot", "old"));
        spool.write("000000000002.json", &request("boot-2", "fresh"));
        let plugin = plugin();

        let writer = Capture {
            plugin: &plugin,
            data: &spool.data,
            spool: &spool.spool,
            boot: "boot-2",
            stale: BTreeSet::new(),
            stop_after: 3,
        }
        .run();
        let lines = writer.lines();
        let stale = lines
            .iter()
            .find(|line| line.contains("\"control\":\"error\""))
            .expect("stale error line");

        assert!(stale.contains("stale boot"));
        assert!(stale.contains("old-boot"));
        assert!(lines.iter().any(|line| line.contains("fresh")));
        assert!(!lines.iter().any(|line| line.contains("\"old\"")));

        spool.cleanup();
    }

    #[test]
    fn test_serve_writes_raw_op_lines() {
        let spool = TestSpool::new("serve-raw");
        spool.write("000000000001.json", &request("boot-3", "render"));
        let plugin = state_plugin();
        let expected = serde_json::to_string(&Op::state(
            "served",
            json!({ "nested": [1, true, "x"] }),
        ))
        .expect("op");

        let writer = Capture {
            plugin: &plugin,
            data: &spool.data,
            spool: &spool.spool,
            boot: "boot-3",
            stale: BTreeSet::new(),
            stop_after: 2,
        }
        .run();
        let lines = writer.lines();

        assert_eq!(lines.get(1).map(String::as_str), Some(expected.as_str()));

        spool.cleanup();
    }

    #[test]
    fn test_serve_flushes_every_line() {
        let spool = TestSpool::new("serve-flush");
        spool.write("000000000001.json", &request("boot-4", "update"));
        let plugin = plugin();

        let writer = Capture {
            plugin: &plugin,
            data: &spool.data,
            spool: &spool.spool,
            boot: "boot-4",
            stale: BTreeSet::new(),
            stop_after: 2,
        }
        .run();
        let lines = writer.lines();

        assert_eq!(lines.len(), 2);
        assert_eq!(writer.flushes, lines.len());

        spool.cleanup();
    }

    #[test]
    fn test_serve_caches_native_results_without_done() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        #[expect(clippy::unnecessary_wraps)]
        fn dispatch(
            _name: &str,
            _args: Value,
            _context: Option<&crate::Context>,
        ) -> StdResult<Value, String> {
            CALLS.fetch_add(1, Ordering::Relaxed);
            Ok(json!({ "rows": 1 }))
        }

        CALLS.store(0, Ordering::Relaxed);
        let spool = TestSpool::new("serve-native-cache");
        let args = json!({ "b": 2, "a": 1 });
        spool.write(
            "000000000001.json",
            &native_request(
                "boot-native-cache",
                "rows",
                1000,
                "first",
                args.clone(),
            ),
        );
        spool.write(
            "000000000002.json",
            &native_request(
                "boot-native-cache",
                "rows",
                1000,
                "second",
                args.clone(),
            ),
        );
        spool.write(
            "000000000003.json",
            &native_request("boot-native-cache", "rows", 0, "third", args),
        );
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .with_native(dispatch);

        let writer = Capture {
            plugin: &plugin,
            data: &spool.data,
            spool: &spool.spool,
            boot: "boot-native-cache",
            stale: BTreeSet::new(),
            stop_after: 4,
        }
        .run();
        let lines = writer.lines();

        assert_eq!(lines.len(), 4);
        for (line, id) in lines.iter().skip(1).zip(["first", "second", "third"])
        {
            let value: Value = serde_json::from_str(line).expect("native line");
            assert_eq!(
                value,
                json!({
                    "control": "native_result",
                    "id": id,
                    "value": { "rows": 1 },
                    "error": null,
                })
            );
        }
        assert!(!lines.iter().any(|line| line.contains("\"done\"")));
        assert_eq!(CALLS.load(Ordering::Relaxed), 2);
        spool.cleanup();
    }

    #[test]
    fn test_serve_caches_canonical_args() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        #[expect(clippy::unnecessary_wraps)]
        fn dispatch(
            _name: &str,
            _args: Value,
            _context: Option<&crate::Context>,
        ) -> StdResult<Value, String> {
            CALLS.fetch_add(1, Ordering::Relaxed);
            Ok(json!({ "ok": true }))
        }

        CALLS.store(0, Ordering::Relaxed);
        let spool = TestSpool::new("serve-native-canonical");
        let nested = json!({ "y": 2, "x": 1 });
        let reordered = json!({ "x": 1, "y": 2 });
        spool.write(
            "000000000001.json",
            &native_request(
                "boot-native-canonical",
                "decode",
                1000,
                "one",
                json!({ "outer": nested, "list": [reordered] }),
            ),
        );
        spool.write(
            "000000000002.json",
            &native_request(
                "boot-native-canonical",
                "decode",
                1000,
                "two",
                json!({ "list": [json!({ "y": 2, "x": 1 })], "outer": json!({ "x": 1, "y": 2 }) }),
            ),
        );
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .with_native(dispatch);

        let writer = Capture {
            plugin: &plugin,
            data: &spool.data,
            spool: &spool.spool,
            boot: "boot-native-canonical",
            stale: BTreeSet::new(),
            stop_after: 3,
        }
        .run();
        let lines = writer.lines();

        assert_eq!(lines.len(), 3);
        assert_eq!(CALLS.load(Ordering::Relaxed), 1);
        assert_eq!(
            canonical_json(&json!({ "b": { "d": 1, "c": 2 }, "a": 3 })),
            "{\"a\":3,\"b\":{\"c\":2,\"d\":1}}"
        );
        spool.cleanup();
    }

    #[test]
    fn test_serve_reports_native_dispatch_error_as_result() {
        fn dispatch(
            _name: &str,
            _args: Value,
            _context: Option<&crate::Context>,
        ) -> StdResult<Value, String> {
            Err("native exploded".to_owned())
        }

        let spool = TestSpool::new("serve-native-error");
        spool.write(
            "000000000001.json",
            &native_request(
                "boot-native-error",
                "fails",
                0,
                "failed",
                json!({}),
            ),
        );
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .with_native(dispatch);

        let writer = Capture {
            plugin: &plugin,
            data: &spool.data,
            spool: &spool.spool,
            boot: "boot-native-error",
            stale: BTreeSet::new(),
            stop_after: 2,
        }
        .run();
        let lines = writer.lines();

        assert_eq!(lines.len(), 2);
        let value: Value =
            serde_json::from_str(&lines[1]).expect("native line");
        assert_eq!(
            value,
            json!({
                "control": "native_result",
                "id": "failed",
                "value": null,
                "error": "native exploded",
            })
        );
        assert!(!lines.iter().any(|line| line.contains("\"done\"")));
        spool.cleanup();
    }

    #[test]
    fn test_serve_reports_unknown_and_malformed_native_requests() {
        let spool = TestSpool::new("serve-native-unknown");
        spool.write(
            "000000000001.json",
            &native_request(
                "boot-native-unknown",
                "missing",
                0,
                "unknown",
                json!({}),
            ),
        );
        spool.write(
            "000000000002.json",
            &json!({
                "boot": "boot-native-unknown",
                "kind": "native",
                "entry": "service",
                "args": {},
                "cache_ms": 0,
            }),
        );
        let plugin = plugin();

        let writer = Capture {
            plugin: &plugin,
            data: &spool.data,
            spool: &spool.spool,
            boot: "boot-native-unknown",
            stale: BTreeSet::new(),
            stop_after: 3,
        }
        .run();
        let lines = writer.lines();

        assert_eq!(lines.len(), 3);
        let unknown: Value =
            serde_json::from_str(&lines[1]).expect("unknown line");
        assert_eq!(
            unknown,
            json!({
                "control": "native_result",
                "id": "unknown",
                "value": null,
                "error": "unknown native function `missing`",
            })
        );
        let malformed: Value =
            serde_json::from_str(&lines[2]).expect("malformed line");
        assert_eq!(
            malformed,
            json!({
                "control": "native_result",
                "id": "000000000002.json",
                "value": null,
                "error": "missing native function name",
            })
        );
        spool.cleanup();
    }

    #[test]
    fn test_serve_keeps_stale_boot_handling_for_native_requests() {
        #[expect(clippy::unnecessary_wraps)]
        fn dispatch(
            _name: &str,
            _args: Value,
            _context: Option<&crate::Context>,
        ) -> StdResult<Value, String> {
            Ok(json!({ "rows": 1 }))
        }

        let spool = TestSpool::new("serve-native-stale");
        spool.write(
            "000000000001.json",
            &native_request("old-boot", "rows", 0, "old", json!({})),
        );
        spool.write(
            "000000000002.json",
            &native_request("boot-native-stale", "rows", 0, "fresh", json!({})),
        );
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .with_native(dispatch);

        let writer = Capture {
            plugin: &plugin,
            data: &spool.data,
            spool: &spool.spool,
            boot: "boot-native-stale",
            stale: BTreeSet::new(),
            stop_after: 3,
        }
        .run();
        let lines = writer.lines();

        assert_eq!(lines.len(), 3);
        assert!(lines[1].contains("\"control\":\"error\""));
        assert!(lines[1].contains("stale boot"));
        let fresh: Value = serde_json::from_str(&lines[2]).expect("fresh line");
        assert_eq!(fresh["control"], "native_result");
        assert_eq!(fresh["id"], "fresh");
        spool.cleanup();
    }

    #[test]
    fn test_native_cache_evicts_oldest_inserted_entry() {
        let mut out: Vec<u8> = Vec::new();
        let plugin = plugin();
        let translations = Value::Null;
        let mut server = Serve {
            plugin: &plugin,
            data_dir: Path::new("/tmp"),
            spool_dir: Path::new("/tmp"),
            boot: "boot",
            stale: BTreeSet::new(),
            native_cache: BTreeMap::new(),
            native_cache_order: VecDeque::new(),
            host_fixed: Value::Object(Map::new()),
            translations: &translations,
            out: &mut out,
        };

        for index in 0..NATIVE_CACHE_LIMIT {
            server.remember_native(&format!("name-{index}"), json!(index));
        }
        server.remember_native("latest", json!(128));

        assert!(!server.native_cache.contains_key("name-0"));
        assert!(server.native_cache.contains_key("name-1"));
        assert!(server.native_cache.contains_key("latest"));
        assert_eq!(server.native_cache_order.len(), NATIVE_CACHE_LIMIT);
    }

    #[test]
    fn test_serve_rebuilds_the_native_context() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn dispatch(
            _name: &str,
            _args: Value,
            context: Option<&crate::Context>,
        ) -> StdResult<Value, String> {
            CALLS.fetch_add(1, Ordering::Relaxed);
            let handler = context.ok_or("missing context")?;
            Ok(json!({
                "event": handler.event,
                "payload": handler.payload,
                "config": handler.config,
                "state": handler.state,
                "now_ms": handler.now_ms,
                "data_dir": handler.data_dir.to_string_lossy(),
            }))
        }

        CALLS.store(0, Ordering::Relaxed);
        let spool = TestSpool::new("serve-native-context");
        let mut first = native_request(
            "boot-native-context",
            "describe",
            1000,
            "first",
            json!({ "value": 1 }),
        );
        first["context"] = json!({
            "event": "click",
            "payload": { "id": "a" },
            "config": { "message": "hi" },
            "state": { "count": 2 },
            "host": { "now_ms": 1_700_000_000 },
        });
        let mut second = first.clone();
        second["id"] = json!("second");
        // Same arguments, different event: the context is part of the cache key.
        second["context"]["event"] = json!("render");
        let mut third = first.clone();
        third["id"] = json!("third");
        // Only the clock moved: that is not part of the key.
        third["context"]["host"]["now_ms"] = json!(1_700_000_999);
        spool.write("000000000001.json", &first);
        spool.write("000000000002.json", &second);
        spool.write("000000000003.json", &third);
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .with_native(dispatch);

        let writer = Capture {
            plugin: &plugin,
            data: &spool.data,
            spool: &spool.spool,
            boot: "boot-native-context",
            stale: BTreeSet::new(),
            stop_after: 4,
        }
        .run();
        let lines = writer.lines();
        spool.cleanup();

        assert_eq!(CALLS.load(Ordering::Relaxed), 2);
        let results: Vec<Value> = lines
            .iter()
            .filter(|line| line.contains("native_result"))
            .map(|line| serde_json::from_str(line).expect("json"))
            .collect();
        assert_eq!(results.len(), 3, "{lines:?}");
        assert_eq!(results[0]["value"]["event"], "click");
        assert_eq!(results[0]["value"]["payload"]["id"], "a");
        assert_eq!(results[0]["value"]["config"]["message"], "hi");
        assert_eq!(results[0]["value"]["state"]["count"], 2);
        assert_eq!(results[0]["value"]["now_ms"], 1_700_000_000_i64);
        assert_eq!(results[1]["value"]["event"], "render");
        assert_eq!(results[2]["value"]["now_ms"], 1_700_000_000_i64);
    }

    #[test]
    #[expect(
        clippy::excessive_nesting,
        reason = "test dispatch has three cases"
    )]
    fn test_serve_answers_a_native_batch_in_one_line() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        fn dispatch(
            name: &str,
            _args: Value,
            _context: Option<&crate::Context>,
        ) -> StdResult<Value, String> {
            CALLS.fetch_add(1, Ordering::Relaxed);
            if name == "missing" {
                return Err("unknown native function".to_owned());
            }
            Ok(json!({ "rows": 1 }))
        }

        CALLS.store(0, Ordering::Relaxed);
        let spool = TestSpool::new("serve-native-batch");
        let args = json!({ "value": 7 });
        spool.write(
            "000000000001.json",
            &json!({
                "boot": "boot-native-batch",
                "kind": "native",
                "entry": "service",
                "id": "batch-1",
                "calls": [
                    { "id": "1", "name": "rows", "args": args, "cache_ms": 1000 },
                    { "id": "2", "name": "rows", "args": { "value": 7 }, "cache_ms": 1000 },
                    { "id": "3", "name": "missing", "args": {}, "cache_ms": 0 },
                    { "id": "4", "name": "rows", "args": {}, "cache_ms": 0 },
                ],
            }),
        );
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .with_native(dispatch);
        let writer = Capture {
            plugin: &plugin,
            data: &spool.data,
            spool: &spool.spool,
            boot: "boot-native-batch",
            stale: BTreeSet::new(),
            stop_after: 2,
        }
        .run();
        let lines = writer.lines();
        spool.cleanup();

        assert_eq!(lines.len(), 2, "{lines:?}");
        let value: Value = serde_json::from_str(&lines[1]).expect("batch line");
        assert_eq!(value["control"], "native_batch_result");
        assert_eq!(value["id"], "batch-1");
        assert_eq!(value["results"][0]["value"], json!({ "rows": 1 }));
        assert_eq!(value["results"][1]["value"], json!({ "rows": 1 }));
        assert_eq!(value["results"][2]["error"], "unknown native function");
        assert_eq!(
            value["results"][3]["error"],
            "skipped after an earlier native failure"
        );
        assert_eq!(CALLS.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_native_cache_honors_ttl() {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        #[expect(clippy::unnecessary_wraps)]
        fn dispatch(
            _name: &str,
            _args: Value,
            _context: Option<&crate::Context>,
        ) -> StdResult<Value, String> {
            CALLS.fetch_add(1, Ordering::Relaxed);
            Ok(json!({ "value": 1 }))
        }

        CALLS.store(0, Ordering::Relaxed);
        let spool = TestSpool::new("serve-native-ttl");
        let plugin = Plugin::new(Meta::new("me/hello", "Hello", "0.1.0"))
            .with_native(dispatch);
        let translations = Value::Null;
        let mut out = CaptureWriter::new(10);
        let mut server = Serve {
            plugin: &plugin,
            data_dir: &spool.data,
            spool_dir: &spool.spool,
            boot: "boot-native-ttl",
            stale: BTreeSet::new(),
            native_cache: BTreeMap::new(),
            native_cache_order: VecDeque::new(),
            host_fixed: Value::Object(Map::new()),
            translations: &translations,
            out: &mut out,
        };
        let args = json!({ "key": 1 });
        let first = server.native_value(
            "service",
            "rows",
            args.clone(),
            5,
            &Value::Null,
        );
        let second = server.native_value(
            "service",
            "rows",
            args.clone(),
            5,
            &Value::Null,
        );
        thread::sleep(Duration::from_millis(20));
        let third =
            server.native_value("service", "rows", args, 5, &Value::Null);
        spool.cleanup();

        assert_eq!(first.expect("first"), json!({ "value": 1 }));
        assert_eq!(second.expect("second"), json!({ "value": 1 }));
        assert_eq!(third.expect("third"), json!({ "value": 1 }));
        assert_eq!(CALLS.load(Ordering::Relaxed), 2);
    }
}
