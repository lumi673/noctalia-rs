pub mod bridge;
pub mod cli;
pub mod codegen;
pub mod context;
pub mod manifest;
pub mod ops;
pub mod ui;

use std::error::Error;
use std::result::Result as StdResult;

pub use cli::run;
pub use context::{Context, Parts};
pub use manifest::{
    Entry, EntryConfig, EntryKind, LauncherCategory, LayerConfig, Meta,
    MetaBuilder, MetaReady, NeedId, NeedName, NeedVersion, NumberConfig,
    Plugin, Setting, SettingKind,
};
pub use noctalia_rs_macros::entry;
pub use ops::Op;
pub use serde_json::Value;

pub type Result<T> = StdResult<T, Box<dyn Error>>;
