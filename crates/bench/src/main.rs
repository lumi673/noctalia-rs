mod metadata;
mod process;
mod stats;

use chrono::Utc;
use serde::Serialize;
use serde_json::{Value, json};

use metadata::{Metadata, SamplePlan};
use stats::{Phase, Summary};

use std::env;
use std::error::Error;
use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const SCRIPT_VM: &str = "function update() end";
const SCRIPT_RUNTIME: &str = r#"
function update(text)
  local rows = json.decode(text)
  local total = 0
  for _, row in ipairs(rows) do
    total = total + row.value
  end
  noctalia.log(tostring(total))
end
"#;
const SCRIPT_JSON: &str = r#"
function update()
  local text = json.encode({
    id = 42, name = "bench", ok = true,
    rows = { 1, 2, 3, 4, 5, 6, 7, 8 }
  })
  local value = json.decode(text)
  assert(value.id == 42, "id")
  assert(value.name == "bench", "name")
  assert(value.ok == true, "ok")
  assert(value.rows[8] == 8, "rows")
end
"#;
const SCRIPT_SNAPSHOT: &str = r##"
function update()
  assert(noctalia.getConfig("name") == "bench", "config.name")
  assert(noctalia.getConfig("count") == 3, "config.count")
  assert(state.get("label") == "ready", "state.label")
  assert(noctalia.getSetting("shell.font") == "sans", "setting")
  assert(noctalia.getColor("primary") == "#fff", "color")
  assert(noctalia.systemStats().cpu == 12, "stats")
  assert(noctalia.nowMs() == 1700000000, "now")
end
"##;
const FIXTURE_MANIFEST: &str = r#"
id = "bench/fixture"
name = "Bench Fixture"
plugin_api = 57
author = "Bench"
license = "MIT"
description = "Generated benchmark fixture"
tags = ["bench"]
icon = "sparkles"
dependencies = []

[[setting]]
key = "message"
type = "string"
label_key = "settings.message.label"
default = "Hello"

[[service]]
id = "service"
entry = "service.luau"
"#;
const SOURCES_VM: &[(&str, &str)] = &[("entry", SCRIPT_VM)];
const SOURCES_RUNTIME: &[(&str, &str)] = &[("entry", SCRIPT_RUNTIME)];
const SOURCES_JSON: &[(&str, &str)] = &[("entry", SCRIPT_JSON)];
const SOURCES_SNAPSHOT: &[(&str, &str)] = &[("entry", SCRIPT_SNAPSHOT)];

#[derive(Debug)]
struct Options {
    samples: usize,
    warmup: usize,
    build_samples: usize,
    build_warmup: usize,
    helper: Option<PathBuf>,
    report: Option<PathBuf>,
}
impl Options {
    fn plan(&self) -> SamplePlan {
        SamplePlan {
            samples: self.samples,
            warmup: self.warmup,
            build_samples: self.build_samples,
            build_warmup: self.build_warmup,
        }
    }

    fn phase(&self, surface: &'static str, build: bool) -> Phase {
        if build {
            Phase {
                surface,
                samples: self.build_samples,
                warmup: self.build_warmup,
            }
        } else {
            Phase {
                surface,
                samples: self.samples,
                warmup: self.warmup,
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct Report {
    metadata: Metadata,
    results: Vec<Summary>,
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("noctalia-bench: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let options = parse_options()?;
    let workspace =
        fs::canonicalize(Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))?;
    let helper = match &options.helper {
        Some(path) => fs::canonicalize(path).unwrap_or_else(|_| path.clone()),
        None => {
            build_helper(&workspace)?;
            helper_path(&workspace)
        }
    };
    if !helper.is_file() {
        return Err(
            format!("helper binary missing: {}", helper.display()).into()
        );
    }

    let scratch = Scratch::new(&workspace)?;
    let metadata = Metadata::collect(&workspace, &helper, options.plan());
    let mut results = Vec::new();

    results.push(measure(
        options.phase("fresh_process_dispatch", false),
        || process::fresh(&helper, &scratch.path().join("fresh")),
    )?);

    let mut spool = process::Spool::start(&helper, scratch.path())?;
    results.push(measure(options.phase("spool_dispatch", false), || {
        spool.request()
    })?);
    drop(spool);

    let mut vm_counter = 0usize;
    results.push(measure(options.phase("luau_vm_creation", false), || {
        let dir = scratch.path().join(format!("vm-cold/{vm_counter}"));
        vm_counter += 1;
        let ctx = context(("entry", "update"), &dir, payload("{}"));
        let ops = noctalia_luau::dispatch(&ctx, SOURCES_VM)?;
        std::hint::black_box(ops);
        Ok(())
    })?);

    let vm_dir = scratch.path().join("vm-warm");
    results.push(measure(
        options.phase("luau_vm_cache_reuse", false),
        || {
            let ctx = context(("entry", "update"), &vm_dir, payload("{}"));
            let ops = noctalia_luau::dispatch(&ctx, SOURCES_VM)?;
            std::hint::black_box(ops);
            Ok(())
        },
    )?);

    let runtime_dir = scratch.path().join("runtime");
    let rows = serde_json::to_string(&json!([
        { "value": 1 }, { "value": 2 }, { "value": 3 }, { "value": 4 }
    ]))?;
    results.push(measure(
        options.phase("luau_runtime_dispatch", false),
        || {
            let ctx =
                context(("entry", "update"), &runtime_dir, payload(&rows));
            let ops =
                noctalia_luau::dispatch_entry(&ctx, SOURCES_RUNTIME, "entry")?;
            std::hint::black_box(ops);
            Ok(())
        },
    )?);

    let json_dir = scratch.path().join("json");
    results.push(measure(
        options.phase("json_roundtrip_luau", false),
        || {
            let ctx = context(("entry", "update"), &json_dir, payload("{}"));
            let ops = noctalia_luau::dispatch(&ctx, SOURCES_JSON)?;
            std::hint::black_box(ops);
            Ok(())
        },
    )?);

    let bridge_source = bridge_snapshot_source();
    let bridge_sources = [("entry", bridge_source.as_str())];
    let bridge_dir = scratch.path().join("bridge-snapshot");
    results.push(measure(
        options.phase("bridge_snapshot_reads", false),
        || {
            let ctx = context(("entry", "update"), &bridge_dir, payload("{}"));
            let ops = noctalia_luau::dispatch(&ctx, &bridge_sources)?;
            std::hint::black_box(ops);
            Ok(())
        },
    )?);

    let sample = sample_value();
    let encoded = serde_json::to_string(&sample)?;
    results.push(measure(options.phase("json_encode_rust", false), || {
        let output = serde_json::to_string(&sample)?;
        std::hint::black_box(output);
        Ok(())
    })?);
    results.push(measure(options.phase("json_decode_rust", false), || {
        let output = serde_json::from_str::<Value>(&encoded)?;
        std::hint::black_box(output);
        Ok(())
    })?);

    let snapshot_dir = scratch.path().join("snapshot-rust");
    results.push(measure(
        options.phase("snapshot_reads_rust", false),
        || {
            let ctx =
                context(("entry", "update"), &snapshot_dir, payload("{}"));
            std::hint::black_box(ctx.config_str("name"));
            std::hint::black_box(ctx.config_bool("count", false));
            std::hint::black_box(ctx.state.get("label"));
            std::hint::black_box(ctx.host("stats.cpu"));
            std::hint::black_box(ctx.setting("shell.font"));
            std::hint::black_box(ctx.color("primary"));
            std::hint::black_box(ctx.now_ms);
            Ok(())
        },
    )?);

    let snapshot_luau_dir = scratch.path().join("snapshot-luau");
    results.push(measure(
        options.phase("snapshot_reads_luau", false),
        || {
            let ctx =
                context(("entry", "update"), &snapshot_luau_dir, payload("{}"));
            let ops = noctalia_luau::dispatch(&ctx, SOURCES_SNAPSHOT)?;
            std::hint::black_box(ops);
            Ok(())
        },
    )?);

    let mut fixture = Fixture::new(&workspace, scratch.path())?;
    results.push(measure(options.phase("codegen_generate", true), || {
        noctalia_build::generate_to(&fixture.manifest, &fixture.out_dir)?;
        Ok(())
    })?);
    results.push(measure(
        options.phase("build_incremental_release", true),
        || fixture.rebuild(),
    )?);

    let report = Report { metadata, results };
    print_report(&report);
    if let Some(path) = &options.report {
        fs::write(path, serde_json::to_string_pretty(&report)?)?;
    }
    Ok(())
}

fn measure<F>(phase: Phase, mut run: F) -> Result<Summary>
where
    F: FnMut() -> Result<()>,
{
    for _ in 0..phase.warmup {
        run()?;
    }
    let mut times = Vec::with_capacity(phase.samples);
    for _ in 0..phase.samples {
        let start = Instant::now();
        run()?;
        times.push(start.elapsed());
    }
    Ok(Summary::new(phase, times))
}

fn context(
    names: (&str, &str),
    data_dir: &Path,
    payload: Value,
) -> noctalia::Context {
    noctalia::Context::new(
        noctalia::Parts {
            entry: names.0.to_owned(),
            event: names.1.to_owned(),
            payload,
            config: sample_config(),
            state: sample_state(),
            host: sample_host(),
            data_dir: data_dir.to_path_buf(),
        },
        json!({ "greet": { "label": "Hello {name}" } }),
    )
}

fn bridge_snapshot_source() -> String {
    let bridge = noctalia::bridge::SOURCE
        .replace("{{HELPER}}", "bench-helper")
        .replace("{{PREFIX}}", "bench");
    format!(
        "local bridge = (function()\n{bridge}\nend)()\n\
         bridge.setup(\"service\", {{\"name\"}})\n\
         function update()\n  bridge.call(\"update\", {{}})\nend\n"
    )
}

fn payload(text: &str) -> Value {
    json!({ "args": [text] })
}

fn sample_config() -> Value {
    json!({ "name": "bench", "count": 3 })
}

fn sample_state() -> Value {
    json!({ "label": "ready", "timers": [1, 2, 3] })
}

fn sample_host() -> Value {
    json!({
        "now_ms": 1700000000_i64,
        "colors": { "primary": "#fff" },
        "setting": { "shell.font": "sans" },
        "stats": { "cpu": 12 }
    })
}

fn sample_value() -> Value {
    json!({
        "id": 42,
        "name": "bench",
        "enabled": true,
        "rows": [
            { "id": 1, "label": "one" },
            { "id": 2, "label": "two" },
            { "id": 3, "label": "three" }
        ],
        "nested": { "tags": ["a", "b"], "ratio": 0.75 }
    })
}

fn helper_path(workspace: &Path) -> PathBuf {
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("target"));
    target.join("release/noctalia-template")
}

fn build_helper(workspace: &Path) -> Result<()> {
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--release", "--quiet", "-p", "noctalia-template"])
        .arg("--manifest-path")
        .arg(workspace.join("Cargo.toml"))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    if !status.success() {
        return Err("failed to build release helper".into());
    }
    Ok(())
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(workspace: &Path) -> Result<Self> {
        let root = workspace.join(".tmp");
        fs::create_dir_all(&root)?;
        let name = format!(
            "bench-{}-{}",
            std::process::id(),
            Utc::now().timestamp_nanos_opt().unwrap_or(0)
        );
        let path = root.join(name);
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    manifest: PathBuf,
    out_dir: PathBuf,
    cargo: PathBuf,
    root: PathBuf,
    source: PathBuf,
    target_dir: PathBuf,
    revision: u64,
}

impl Fixture {
    fn new(workspace: &Path, scratch: &Path) -> Result<Self> {
        let root = scratch.join("fixture");
        fs::create_dir_all(root.join("src"))?;
        let out_dir = scratch.join("generated");
        fs::create_dir_all(&out_dir)?;

        let deps = workspace.display().to_string();
        let manifest = format!(
            "[package]\nname = \"noctalia-bench-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n\
             [dependencies]\nnoctalia-rs = {{ path = \"{deps}/crates/lib\" }}\nnoctalia-rs-luau = {{ path = \"{deps}/crates/luau\" }}\nserde_json = \"1\"\n\n\
             [build-dependencies]\nnoctalia-rs-build = {{ path = \"{deps}/crates/build\" }}\n\n[workspace]\n"
        );
        fs::write(root.join("Cargo.toml"), manifest)?;
        fs::write(
            root.join("build.rs"),
            "fn main() -> Result<(), Box<dyn std::error::Error>> {\n    noctalia_build::generate()\n}\n",
        )?;
        fs::write(root.join("noctalia.toml"), FIXTURE_MANIFEST)?;
        fs::write(
            root.join("service.luau"),
            "function update()\n  noctalia.log(noctalia.getConfig(\"message\"))\nend\n",
        )?;
        let source = root.join("src/main.rs");
        fs::write(
            &source,
            "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\nfn main() {}\n",
        )?;

        let target_dir = root.join("target");
        Ok(Self {
            manifest: root.join("noctalia.toml"),
            out_dir,
            cargo: env::var_os("CARGO")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("cargo")),
            root,
            source,
            target_dir,
            revision: 0,
        })
    }

    fn rebuild(&mut self) -> Result<()> {
        self.revision += 1;
        let mut file = OpenOptions::new().append(true).open(&self.source)?;
        writeln!(file, "// revision {}", self.revision)?;
        drop(file);

        let mut command = Command::new(&self.cargo);
        command
            .args([
                "build",
                "--release",
                "--quiet",
                "--offline",
                "--manifest-path",
            ])
            .arg(self.root.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", &self.target_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let output = command.output()?;
        if output.status.success() {
            return Ok(());
        }
        let retry = Command::new(&self.cargo)
            .args(["build", "--release", "--quiet", "--manifest-path"])
            .arg(self.root.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", &self.target_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()?;
        if retry.status.success() {
            return Ok(());
        }
        Err(format!(
            "fixture build failed: {}",
            String::from_utf8_lossy(&retry.stderr)
        )
        .into())
    }
}

fn parse_options() -> Result<Options> {
    let mut options = Options {
        samples: 200,
        warmup: 20,
        build_samples: 5,
        build_warmup: 1,
        helper: None,
        report: None,
    };
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{}", usage());
                std::process::exit(0);
            }
            "--samples" => options.samples = number(&mut args, "--samples")?,
            "--warmup" => options.warmup = number(&mut args, "--warmup")?,
            "--build-samples" => {
                options.build_samples = number(&mut args, "--build-samples")?
            }
            "--build-warmup" => {
                options.build_warmup = number(&mut args, "--build-warmup")?
            }
            "--helper" => {
                options.helper =
                    Some(PathBuf::from(next(&mut args, "--helper")?))
            }
            "--report" => {
                options.report =
                    Some(PathBuf::from(next(&mut args, "--report")?))
            }
            other => {
                return Err(
                    format!("unknown option {other}. {}", usage()).into()
                );
            }
        }
    }
    if options.samples == 0 || options.build_samples == 0 {
        return Err("sample counts must be positive".into());
    }
    Ok(options)
}

fn next(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    args.next()
        .ok_or_else(|| format!("missing value for {flag}").into())
}

fn number(
    args: &mut impl Iterator<Item = String>,
    flag: &str,
) -> Result<usize> {
    let value = next(args, flag)?;
    value
        .parse::<usize>()
        .map_err(|_| format!("invalid value for {flag}: {value}").into())
}

fn usage() -> String {
    let mut text = String::new();
    let _ = writeln!(text, "usage: noctalia-bench [OPTIONS]");
    let _ = writeln!(
        text,
        "  --samples N        samples per runtime surface (default 200)"
    );
    let _ = writeln!(
        text,
        "  --warmup N         warmups per runtime surface (default 20)"
    );
    let _ = writeln!(
        text,
        "  --build-samples N  samples per build surface (default 5)"
    );
    let _ = writeln!(
        text,
        "  --build-warmup N   warmups per build surface (default 1)"
    );
    let _ = writeln!(
        text,
        "  --helper PATH      use an existing release helper binary"
    );
    let _ = writeln!(text, "  --report PATH      write a JSON report");
    text
}

fn print_report(report: &Report) {
    let meta = &report.metadata;
    println!("timestamp: {}", meta.timestamp_utc);
    println!("machine: {} | {}", meta.os, meta.cpu_model);
    println!(
        "cpus: {} | max {} kHz | memory {} KiB",
        meta.logical_cpus, meta.cpu_max_freq_khz, meta.memory_total_kib
    );
    println!(
        "toolchain: {} | {} | profile {}",
        meta.rustc, meta.cargo, meta.profile
    );
    println!(
        "workspace: {} | git {} | dirty {}",
        meta.workspace, meta.git_head, meta.git_dirty
    );
    println!("helper: {}", meta.helper);
    println!(
        "samples: {} (warmup {}), build samples: {} (warmup {})",
        meta.samples, meta.warmup, meta.build_samples, meta.build_warmup
    );
    println!(
        "method: median (mean of two middle values for even n), {} p95/p99, {}",
        meta.percentile_method, meta.timer
    );
    println!();
    println!(
        "{:<28} {:>7} {:>6} {:>12} {:>12} {:>12} {:>10} {:>10}",
        "surface",
        "samples",
        "warmup",
        "median_us",
        "p95_us",
        "p99_us",
        "min_us",
        "max_us"
    );
    for item in &report.results {
        println!(
            "{:<28} {:>7} {:>6} {:>12.3} {:>12.3} {:>12.3} {:>10.3} {:>10.3}",
            item.surface,
            item.samples,
            item.warmup,
            item.median_us,
            item.p95_us,
            item.p99_us,
            item.min_us,
            item.max_us
        );
    }
}
