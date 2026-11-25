pub mod api;
pub mod bridge;
pub mod bytes;
pub mod cli;
pub mod clock;
pub mod codegen;
pub mod context;
pub mod coroutine;
pub mod manifest;
pub mod ops;
pub mod rng;
pub mod ui;
use std::error::Error;
use std::result::Result as StdResult;

pub use bytes::ByteBuf;
pub use cli::run;
pub use clock::{ClockBudget, os_clock, unix_seconds};
pub use context::{Context, FileInfo, Parts};
pub use coroutine::{
    Coroutine, Poll as CoroutinePoll, run_budgeted, yield_now, yield_value,
};
pub use manifest::{
    Entry, EntryConfig, EntryKind, ExclusiveZone, LauncherCategory,
    LayerConfig, Meta, MetaBuilder, MetaReady, NeedId, NeedName, NeedVersion,
    NumberConfig, Plugin, Setting, SettingKind,
};
pub use rng::LuauRng;
pub use ui::{NodeProps, Props};

pub use noctalia_rs_macros::{entry, luau, native};
pub use ops::Op;
pub use serde_json::Value;

pub type Result<T> = StdResult<T, Box<dyn Error>>;
