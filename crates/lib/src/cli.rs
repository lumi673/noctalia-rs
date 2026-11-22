use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{self, ExitCode};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use serde_json::{Value, json};

use crate::Result;
use crate::context::{Context, Parts};
use crate::manifest::Plugin;
use crate::ops::Op;

const POLL_INTERVAL: Duration = Duration::from_millis(1);

#[derive(Debug, Deserialize)]
struct Request {
    boot: String,
    entry: String,
    event: String,
    #[serde(default)]
    payload: Value,
    #[serde(default)]
    config: Value,
    #[serde(default)]
    state: Value,
    #[serde(default)]
    host: Value,
}

struct Serve<'a, W: Write> {
    plugin: &'a Plugin,
    data_dir: &'a Path,
    spool_dir: &'a Path,
    boot: &'a str,
    stale: BTreeSet<PathBuf>,
    translations: &'a Value,
    out: &'a mut W,
}

impl<W: Write> Serve<'_, W> {
    fn run(&mut self) -> Result<()> {
        self.emit_ready()?;
        loop {
            let processed = self.process_batch()?;
            Self::pause_if_idle(processed);
        }
    }

    fn pause_if_idle(processed: usize) {
        if processed == 0 {
            thread::sleep(POLL_INTERVAL);
        }
    }

    fn emit_ready(&mut self) -> Result<()> {
        self.emit_json(&json!({
            "control": "ready",
            "boot": self.boot,
            "spool": self.spool_dir.to_string_lossy()
        }))
    }

    fn process_batch(&mut self) -> Result<usize> {
        let mut processed = 0usize;
        for path in request_files(self.spool_dir)? {
            let count = self.process_one(&path)?;
            processed = processed.saturating_add(count);
        }
        Ok(processed)
    }

    fn process_one(&mut self, path: &Path) -> Result<usize> {
        let name = request_name(path);
        if self.stale.contains(path) {
            self.emit_error(&name, "stale boot")?;
            let _ = fs::remove_file(path);
            return Ok(1);
        }

        match self.process_request(path) {
            Ok(()) => self.emit_done(&name)?,
            Err(error) => self.emit_error(&name, &error.to_string())?,
        }
        let _ = fs::remove_file(path);
        Ok(1)
    }

    fn process_request(&mut self, path: &Path) -> Result<()> {
        let text = fs::read_to_string(path)?;
        let request: Request = serde_json::from_str(&text)?;
        if request.boot != self.boot {
            return Err(format!(
                "stale boot {} (expected {})",
                request.boot, self.boot
            )
            .into());
        }

        let parts = Parts {
            entry: request.entry,
            event: request.event,
            payload: request.payload,
            config: request.config,
            state: request.state,
            host: request.host,
            data_dir: self.data_dir.to_path_buf(),
        };
        let context = Context::new(parts, self.translations.clone());
        for op in self.plugin.dispatch(&context)? {
            self.emit_op(&op)?;
        }
        Ok(())
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
    let mut out = stdout.lock();
    let boot = boot_id();
    let run_dir = spool_dir.join(&boot);
    fs::create_dir_all(&run_dir)?;
    let stale = request_files(&run_dir)?;
    let translations = plugin.translations_value();
    let mut server = Serve {
        plugin,
        data_dir,
        spool_dir: &run_dir,
        boot: &boot,
        stale,
        translations: &translations,
        out: &mut out,
    };
    server.run()
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
    let json = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"));
    json && path.is_file()
}

fn request_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map_or_else(|| "unknown".to_owned(), str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Meta, Op};
    use std::process;

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
            Ok(())
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
}
