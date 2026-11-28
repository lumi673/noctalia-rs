//! Boundary metrics for one compiled entry.
//!
//! The counts describe the generated Luau script. They let a build or a CI job
//! compare how much of a handler runs in Luau and how much still crosses into
//! the helper process, without running the plugin.

use std::collections::BTreeMap;

/// One entry's compiled and native boundary counts.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BoundaryReport {
    /// Rust-authored Luau functions in the entry.
    ///
    /// Each emitted handler body counts once, plus any local function the
    /// emitter places inside that body.
    pub compiled_functions: usize,
    /// Functions that run in the helper process through the native boundary.
    pub native_functions: usize,
    /// Calls to native stubs in each emitted event body.
    pub helper_calls_per_event: BTreeMap<String, usize>,
    /// Size of the generated entry script in bytes.
    pub generated_bytes: usize,
}

impl BoundaryReport {
    pub(crate) fn new(
        compiled_functions: usize,
        native_functions: usize,
        helper_calls_per_event: BTreeMap<String, usize>,
        generated_bytes: usize,
    ) -> Self {
        Self {
            compiled_functions,
            native_functions,
            helper_calls_per_event,
            generated_bytes,
        }
    }
}
