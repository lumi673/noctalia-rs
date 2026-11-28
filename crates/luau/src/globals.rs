use mlua::{
    Function, IntoLuaMulti, Lua, MultiValue, Result as LuaResult, Table,
    Value as LuaValue, Variadic,
};
use noctalia_rs::{ByteBuf, Op};
use serde_json::{Value as JsonValue, json};
use std::fs;
use std::sync::Arc;

use crate::host::Host;
use crate::value::{
    Convert, FunctionRole, as_string, from_json, from_json_with_bytes, params,
    string_arg,
};

type Args = Variadic<LuaValue>;

fn host(lua: &Lua) -> LuaResult<Arc<Host>> {
    lua.app_data_ref::<Arc<Host>>()
        .map(|value| Arc::clone(&value))
        .ok_or_else(|| mlua::Error::runtime("missing noctalia host"))
}

fn function<R, F>(lua: &Lua, func: F) -> LuaResult<Function>
where
    F: Fn(&Lua, Args) -> LuaResult<R> + Send + 'static,
    R: IntoLuaMulti,
{
    lua.create_function(func)
}

macro_rules! set_fn {
    ($lua:expr, $table:expr, $name:expr, $func:expr) => {
        $table.set($name, function($lua, $func)?)
    };
}

struct Fallback {
    root: &'static str,
    prefix: Vec<String>,
}

impl Fallback {
    fn new(root: &'static str, prefix: &[&str]) -> Self {
        Self {
            root,
            prefix: prefix.iter().map(|item| (*item).to_owned()).collect(),
        }
    }
}

macro_rules! fallback {
    ($lua:expr, $table:expr, $root:expr, $prefix:expr) => {
        fallback($lua, $table, &Fallback::new($root, $prefix))
    };
}

fn fallback(lua: &Lua, table: &Table, spec: &Fallback) -> LuaResult<()> {
    let prefix = spec.prefix.clone();
    let root = spec.root;
    let index =
        lua.create_function(move |lua, (_, key): (LuaValue, LuaValue)| {
            let mut path = prefix.clone();
            path.push(as_string(lua, key));
            let full = format!("{}.{}", root, path.join("."));
            let member = full.rsplit_once('.').and_then(|(root, name)| {
                noctalia_rs::api::find(root, name).copied()
            });
            if root == "layer" && member.is_none() {
                return Ok(LuaValue::Nil);
            }
            let call = function(lua, move |lua, args: Args| {
                let host = host(lua)?;
                let runtime = runtime(lua, &host);
                let values = runtime.json_args(args.to_vec())?;
                let parts = path.iter().map(String::as_str).collect::<Vec<_>>();
                match member.map(|member| member.kind) {
                    Some(noctalia_rs::api::MemberKind::Value)
                    | Some(noctalia_rs::api::MemberKind::Pair) => {
                        runtime.query(root, &parts, values)
                    }
                    Some(noctalia_rs::api::MemberKind::Bool) => {
                        host.collector
                            .emit(Op::host_call(root, &parts, values));
                        Ok(MultiValue::from_vec(vec![LuaValue::Boolean(true)]))
                    }
                    Some(noctalia_rs::api::MemberKind::Void) => {
                        host.collector
                            .emit(Op::host_call(root, &parts, values));
                        Ok(MultiValue::from_vec(vec![LuaValue::Nil]))
                    }
                    _ => {
                        host.collector
                            .emit(Op::host_call(root, &parts, values));
                        Ok(MultiValue::from_vec(vec![LuaValue::Nil]))
                    }
                }
            })?;
            Ok(LuaValue::Function(call))
        })?;
    let metatable = lua.create_table()?;
    metatable.set("__index", index)?;
    table.set_metatable(Some(metatable));
    Ok(())
}

struct Runtime<'a> {
    lua: &'a Lua,
    host: &'a Host,
}

impl Runtime<'_> {
    fn json_args(&self, args: Vec<LuaValue>) -> LuaResult<Vec<JsonValue>> {
        args.into_iter()
            .map(|value| {
                self.reject_markers(&value, 0)?;
                host_converter(self.lua, self.host).to_json(value)
            })
            .collect()
    }

    /// Fails a strict call that carries a bridge marker table.
    ///
    /// Plugin API 57 takes a function for a callback, so a strict entry must
    /// pass a closure. A table with `__host_callback` or `__call` is the
    /// compiler's bridge form, which strict emit does not produce.
    fn reject_markers(&self, value: &LuaValue, depth: usize) -> LuaResult<()> {
        const MAX_DEPTH: usize = 32;
        if !self.host.strict() || depth > MAX_DEPTH {
            return Ok(());
        }
        let LuaValue::Table(table) = value else {
            return Ok(());
        };
        for key in ["__host_callback", "__call"] {
            if let LuaValue::String(_) = table.raw_get::<LuaValue>(key)? {
                return Err(mlua::Error::runtime(format!(
                    "plugin API 57 takes a function here; strict emit passed the `{key}` marker table"
                )));
            }
        }
        if let LuaValue::String(_) =
            table.raw_get::<LuaValue>(noctalia_rs::bytes::BYTES_KEY)?
        {
            return Err(mlua::Error::runtime(format!(
                "plugin API 57 takes a Luau byte string; strict emit passed the `{}` marker table",
                noctalia_rs::bytes::BYTES_KEY
            )));
        }
        for pair in table.clone().pairs::<LuaValue, LuaValue>() {
            let (_, item) = pair?;
            self.reject_markers(&item, depth + 1)?;
        }
        Ok(())
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one call carries the full query envelope"
    )]
    fn query(
        &self,
        root: &str,
        path: &[&str],
        args: Vec<JsonValue>,
    ) -> LuaResult<MultiValue> {
        if self.host.strict()
            && self.host.query_dir().is_none()
            && let Some(result) = self.strict_response(root, path, &args)?
        {
            return Ok(result);
        }
        if self.host.query_dir().is_none() {
            return Ok(MultiValue::from_vec(vec![LuaValue::Nil]));
        }
        let id = self.host.collector.callback_id();
        let response = self.host.response_path(&id);
        if let Some(parent) = response.parent() {
            let _ = fs::create_dir_all(parent);
        }
        self.host.collector.emit(Op::query(
            &id,
            root,
            path,
            args,
            response.to_string_lossy().to_string(),
        ));
        let Some(value) = self.host.read_response(&response) else {
            return Ok(MultiValue::from_vec(vec![LuaValue::Nil]));
        };
        let mut result = vec![from_json_with_bytes(
            self.lua,
            value.get("value").unwrap_or(&JsonValue::Null),
        )?];
        if let Some(error) = value.get("error")
            && !error.is_null()
        {
            result.push(from_json(self.lua, error)?);
        }
        Ok(MultiValue::from_vec(result))
    }

    /// Serves `readFile` and `removeFile` from the strict response registry.
    ///
    /// A strict entry has no helper and no response files. A query result is
    /// stored by path when the entry writes it, so the read that follows finds
    /// the same text the bridge would have written. A path that is not in the
    /// registry falls through to the host.
    #[expect(
        clippy::too_many_arguments,
        reason = "one call carries the full query envelope"
    )]
    fn strict_response(
        &self,
        root: &str,
        path: &[&str],
        args: &[JsonValue],
    ) -> LuaResult<Option<MultiValue>> {
        if root != "noctalia" || path.len() != 1 {
            return Ok(None);
        }
        let name = path.first().copied().unwrap_or_default();
        let string_at = |index: usize| {
            args.get(index)
                .and_then(JsonValue::as_str)
                .map(str::to_owned)
        };
        let pair = |ok: bool| {
            Some(MultiValue::from_vec(vec![
                LuaValue::Boolean(ok),
                LuaValue::Nil,
            ]))
        };
        match name {
            "writeFile" => {
                let (Some(target), Some(body)) = (string_at(0), string_at(1))
                else {
                    return Ok(None);
                };
                self.host.store_response(&target, body);
                Ok(pair(true))
            }
            "readFile" => {
                let Some(target) = string_at(0) else {
                    return Ok(None);
                };
                let Some(body) = self.host.response(&target) else {
                    return Ok(None);
                };
                Ok(Some(MultiValue::from_vec(vec![
                    LuaValue::String(self.lua.create_string(body)?),
                    LuaValue::Nil,
                ])))
            }
            "removeFile" => {
                let Some(target) = string_at(0) else {
                    return Ok(None);
                };
                if !self.host.has_response(&target) {
                    return Ok(None);
                }
                let _ = self.host.take_response(&target);
                Ok(pair(true))
            }
            "renameFile" => {
                let (Some(from), Some(to)) = (string_at(0), string_at(1))
                else {
                    return Ok(None);
                };
                let Some(body) = self.host.take_response(&from) else {
                    return Ok(None);
                };
                self.host.store_response(&to, body);
                Ok(pair(true))
            }
            "fileExists" => {
                let Some(target) = string_at(0) else {
                    return Ok(None);
                };
                if self.host.has_response(&target) {
                    return Ok(Some(MultiValue::from_vec(vec![
                        LuaValue::Boolean(true),
                    ])));
                }
                Ok(None)
            }
            _ => Ok(None),
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "one call carries the full query envelope"
    )]
    fn query_or_snapshot(
        &self,
        root: &str,
        path: &[&str],
        args: Vec<JsonValue>,
        fallback: JsonValue,
    ) -> LuaResult<MultiValue> {
        let result = self.query(root, path, args)?;
        let first = result.iter().next().cloned().unwrap_or(LuaValue::Nil);
        if matches!(first, LuaValue::Nil) {
            return Ok(MultiValue::from_vec(vec![from_json(
                self.lua, &fallback,
            )?]));
        }
        Ok(result)
    }

    fn json(&self, args: &[LuaValue], index: usize) -> LuaResult<JsonValue> {
        let value = args.get(index).cloned().unwrap_or(LuaValue::Nil);
        self.reject_markers(&value, 0)?;
        host_converter(self.lua, self.host).to_json(value)
    }

    fn callback_marker(
        &self,
        args: &[LuaValue],
        index: usize,
    ) -> LuaResult<Option<String>> {
        let value = match args.get(index) {
            Some(value) if !matches!(value, LuaValue::Nil) => value.clone(),
            _ => return Ok(None),
        };
        if self.host.strict() && !matches!(value, LuaValue::Function(_)) {
            return Err(mlua::Error::runtime(
                "plugin API 57 takes a function here; strict emit passed a table or a name",
            ));
        }
        let json = host_converter(self.lua, self.host).to_json(value)?;
        Ok(json
            .get("__host_callback")
            .and_then(JsonValue::as_str)
            .map(str::to_owned))
    }

    fn callback_id(
        &self,
        args: &[LuaValue],
        index: usize,
    ) -> LuaResult<String> {
        Ok(self
            .callback_marker(args, index)?
            .unwrap_or_else(|| self.host.collector.callback_id()))
    }
}

fn runtime<'a>(lua: &'a Lua, host: &'a Host) -> Runtime<'a> {
    Runtime { lua, host }
}

fn call_converter<'a>(lua: &'a Lua, host: &'a Host) -> Convert<'a> {
    Convert::new(lua, FunctionRole::Call, &host.collector)
}

fn host_converter<'a>(lua: &'a Lua, host: &'a Host) -> Convert<'a> {
    Convert::new(lua, FunctionRole::HostCallback, &host.collector)
}

fn json_rows(value: JsonValue) -> Vec<JsonValue> {
    match value {
        JsonValue::Array(values) => values,
        JsonValue::Object(values) if values.is_empty() => Vec::new(),
        JsonValue::Null => Vec::new(),
        other => vec![other],
    }
}

fn integer_arg(lua: &Lua, args: &[LuaValue], index: usize) -> mlua::Integer {
    args.get(index)
        .and_then(|value| lua.coerce_integer(value.clone()).ok().flatten())
        .unwrap_or_default()
}

fn integer_value(value: i64) -> LuaValue {
    LuaValue::Integer(
        mlua::Integer::try_from(value).unwrap_or(mlua::Integer::MAX),
    )
}

fn bool_arg(args: &[LuaValue], index: usize, default: bool) -> bool {
    match args.get(index) {
        Some(LuaValue::Nil) | None => default,
        Some(LuaValue::Boolean(value)) => *value,
        Some(_) => true,
    }
}

/// Whether a command can be handed to the host.
///
/// `runAsync` takes a string or an argv table, and `runStream` takes a
/// command string. Anything else is not accepted, so the call reports false
/// instead of starting a process it cannot describe.
fn command_accepted(command: &JsonValue) -> bool {
    match command {
        JsonValue::String(value) => !value.is_empty(),
        JsonValue::Array(values) => {
            !values.is_empty()
                && values.iter().all(|value| {
                    value.as_str().is_some_and(|text| !text.is_empty())
                })
        }
        _ => false,
    }
}

fn json_arg(
    lua: &Lua,
    args: &[LuaValue],
    index: usize,
) -> LuaResult<JsonValue> {
    let host = host(lua)?;
    runtime(lua, &host).json(args, index)
}

fn callback_marker(
    lua: &Lua,
    args: &[LuaValue],
    index: usize,
) -> LuaResult<Option<String>> {
    let host = host(lua)?;
    runtime(lua, &host).callback_marker(args, index)
}

fn callback_id(
    lua: &Lua,
    args: &[LuaValue],
    index: usize,
) -> LuaResult<String> {
    let host = host(lua)?;
    runtime(lua, &host).callback_id(args, index)
}

fn to_json(
    lua: &Lua,
    value: LuaValue,
    role: FunctionRole,
) -> LuaResult<JsonValue> {
    let host = host(lua)?;
    Convert::new(lua, role, &host.collector).to_json(value)
}

fn ui_node(lua: &Lua, value: LuaValue) -> LuaResult<noctalia_rs::ui::UiNode> {
    let host = host(lua)?;
    if host.strict() {
        return host_converter(lua, &host).ui_node(value);
    }
    call_converter(lua, &host).ui_node(value)
}

fn make_json_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;
    set_fn!(lua, &table, "encode", |lua, args: Args| {
        let values = args.to_vec();
        let value = to_json(
            lua,
            values.first().cloned().unwrap_or(LuaValue::Nil),
            FunctionRole::Call,
        )?;
        let text =
            serde_json::to_string(&value).map_err(mlua::Error::external)?;
        Ok(MultiValue::from_vec(vec![
            LuaValue::String(lua.create_string(text)?),
            LuaValue::Nil,
        ]))
    })?;
    set_fn!(lua, &table, "decode", |lua, args: Args| {
        let values = args.to_vec();
        let text = string_arg(lua, &values, 0);
        match serde_json::from_str::<JsonValue>(&text) {
            Ok(value) => Ok(MultiValue::from_vec(vec![
                from_json(lua, &value)?,
                LuaValue::Nil,
            ])),
            Err(error) => Ok(MultiValue::from_vec(vec![
                LuaValue::Nil,
                LuaValue::String(lua.create_string(error.to_string())?),
            ])),
        }
    })?;
    set_fn!(lua, &table, "dumps", |lua, args: Args| {
        lua.globals()
            .get::<Table>("json")?
            .get::<Function>("encode")?
            .call::<MultiValue>(args)
    })?;
    set_fn!(lua, &table, "loads", |lua, args: Args| {
        lua.globals()
            .get::<Table>("json")?
            .get::<Function>("decode")?
            .call::<MultiValue>(args)
    })?;
    fallback!(lua, &table, "noctalia", &["json"])?;
    Ok(table)
}

fn make_state_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;
    set_fn!(lua, &table, "get", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let key = string_arg(lua, &values, 0);
        if host.snapshot.has_state(&key) {
            return from_json(lua, &host.snapshot.state(&key));
        }
        let result = runtime(lua, &host).query_or_snapshot(
            "noctalia",
            &["state", "get"],
            vec![JsonValue::String(key)],
            JsonValue::Null,
        )?;
        Ok(result.into_iter().next().unwrap_or(LuaValue::Nil))
    })?;
    set_fn!(lua, &table, "set", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let key = string_arg(lua, &values, 0);
        let value = json_arg(lua, &values, 1)?;
        host.collector.emit(Op::state(&key, value));
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "watch", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let key = string_arg(lua, &values, 0);
        let id = callback_id(lua, &values, 1)?;
        host.collector.emit(Op::state_watch(&key, &id));
        Ok(LuaValue::Nil)
    })?;
    fallback!(lua, &table, "noctalia", &["state"])?;
    Ok(table)
}

fn make_string_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;
    set_fn!(lua, &table, "trim", |lua, args: Args| {
        let values = args.to_vec();
        let text = string_arg(lua, &values, 0);
        Ok(LuaValue::String(lua.create_string(text.trim())?))
    })?;
    set_fn!(lua, &table, "urlEncode", |lua, args: Args| {
        let values = args.to_vec();
        let text = string_arg(lua, &values, 0);
        Ok(LuaValue::String(lua.create_string(percent_encode(&text))?))
    })?;
    set_fn!(lua, &table, "urlDecode", |lua, args: Args| {
        let values = args.to_vec();
        let text = string_arg(lua, &values, 0);
        Ok(LuaValue::String(lua.create_string(percent_decode(&text))?))
    })?;
    fallback!(lua, &table, "noctalia", &["string"])?;
    Ok(table)
}

fn make_noctalia_layer_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;
    set_fn!(lua, &table, "show", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        host.collector.emit(Op::LayerShow {
            id: string_arg(lua, &values, 0),
        });
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "hide", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        host.collector.emit(Op::LayerHide {
            id: string_arg(lua, &values, 0),
        });
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "configure", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let id = string_arg(lua, &values, 0);
        let props = json_arg(lua, &values, 1)?;
        host.collector.emit(Op::LayerConfigure { id, props });
        Ok(LuaValue::Nil)
    })?;
    fallback!(lua, &table, "noctalia", &["layer"])?;
    Ok(table)
}

fn make_global_layer_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;
    set_fn!(lua, &table, "render", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let tree =
            ui_node(lua, values.first().cloned().unwrap_or(LuaValue::Nil))?;
        host.collector.emit(Op::LayerRender { tree });
        Ok(LuaValue::Nil)
    })?;
    fallback!(lua, &table, "layer", &[])?;
    Ok(table)
}

fn make_noctalia_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;
    let json = make_json_table(lua)?;
    let state = make_state_table(lua)?;
    let strings = make_string_table(lua)?;
    let layer = make_noctalia_layer_table(lua)?;
    table.set("json", json.clone())?;
    table.set("state", state.clone())?;
    table.set("string", strings)?;
    table.set("layer", layer)?;
    let sound = lua.create_table()?;
    fallback!(lua, &sound, "noctalia", &["sound"])?;
    table.set("sound", sound)?;
    let idle = lua.create_table()?;
    fallback!(lua, &idle, "noctalia", &["idle"])?;
    table.set("idle", idle)?;
    let umbriel = lua.create_table()?;
    fallback!(lua, &umbriel, "noctalia", &["umbriel"])?;
    table.set("umbriel", umbriel)?;

    let osd = lua.create_table()?;
    set_fn!(lua, &osd, "show", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let icon = string_arg(lua, &values, 0);
        let value = string_arg(lua, &values, 1);
        let extra = if values
            .get(2)
            .is_some_and(|value| !matches!(value, LuaValue::Nil))
        {
            Some(json_arg(lua, &values, 2)?)
        } else {
            None
        };
        host.collector.emit(Op::osd(&icon, &value, extra));
        Ok(LuaValue::Nil)
    })?;
    fallback!(lua, &osd, "noctalia", &["osd"])?;
    table.set("osd", osd)?;

    let compositor = lua.create_table()?;
    set_fn!(lua, &compositor, "name", |lua, _: Args| {
        let host = host(lua)?;
        runtime(lua, &host).query_or_snapshot(
            "noctalia",
            &["compositor", "name"],
            Vec::new(),
            host.snapshot.host_path("compositor.name"),
        )
    })?;
    fallback!(lua, &compositor, "noctalia", &["compositor"])?;
    table.set("compositor", compositor)?;

    let nightlight = lua.create_table()?;
    set_fn!(lua, &nightlight, "state", |lua, _: Args| {
        let host = host(lua)?;
        runtime(lua, &host).query_or_snapshot(
            "noctalia",
            &["nightlight", "state"],
            Vec::new(),
            host.snapshot.host_path("nightlight.state"),
        )
    })?;
    fallback!(lua, &nightlight, "noctalia", &["nightlight"])?;
    table.set("nightlight", nightlight)?;

    let media = lua.create_table()?;
    fallback!(lua, &media, "noctalia", &["media"])?;
    table.set("media", media)?;

    set_fn!(lua, &table, "log", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        host.collector.emit(Op::log(string_arg(lua, &values, 0)));
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "notify", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let title = string_arg(lua, &values, 0);
        let body = string_arg(lua, &values, 1);
        let options = if matches!(values.get(2), None | Some(LuaValue::Nil)) {
            None
        } else {
            Some(json_arg(lua, &values, 2)?)
        };
        host.collector
            .emit(Op::notify_with_options(&title, &body, options));
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "notifyError", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let title = string_arg(lua, &values, 0);
        let body = string_arg(lua, &values, 1);
        let options = if matches!(values.get(2), None | Some(LuaValue::Nil)) {
            None
        } else {
            Some(json_arg(lua, &values, 2)?)
        };
        host.collector
            .emit(Op::notify_error_with_options(&title, &body, options));
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "setUpdateInterval", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let ms = u64::try_from(integer_arg(lua, &values, 0).max(0))
            .unwrap_or(u64::MAX);
        host.collector.emit(Op::set_interval(ms));
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "togglePanel", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        host.collector.emit(Op::TogglePanel {
            id: string_arg(lua, &values, 0),
        });
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "openSettings", |lua, _: Args| {
        host(lua)?.collector.emit(Op::OpenSettings);
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "copyToClipboard", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        if host.strict()
            && let Some(LuaValue::Table(table)) = values.first()
            && let LuaValue::String(_) =
                table.raw_get::<LuaValue>(noctalia_rs::bytes::BYTES_KEY)?
        {
            return Err(mlua::Error::runtime(format!(
                "plugin API 57 takes a Luau byte string; strict emit passed the `{}` marker table",
                noctalia_rs::bytes::BYTES_KEY
            )));
        }
        let mime = string_arg(lua, &values, 1);
        let mime = if mime.is_empty() {
            "text/plain".to_owned()
        } else {
            mime
        };
        let text = string_arg(lua, &values, 0);
        match values.first().and_then(|value| match value {
            LuaValue::String(text) => Some(text.as_bytes().to_vec()),
            _ => None,
        }) {
            Some(bytes) if std::str::from_utf8(&bytes).is_err() => {
                host.collector
                    .emit(Op::copy_bytes(ByteBuf::new(bytes), &mime));
            }
            _ => host.collector.emit(Op::copy_with_mime(&text, &mime)),
        }
        Ok(LuaValue::Boolean(true))
    })?;
    set_fn!(lua, &table, "openPath", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        host.collector
            .emit(Op::open_path(&string_arg(lua, &values, 0)));
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "setWallpaper", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let first = string_arg(lua, &values, 0);
        let op = if matches!(values.get(1), None | Some(LuaValue::Nil)) {
            Op::set_wallpaper(&first)
        } else {
            Op::set_wallpaper_on(&first, &string_arg(lua, &values, 1))
        };
        host.collector.emit(op);
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "http", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let request = json_arg(lua, &values, 0)?;
        let callback = callback_marker(lua, &values, 1)?;
        if host.strict()
            && host
                .snapshot
                .setting("shell.offline_mode")
                .as_bool()
                .unwrap_or_default()
        {
            return Ok(LuaValue::Boolean(false));
        }
        host.collector.emit(Op::http(request, callback.as_deref()));
        Ok(LuaValue::Boolean(true))
    })?;
    set_fn!(lua, &table, "httpStream", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let request = json_arg(lua, &values, 0)?;
        let on_line = json_arg(lua, &values, 1)?;
        let on_close = json_arg(lua, &values, 2)?;
        let result = runtime(lua, &host).query(
            "noctalia",
            &["httpStream"],
            vec![request, on_line, on_close],
        )?;
        let handle = result.into_iter().next().unwrap_or(LuaValue::Nil);
        if matches!(handle, LuaValue::Nil) {
            return Ok(LuaValue::Nil);
        }
        let table = lua.create_table()?;
        let stop = if host.strict() {
            function(lua, move |_: &Lua, _: Args| Ok(LuaValue::Nil))?
        } else {
            let handle = handle.clone();
            let host = Arc::clone(&host);
            function(lua, move |lua, _: Args| {
                let token = to_json(lua, handle.clone(), FunctionRole::Call)?;
                host.collector.emit(Op::host_call(
                    "__noctalia_rs",
                    &["httpStreamStop"],
                    vec![token],
                ));
                Ok(LuaValue::Nil)
            })?
        };
        table.set("stop", stop)?;
        Ok(LuaValue::Table(table))
    })?;
    set_fn!(lua, &table, "runAsync", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let command = json_arg(lua, &values, 0)?;
        let callback = callback_marker(lua, &values, 1)?;
        let timeout = values.get(2).cloned().unwrap_or(LuaValue::Nil);
        let timeout = if matches!(timeout, LuaValue::Nil) {
            None
        } else {
            lua.coerce_number(timeout).ok().flatten()
        };
        if host.strict() && !command_accepted(&command) {
            return Ok(LuaValue::Boolean(false));
        }
        host.collector.emit(Op::run_with_timeout(
            command,
            callback.as_deref(),
            timeout,
        ));
        Ok(LuaValue::Boolean(true))
    })?;
    set_fn!(lua, &table, "runStream", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let command = json_arg(lua, &values, 0)?;
        let id = callback_id(lua, &values, 1)?;
        if host.strict() && !command_accepted(&command) {
            return Ok(LuaValue::Boolean(false));
        }
        host.collector.emit(Op::run_stream(command, &id));
        Ok(LuaValue::Boolean(true))
    })?;

    set_fn!(lua, &table, "getConfig", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        from_json(lua, &host.snapshot.config(&string_arg(lua, &values, 0)))
    })?;
    set_fn!(lua, &table, "getSetting", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let key = string_arg(lua, &values, 0);
        runtime(lua, &host).query_or_snapshot(
            "noctalia",
            &["getSetting"],
            vec![JsonValue::String(key.clone())],
            host.snapshot.setting(&key),
        )
    })?;
    set_fn!(lua, &table, "getColor", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let role = string_arg(lua, &values, 0);
        runtime(lua, &host).query_or_snapshot(
            "noctalia",
            &["getColor"],
            vec![JsonValue::String(role.clone())],
            host.snapshot.color(&role),
        )
    })?;
    set_fn!(lua, &table, "nowMs", |lua, _: Args| {
        Ok(integer_value(host(lua)?.snapshot.now_ms()))
    })?;
    set_fn!(lua, &table, "isDarkMode", |lua, _: Args| {
        let host = host(lua)?;
        Ok(LuaValue::Boolean(
            host.snapshot
                .host_path("dark")
                .as_bool()
                .unwrap_or_default(),
        ))
    })?;
    set_fn!(lua, &table, "outputs", |lua, _: Args| {
        let host = host(lua)?;
        runtime(lua, &host).query_or_snapshot(
            "noctalia",
            &["outputs"],
            Vec::new(),
            host.snapshot.host_path("outputs"),
        )
    })?;
    set_fn!(lua, &table, "tr", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let key = string_arg(lua, &values, 0);
        let subst = values.get(1).cloned().unwrap_or(LuaValue::Nil);
        let json = to_json(lua, subst, FunctionRole::Call)?;
        let fallback = JsonValue::String(host.tr(&key, &params(&json)));
        runtime(lua, &host).query_or_snapshot(
            "noctalia",
            &["tr"],
            vec![JsonValue::String(key), json],
            fallback,
        )
    })?;
    set_fn!(lua, &table, "trp", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let key = string_arg(lua, &values, 0);
        let count = integer_arg(lua, &values, 1);
        let subst = values.get(2).cloned().unwrap_or(LuaValue::Nil);
        let json = to_json(lua, subst, FunctionRole::Call)?;
        let mut pairs = params(&json);
        pairs.push(("count".to_owned(), count.to_string()));
        let fallback = JsonValue::String(host.tr(&key, &pairs));
        runtime(lua, &host).query_or_snapshot(
            "noctalia",
            &["trp"],
            vec![JsonValue::String(key), json!(count), json],
            fallback,
        )
    })?;
    set_fn!(lua, &table, "focusedOutputName", |lua, _: Args| {
        let host = host(lua)?;
        runtime(lua, &host).query_or_snapshot(
            "noctalia",
            &["focusedOutputName"],
            Vec::new(),
            host.snapshot.host_path("focused_output_name"),
        )
    })?;
    set_fn!(lua, &table, "clipboardText", |lua, _: Args| {
        let host = host(lua)?;
        from_json(lua, &host.snapshot.host_path("clipboard"))
    })?;
    set_fn!(lua, &table, "calendarEvents", |lua, _: Args| {
        let host = host(lua)?;
        from_json(lua, &host.snapshot.host_path("calendar"))
    })?;
    set_fn!(lua, &table, "mediaPlayer", |lua, _: Args| {
        let host = host(lua)?;
        from_json(lua, &host.snapshot.host_path("media"))
    })?;
    set_fn!(lua, &table, "systemStats", |lua, _: Args| {
        let host = host(lua)?;
        from_json(lua, &host.snapshot.host_path("stats"))
    })?;
    set_fn!(lua, &table, "pluginDir", |lua, _: Args| {
        let host = host(lua)?;
        let fallback = JsonValue::String(
            host.snapshot.data_dir().to_string_lossy().into_owned(),
        );
        runtime(lua, &host).query_or_snapshot(
            "noctalia",
            &["pluginDir"],
            Vec::new(),
            fallback,
        )
    })?;
    set_fn!(lua, &table, "pluginDataDir", |lua, _: Args| {
        let host = host(lua)?;
        let fallback = JsonValue::Null;
        runtime(lua, &host).query_or_snapshot(
            "noctalia",
            &["pluginDataDir"],
            Vec::new(),
            fallback,
        )
    })?;
    fallback!(lua, &table, "noctalia", &[])?;
    Ok(table)
}

fn make_bar_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;
    set_fn!(lua, &table, "isVertical", |lua, _: Args| {
        let host = host(lua)?;
        Ok(LuaValue::Boolean(
            host.snapshot
                .host_path("bar.vertical")
                .as_bool()
                .unwrap_or_default(),
        ))
    })?;
    set_fn!(lua, &table, "outputName", |lua, _: Args| {
        let host = host(lua)?;
        from_json(lua, &host.snapshot.host_path("bar.output_name"))
    })?;
    set_fn!(lua, &table, "setText", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        host.collector.emit(Op::BarText {
            text: string_arg(lua, &values, 0),
        });
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "setColor", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        host.collector.emit(Op::BarColor {
            role: string_arg(lua, &values, 0),
        });
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "setVisible", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        host.collector.emit(Op::BarVisible {
            visible: bool_arg(&values, 0, true),
        });
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "setTooltip", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let value = json_arg(lua, &values, 0)?;
        host.collector.emit(Op::BarTooltip { value });
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "clearTooltip", |lua, _: Args| {
        host(lua)?.collector.emit(Op::BarTooltip {
            value: JsonValue::Null,
        });
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "setImage", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let path = string_arg(lua, &values, 0);
        let preserve = bool_arg(&values, 1, false);
        let width =
            u32::try_from(integer_arg(lua, &values, 2).max(0)).unwrap_or(0);
        let height =
            u32::try_from(integer_arg(lua, &values, 3).max(0)).unwrap_or(0);
        host.collector.emit(Op::BarImage {
            path,
            preserve,
            width,
            height,
        });
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "render", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let tree =
            ui_node(lua, values.first().cloned().unwrap_or(LuaValue::Nil))?;
        host.collector.emit(Op::BarRender { tree });
        Ok(LuaValue::Nil)
    })?;
    fallback!(lua, &table, "barWidget", &[])?;
    Ok(table)
}

fn make_panel_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;
    set_fn!(lua, &table, "render", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let tree =
            ui_node(lua, values.first().cloned().unwrap_or(LuaValue::Nil))?;
        host.collector.emit(Op::PanelRender { tree });
        Ok(LuaValue::Nil)
    })?;
    fallback!(lua, &table, "panel", &[])?;
    Ok(table)
}

fn make_desktop_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;
    set_fn!(lua, &table, "render", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let tree =
            ui_node(lua, values.first().cloned().unwrap_or(LuaValue::Nil))?;
        host.collector.emit(Op::DesktopWidgetRender { tree });
        Ok(LuaValue::Nil)
    })?;
    fallback!(lua, &table, "desktopWidget", &[])?;
    Ok(table)
}

fn make_launcher_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;
    set_fn!(lua, &table, "setResults", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let text = string_arg(lua, &values, 0);
        let rows = json_rows(json_arg(lua, &values, 1)?);
        host.collector.emit(Op::LauncherResults { text, rows });
        Ok(LuaValue::Nil)
    })?;
    fallback!(lua, &table, "launcher", &[])?;
    Ok(table)
}

fn make_shortcut_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;
    fallback!(lua, &table, "shortcut", &[])?;
    Ok(table)
}

fn make_ui_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;
    let index =
        lua.create_function(move |lua, (_, key): (LuaValue, LuaValue)| {
            let kind = as_string(lua, key);
            let constructor = function(lua, move |lua, args: Args| {
                let values = args.to_vec();
                let props = values.first().cloned().unwrap_or(LuaValue::Nil);
                let props = if matches!(props, LuaValue::Nil) {
                    LuaValue::Table(lua.create_table()?)
                } else {
                    props
                };
                let children = values.get(1).cloned().unwrap_or(LuaValue::Nil);
                let children = if matches!(children, LuaValue::Nil) {
                    LuaValue::Table(lua.create_table()?)
                } else {
                    children
                };
                let node = lua.create_table()?;
                node.raw_set(
                    "kind",
                    LuaValue::String(lua.create_string(&kind)?),
                )?;
                node.raw_set("props", props)?;
                node.raw_set("children", children)?;
                Ok(LuaValue::Table(node))
            })?;
            Ok(LuaValue::Function(constructor))
        })?;
    let metatable = lua.create_table()?;
    metatable.set("__index", index)?;
    table.set_metatable(Some(metatable));
    Ok(table)
}

fn percent_encode(value: &str) -> String {
    let mut result = String::new();
    for byte in value.as_bytes() {
        if byte.is_ascii_alphanumeric()
            || matches!(*byte, b'-' | b'_' | b'.' | b'~')
        {
            result.push(char::from(*byte));
        } else {
            result.push_str(&format!("%{byte:02X}"));
        }
    }
    result
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let high = hex_value(bytes[index + 1]);
            let low = hex_value(bytes[index + 2]);
            if let (Some(high), Some(low)) = (high, low) {
                result.push(high * 16 + low);
                index += 3;
                continue;
            }
        }
        if bytes[index] == b'+' {
            result.push(b' ');
        } else {
            result.push(bytes[index]);
        }
        index += 1;
    }
    String::from_utf8_lossy(&result).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub fn register(lua: &Lua) -> LuaResult<()> {
    let globals = lua.globals();
    let noctalia = make_noctalia_table(lua)?;
    globals.set("noctalia", noctalia.clone())?;
    globals.set("json", noctalia.get::<Table>("json")?)?;
    globals.set("state", noctalia.get::<Table>("state")?)?;
    globals.set("barWidget", make_bar_table(lua)?)?;
    globals.set("panel", make_panel_table(lua)?)?;
    globals.set("desktopWidget", make_desktop_table(lua)?)?;
    globals.set("launcher", make_launcher_table(lua)?)?;
    globals.set("shortcut", make_shortcut_table(lua)?)?;
    globals.set("layer", make_global_layer_table(lua)?)?;
    globals.set("ui", make_ui_table(lua)?)?;
    Ok(())
}
