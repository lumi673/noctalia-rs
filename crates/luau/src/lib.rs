mod callback;
mod globals;
mod host;
mod value;

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use mlua::{Function, Lua, MultiValue, Value as LuaValue};
use noctalia_rs::{Context, Op, Result};
use serde_json::Value as Json;

use crate::callback::{callback_args, callback_for_event};
use crate::host::Host;
use crate::value::{from_json, from_json_with_bytes};

pub use value::ARRAY_MARKER;

type SharedRuntime = Arc<Mutex<Runtime>>;

struct Runtime {
    host: Arc<Host>,
    lua: Lua,
    entry: String,
}

impl Runtime {
    #[expect(
        clippy::too_many_arguments,
        reason = "the strict flag travels with the entry"
    )]
    fn start(
        ctx: &Context,
        sources: &[(&str, &str)],
        script: (&str, &str),
        strict: bool,
    ) -> Result<(Self, Vec<Op>)> {
        let (name, source) = script;
        let host = Arc::new(if strict {
            Host::new_strict(ctx, sources)
        } else {
            Host::new(ctx, sources)
        });
        let lua = Lua::new();
        lua.set_app_data(Arc::clone(&host));
        register_sources(&lua, sources)?;
        globals::register(&lua)?;

        if let Err(error) = lua.load(source).set_name(name).exec() {
            return Err(format!("{name}: {}", traceback(&lua, &error)).into());
        }
        if strict && defines_host_callback(&lua) {
            return Err(format!(
                "{name}: strict emit defines `onHostCallback`, which plugin API 57 does not have"
            )
            .into());
        }
        let runtime = Self {
            host,
            lua,
            entry: name.to_owned(),
        };
        let mut ops = runtime.host.collector.take_ops();
        ops.extend(runtime.dispatch(ctx)?);
        Ok((runtime, ops))
    }

    fn dispatch(&self, ctx: &Context) -> Result<Vec<Op>> {
        self.host.update(ctx);
        let _ = self.host.collector.take_ops();
        if ctx.event == "host_callback" {
            return self.dispatch_host_callback(ctx);
        }

        if let Some(callback) = callback_for_event(&ctx.event)
            && let Ok(function) = self.lua.globals().get::<Function>(callback)
        {
            let args = callback_args(&self.host.snapshot, callback)
                .iter()
                .map(|value| from_json(&self.lua, value))
                .collect::<mlua::Result<Vec<_>>>()?;
            if let Err(error) =
                function.call::<MultiValue>(MultiValue::from_vec(args))
            {
                return Err(format!(
                    "{} {callback}: {}",
                    self.entry,
                    traceback(&self.lua, &error)
                )
                .into());
            }
        }

        Ok(self.host.collector.take_ops())
    }

    /// Runs a host callback delivered by name.
    ///
    /// Hand-written scripts pass Luau functions to the host, so the runtime
    /// resolves those ids here. A compiled entry passes a stable name instead
    /// and receives the payload in its `onHostCallback` global.
    fn dispatch_host_callback(&self, ctx: &Context) -> Result<Vec<Op>> {
        let Some(id) = ctx.payload.get("id").and_then(Json::as_str) else {
            return Ok(self.host.collector.take_ops());
        };
        if let Some(function) = self.host.collector.function(id) {
            let args = ctx
                .payload
                .get("args")
                .and_then(Json::as_array)
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|value| from_json_with_bytes(&self.lua, value))
                .collect::<mlua::Result<Vec<_>>>()?;
            if let Err(error) =
                function.call::<MultiValue>(MultiValue::from_vec(args))
            {
                return Err(format!(
                    "{} host callback {id}: {}",
                    self.entry,
                    traceback(&self.lua, &error)
                )
                .into());
            }
            return Ok(self.host.collector.take_ops());
        }
        let Ok(function) = self.lua.globals().get::<Function>("onHostCallback")
        else {
            return Ok(self.host.collector.take_ops());
        };
        let payload = from_json_with_bytes(&self.lua, &ctx.payload)?;
        if let Err(error) = function.call::<MultiValue>(payload) {
            return Err(format!(
                "{} host callback {id}: {}",
                self.entry,
                traceback(&self.lua, &error)
            )
            .into());
        }
        Ok(self.host.collector.take_ops())
    }
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn runtime_cache() -> &'static Mutex<HashMap<String, SharedRuntime>> {
    static CACHE: OnceLock<Mutex<HashMap<String, SharedRuntime>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[expect(
    clippy::too_many_arguments,
    reason = "the strict flag keeps strict runtimes out of the auto cache"
)]
fn cache_key(ctx: &Context, name: &str, source: &str, strict: bool) -> String {
    let pointer = source.as_ptr() as usize;
    format!(
        "{}:{}:{name}:{pointer:x}:{}:{strict}",
        ctx.entry,
        ctx.data_dir.to_string_lossy(),
        source.len()
    )
}

fn traceback(lua: &Lua, error: &mlua::Error) -> String {
    let message = error.to_string();
    let function = lua.load("return debug.traceback").eval::<Function>();
    match function {
        Ok(function) => {
            function.call::<String>(message.clone()).unwrap_or(message)
        }
        Err(_) => message,
    }
}
fn normalize_require(base: &str, name: &str) -> String {
    let joined = if name.starts_with('.') {
        Path::new(base)
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(name)
    } else {
        PathBuf::from(name)
    };
    let mut parts: Vec<String> = Vec::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            Component::Normal(value) => {
                parts.push(value.to_string_lossy().to_string());
            }
            Component::RootDir => {}
            Component::Prefix(_) => {}
        }
    }
    parts.join("/")
}
fn defines_host_callback(lua: &Lua) -> bool {
    lua.globals()
        .get::<LuaValue>("onHostCallback")
        .is_ok_and(|value| !matches!(value, LuaValue::Nil))
}

pub fn dispatch(ctx: &Context, sources: &[(&str, &str)]) -> Result<Vec<Op>> {
    let entry = ctx.entry.clone();
    dispatch_named(ctx, sources, &entry, false)
}

pub fn dispatch_strict(
    ctx: &Context,
    sources: &[(&str, &str)],
) -> Result<Vec<Op>> {
    let entry = ctx.entry.clone();
    dispatch_named(ctx, sources, &entry, true)
}

pub fn dispatch_entry(
    ctx: &Context,
    sources: &[(&str, &str)],
    entry: &str,
) -> Result<Vec<Op>> {
    dispatch_named(ctx, sources, entry, false)
}

pub fn dispatch_entry_strict(
    ctx: &Context,
    sources: &[(&str, &str)],
    entry: &str,
) -> Result<Vec<Op>> {
    dispatch_named(ctx, sources, entry, true)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the strict flag selects the runtime contract"
)]
fn dispatch_named(
    ctx: &Context,
    sources: &[(&str, &str)],
    entry: &str,
    strict: bool,
) -> Result<Vec<Op>> {
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    let Some((name, source)) = entry_source(entry, sources) else {
        return Err(format!("unknown entry {entry}").into());
    };
    let key = cache_key(ctx, name, source, strict);
    let cache = runtime_cache();
    let runtime = {
        let mut cache = locked(cache);
        if let Some(runtime) = cache.get(&key) {
            Arc::clone(runtime)
        } else {
            let (runtime, ops) =
                Runtime::start(ctx, sources, (name, source), strict)?;
            let runtime = Arc::new(Mutex::new(runtime));
            cache.insert(key, Arc::clone(&runtime));
            return Ok(ops);
        }
    };

    locked(&runtime).dispatch(ctx)
}

fn register_sources(lua: &Lua, sources: &[(&str, &str)]) -> mlua::Result<()> {
    let globals = lua.globals();
    let package = match globals.get::<mlua::Table>("package") {
        Ok(table) => table,
        Err(_) => {
            let table = lua.create_table()?;
            globals.set("package", table.clone())?;
            table
        }
    };
    let preload = match package.get::<mlua::Table>("preload") {
        Ok(table) => table,
        Err(_) => {
            let table = lua.create_table()?;
            package.set("preload", table.clone())?;
            let loaders = match package.get::<mlua::Table>("loaders") {
                Ok(table) => table,
                Err(_) => {
                    let table =
                        lua.create_sequence_from::<Vec<LuaValue>>(Vec::new())?;
                    package.set("loaders", table.clone())?;
                    table
                }
            };
            let loader = lua.create_function(|lua, name: String| {
                let package = lua.globals().get::<mlua::Table>("package")?;
                let preload = package.get::<mlua::Table>("preload")?;
                match preload.get::<mlua::Function>(name) {
                    Ok(function) => Ok(LuaValue::Function(function)),
                    Err(_) => Ok(LuaValue::Nil),
                }
            })?;
            loaders.raw_insert(1, loader)?;
            globals.set(
                "require",
                lua.create_function(|lua, name: String| {
                    let source = lua
                        .load("return debug.getinfo(2, 'S').source")
                        .eval::<String>()
                        .unwrap_or_default();
                    let base = source.trim_start_matches('@');
                    let resolved = normalize_require(base, &name);
                    let candidates = [
                        resolved.clone(),
                        resolved
                            .strip_suffix(".luau")
                            .unwrap_or(&resolved)
                            .to_owned(),
                        name.clone(),
                    ];
                    let package =
                        lua.globals().get::<mlua::Table>("package")?;
                    let preload = package.get::<mlua::Table>("preload")?;
                    let loaded = match package.get::<mlua::Table>("loaded") {
                        Ok(table) => table,
                        Err(_) => {
                            let table = lua.create_table()?;
                            package.set("loaded", table.clone())?;
                            table
                        }
                    };
                    if let Ok(cached) =
                        loaded.get::<LuaValue>(resolved.as_str())
                        && !matches!(cached, LuaValue::Nil)
                    {
                        return Ok(cached);
                    }

                    let mut function = None;
                    for candidate in &candidates {
                        if let Ok(value) =
                            preload.get::<mlua::Function>(candidate.clone())
                        {
                            function = Some(value);
                            break;
                        }
                    }
                    let Some(function) = function else {
                        return Ok(LuaValue::Nil);
                    };
                    let value = function.call::<LuaValue>(())?;
                    loaded.set(resolved, value.clone())?;
                    Ok(value)
                })?,
            )?;
            table
        }
    };
    for (name, source) in sources {
        let source = (*source).to_owned();
        let chunk_name = (*name).to_owned();
        let loader = lua.create_function(move |lua, _: LuaValue| {
            lua.load(&source).set_name(&chunk_name).eval::<LuaValue>()
        })?;
        for alias in source_names(name) {
            preload.set(alias.clone(), loader.clone())?;
            if !alias.starts_with("./") {
                preload.set(format!("./{alias}"), loader.clone())?;
            }
        }
    }
    Ok(())
}

fn source_names(name: &str) -> Vec<String> {
    let trimmed = name.trim_start_matches("./");
    let bare = trimmed.strip_suffix(".luau").unwrap_or(trimmed);
    let mut names = vec![name.to_owned()];
    for alias in [trimmed, bare] {
        if !alias.is_empty() && !names.iter().any(|item| item == alias) {
            names.push(alias.to_owned());
        }
    }
    if let Some(base) = bare.rsplit('/').next()
        && !base.is_empty()
    {
        let base_luau = format!("{base}.luau");
        for alias in [base, base_luau.as_str()] {
            if !names.iter().any(|item| item == alias) {
                names.push(alias.to_owned());
            }
        }
    }
    names
}
struct EntryMatch<'a> {
    score: u8,
    index: usize,
    name: &'a str,
    source: &'a str,
}

fn entry_source<'a>(
    entry: &str,
    sources: &'a [(&str, &'a str)],
) -> Option<(&'a str, &'a str)> {
    let entry = entry.trim_start_matches("./");
    let bare_entry = entry.strip_suffix(".luau").unwrap_or(entry);
    let mut best: Option<EntryMatch<'a>> = None;

    for (index, (name, source)) in sources.iter().enumerate() {
        let trimmed = name.trim_start_matches("./");
        let bare_name = trimmed.strip_suffix(".luau").unwrap_or(trimmed);
        let base_name = bare_name.rsplit('/').next().unwrap_or(bare_name);
        let score = if *name == entry
            || trimmed == entry
            || bare_name == entry
            || base_name == entry
        {
            3
        } else if trimmed == bare_entry
            || bare_name == bare_entry
            || base_name == bare_entry
        {
            2
        } else {
            continue;
        };
        if best.as_ref().is_none_or(|current| {
            score > current.score
                || (score == current.score && index < current.index)
        }) {
            best = Some(EntryMatch {
                score,
                index,
                name,
                source,
            });
        }
    }

    best.map(|found| (found.name, found.source))
}
