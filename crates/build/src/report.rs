//! Build-time boundary report.
//!
//! The report is written beside `generated.rs` so a build or a CI job can see
//! how each entry was compiled without reading the generated source again.

use std::error::Error;
use std::fs;
use std::path::Path;

use noctalia_rs_compiler::BoundaryReport as CompilerReport;
use serde_json::{Map, Value, json};

type BoxError = Box<dyn Error>;

/// Collects one record per declared entry.
pub(crate) struct BuildReport {
    mode: &'static str,
    entries: Vec<Value>,
}

impl BuildReport {
    pub(crate) const fn new(mode: &'static str) -> Self {
        Self {
            mode,
            entries: Vec::new(),
        }
    }

    pub(crate) fn push_compiled(&mut self, id: &str, report: &CompilerReport) {
        let helper_calls = report
            .helper_calls_per_event
            .iter()
            .map(|(event, calls)| (event.clone(), json!(calls)))
            .collect::<Map<_, _>>();
        self.entries.push(json!({
            "id": id,
            "compiled": true,
            "compiled_functions": report.compiled_functions,
            "native_functions": report.native_functions,
            "helper_calls_per_event": helper_calls,
            "generated_bytes": report.generated_bytes,
        }));
    }

    /// Records an entry that runs a hand-written Luau script or stays native
    /// because the compile mode is `off`.
    pub(crate) fn push_uncompiled(
        &mut self,
        id: &str,
        source: &Path,
        script: Option<&str>,
    ) -> Result<(), BoxError> {
        let bytes = script
            .and_then(|name| fs::read(source.join(format!("{name}.luau"))).ok())
            .map_or(0, |data| data.len());
        self.entries.push(json!({
            "id": id,
            "compiled": false,
            "compiled_functions": 0,
            "native_functions": 0,
            "helper_calls_per_event": Map::new(),
            "generated_bytes": bytes,
        }));
        Ok(())
    }

    pub(crate) fn write(&self, out_dir: &Path) -> Result<(), BoxError> {
        let report = json!({
            "version": 1,
            "mode": self.mode,
            "entries": self.entries,
        });
        let target = out_dir.join("noctalia-boundary-report.json");
        fs::write(
            target,
            format!("{}\n", serde_json::to_string_pretty(&report)?),
        )?;
        Ok(())
    }
}
