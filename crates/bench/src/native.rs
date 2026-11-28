//! A scratch helper with real `#[native]` functions for round-trip benchmarks.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use super::Result;

const MANIFEST: &str = r#"
id = "bench/native-helper"
name = "Bench Native Helper"
plugin_api = 57
author = "Bench"
license = "MIT"
description = "Native helper for the benchmark harness"
tags = ["bench"]
icon = "sparkles"
dependencies = []

[[service]]
id = "service"
entry = "service.luau"

[[widget]]
id = "widget"
entry = "widget.luau"
handler = "widget"
luau = "src/main.rs"
"#;

const SERVICE: &str = "function update() end\n";

const SOURCE: &str = r#"
use std::process::ExitCode;

use noctalia_rs::{Context, Op, Result, native};

include!(concat!(env!("OUT_DIR"), "/generated.rs"));

#[native]
fn bench_work(value: u64) -> u64 {
    value.wrapping_mul(6364136223846793005).wrapping_add(1)
}

#[native(cache_ms = 60_000)]
fn bench_cached(value: u64) -> u64 {
    let mut value = value;
    for _ in 0..200_000 {
        value = bench_work(value);
    }
    value
}

fn widget(_ctx: &Context) -> Result<Vec<Op>> {
    let first = bench_work(7);
    let second = bench_cached(7);
    Ok(vec![Op::log(format!("{first}:{second}"))])
}

fn service(_ctx: &Context) -> Result<Vec<Op>> {
    Ok(Vec::new())
}

fn main() -> ExitCode {
    noctalia_rs::run(&plugin())
}
"#;

pub struct NativeHelper {
    binary: PathBuf,
}

impl NativeHelper {
    pub fn path(&self) -> &Path {
        &self.binary
    }

    #[expect(
        clippy::excessive_nesting,
        reason = "build retries once offline then online"
    )]
    pub fn build(workspace: &Path, scratch: &Path) -> Result<Self> {
        let root = scratch.join("native-helper");
        fs::create_dir_all(root.join("src"))?;
        let deps = workspace.display().to_string();
        fs::write(
            root.join("Cargo.toml"),
            format!(
                "[package]\nname = \"noctalia-bench-native-helper\"\nversion = \"0.1.0\"\nedition = \"2024\"\npublish = false\n\n\
                 [dependencies]\nnoctalia-rs = {{ path = \"{deps}/crates/lib\" }}\nnoctalia-rs-luau = {{ path = \"{deps}/crates/luau\" }}\nserde_json = \"1\"\n\n\
                 [build-dependencies]\nnoctalia-rs-build = {{ path = \"{deps}/crates/build\" }}\n\n[workspace]\n"
            ),
        )?;
        fs::write(
            root.join("build.rs"),
            "fn main() -> Result<(), Box<dyn std::error::Error>> {\n    noctalia_rs_build::generate()\n}\n",
        )?;
        fs::write(root.join("noctalia.toml"), MANIFEST)?;
        fs::write(root.join("service.luau"), SERVICE)?;
        fs::write(root.join("src/main.rs"), SOURCE)?;

        let cargo = std::env::var_os("CARGO")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("cargo"));
        let target_dir = std::env::var_os("NOCTALIA_BENCH_NATIVE_TARGET")
            .map_or_else(|| root.join("target"), PathBuf::from);
        let mut command = Command::new(&cargo);
        command
            .args([
                "build",
                "--release",
                "--quiet",
                "--offline",
                "--manifest-path",
            ])
            .arg(root.join("Cargo.toml"))
            .env("CARGO_TARGET_DIR", &target_dir)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let output = command.output()?;
        if !output.status.success() {
            let retry = Command::new(&cargo)
                .args(["build", "--release", "--quiet", "--manifest-path"])
                .arg(root.join("Cargo.toml"))
                .env("CARGO_TARGET_DIR", &target_dir)
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output()?;
            if !retry.status.success() {
                return Err(format!(
                    "native helper build failed: {}",
                    String::from_utf8_lossy(&retry.stderr)
                )
                .into());
            }
        }
        Ok(Self {
            binary: target_dir.join("release/noctalia-bench-native-helper"),
        })
    }
}
