use serde_json::{Value, json};

use std::error::Error;
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

pub fn fresh(helper: &Path, data_dir: &Path) -> Result<()> {
    let args: Vec<OsString> = vec![
        "service".into(),
        "update".into(),
        data_dir.as_os_str().to_os_string(),
        "{}".into(),
        r#"{"message":"bench"}"#.into(),
        "{}".into(),
        crate::sample_host().to_string().into(),
    ];
    let output = Command::new(helper).args(args).output()?;
    if !output.status.success() {
        return Err(format!("fresh dispatch exited with {output:?}").into());
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !stdout.lines().any(|line| line.contains("\"op\"")) {
        return Err("fresh dispatch emitted no ops".into());
    }
    Ok(())
}

pub struct Spool {
    child: Child,
    lines: Receiver<String>,
    boot: String,
    spool_dir: PathBuf,
    sequence: u64,
}

impl Spool {
    pub fn start(helper: &Path, scratch: &Path) -> Result<Self> {
        let data_dir = scratch.join("spool/data");
        let spool_dir = scratch.join("spool/requests");
        fs::create_dir_all(&data_dir)?;
        fs::create_dir_all(&spool_dir)?;

        let mut child = Command::new(helper)
            .arg("serve")
            .arg(&data_dir)
            .arg(&spool_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdout = child.stdout.take().ok_or("missing helper stdout")?;
        let (sender, lines) = mpsc::channel();
        std::thread::spawn(move || {
            BufReader::new(stdout)
                .lines()
                .map_while(|line| line.ok())
                .all(|line| sender.send(line).is_ok());
        });

        let ready = wait_line(&lines, |value| {
            value.get("control").and_then(Value::as_str) == Some("ready")
        })?;
        let boot = ready
            .get("boot")
            .and_then(Value::as_str)
            .ok_or("ready line has no boot id")?
            .to_owned();
        let run_dir = ready
            .get("spool")
            .and_then(Value::as_str)
            .ok_or("ready line has no spool dir")?;

        Ok(Self {
            child,
            lines,
            boot,
            spool_dir: PathBuf::from(run_dir),
            sequence: 0,
        })
    }

    pub fn request(&mut self) -> Result<()> {
        self.sequence += 1;
        let name = format!("{:012}.json", self.sequence);
        let request = json!({
            "boot": self.boot,
            "entry": "service",
            "event": "update",
            "payload": {},
            "config": { "message": "bench" },
            "state": {},
            "host": crate::sample_host(),
        });
        fs::write(
            self.spool_dir.join(&name),
            serde_json::to_string(&request)?,
        )?;

        let start = Instant::now();
        wait_line(&self.lines, |value| {
            value.get("control").and_then(Value::as_str) == Some("done")
                && value.get("request").and_then(Value::as_str)
                    == Some(name.as_str())
        })?;
        std::hint::black_box(start.elapsed());
        Ok(())
    }

    fn write_native_request(&mut self, mut request: Value) -> Result<String> {
        self.sequence += 1;
        let file = format!("{:012}.json", self.sequence);
        let id = format!("native-{}", self.sequence);
        let object = request
            .as_object_mut()
            .ok_or("native request is not an object")?;
        object.insert("boot".to_owned(), json!(self.boot));
        object.insert("entry".to_owned(), json!("service"));
        object.insert("id".to_owned(), json!(id));
        fs::write(self.spool_dir.join(file), serde_json::to_string(&request)?)?;
        Ok(id)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "a native call carries its cache policy"
    )]
    pub fn native(
        &mut self,
        name: &str,
        args: Value,
        cache_ms: u64,
    ) -> Result<Value> {
        let id = self.write_native_request(json!({
            "kind": "native",
            "name": name,
            "args": args,
            "cache_ms": cache_ms,
        }))?;
        let line = wait_line(&self.lines, |value| {
            value.get("control").and_then(Value::as_str)
                == Some("native_result")
                && value.get("id").and_then(Value::as_str) == Some(id.as_str())
        })?;
        if !matches!(line.get("error"), None | Some(Value::Null)) {
            return Err(format!("native result error: {line}").into());
        }
        let value = line
            .get("value")
            .cloned()
            .ok_or("native result has no value")?;
        std::hint::black_box(&value);
        Ok(value)
    }

    pub fn native_batch(
        &mut self,
        calls: &[(&str, Value, u64)],
    ) -> Result<Vec<Value>> {
        let entries = calls
            .iter()
            .enumerate()
            .map(|(index, (name, args, cache_ms))| {
                json!({
                    "id": index.to_string(),
                    "name": name,
                    "args": args,
                    "cache_ms": cache_ms,
                })
            })
            .collect::<Vec<_>>();
        let id = self.write_native_request(json!({
            "kind": "native",
            "calls": entries,
        }))?;
        let line = wait_line(&self.lines, |value| {
            value.get("control").and_then(Value::as_str)
                == Some("native_batch_result")
                && value.get("id").and_then(Value::as_str) == Some(id.as_str())
        })?;
        let results = line
            .get("results")
            .and_then(Value::as_array)
            .ok_or("native batch result has no results")?;
        let mut values = Vec::with_capacity(results.len());
        for result in results {
            let value = batch_value(result)?;
            values.push(value);
        }
        Ok(values)
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn batch_value(result: &Value) -> Result<Value> {
    if !matches!(result.get("error"), None | Some(Value::Null)) {
        return Err(format!("native batch result error: {result}").into());
    }
    let value = result
        .get("value")
        .cloned()
        .ok_or("native batch result has no value")?;
    std::hint::black_box(&value);
    Ok(value)
}

fn wait_line<F>(lines: &Receiver<String>, accept: F) -> Result<Value>
where
    F: Fn(&Value) -> bool,
{
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    loop {
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("spool request timed out")?;
        let line = lines.recv_timeout(remaining)?;
        let value: Value = serde_json::from_str(&line)?;
        if value.get("control").and_then(Value::as_str) == Some("error") {
            return Err(format!("helper error: {value}").into());
        }
        if accept(&value) {
            return Ok(value);
        }
    }
}
