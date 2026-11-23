use chrono::Local;
use mlua::{
    Function, IntoLuaMulti, Lua, MultiValue, Result as LuaResult, Table,
    Value as LuaValue, Variadic,
};
use noctalia::Op;
use serde_json::Value as JsonValue;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

use crate::host::Host;
use crate::value::{
    Convert, FunctionRole, as_string, from_json, params, string_arg,
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
            let call = function(lua, move |lua, args: Args| {
                let host = host(lua)?;
                let values = runtime(lua, &host).json_args(args.to_vec())?;
                let path = path.iter().map(String::as_str).collect::<Vec<_>>();
                host.collector.emit(Op::host_call(root, &path, values));
                Ok(LuaValue::Nil)
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
            .map(|value| host_converter(self.lua, self.host).to_json(value))
            .collect()
    }

    fn json(&self, args: &[LuaValue], index: usize) -> LuaResult<JsonValue> {
        let value = args.get(index).cloned().unwrap_or(LuaValue::Nil);
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

fn expand_path(_host: &Host, path: &str) -> PathBuf {
    if let (Some(rest), Some(home)) =
        (path.strip_prefix("~/"), env::var_os("HOME"))
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(path)
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

fn ui_node(lua: &Lua, value: LuaValue) -> LuaResult<noctalia::ui::UiNode> {
    let host = host(lua)?;
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
        from_json(lua, &host.snapshot.state(&key))
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

fn make_layer_table(lua: &Lua, root: &'static str) -> LuaResult<Table> {
    let table = lua.create_table()?;
    let prefix: &[&str] = if root == "noctalia" { &["layer"] } else { &[] };
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
    set_fn!(lua, &table, "render", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let tree =
            ui_node(lua, values.first().cloned().unwrap_or(LuaValue::Nil))?;
        host.collector.emit(Op::LayerRender { tree });
        Ok(LuaValue::Nil)
    })?;
    fallback!(lua, &table, root, prefix)?;
    Ok(table)
}

fn make_noctalia_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;
    let json = make_json_table(lua)?;
    let state = make_state_table(lua)?;
    let strings = make_string_table(lua)?;
    let layer = make_layer_table(lua, "noctalia")?;
    table.set("json", json.clone())?;
    table.set("state", state.clone())?;
    table.set("string", strings)?;
    table.set("layer", layer)?;
    let sound = lua.create_table()?;
    fallback!(lua, &sound, "noctalia", &["sound"])?;
    table.set("sound", sound)?;

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
        match host.snapshot.host_path("compositor.name") {
            JsonValue::String(value) => {
                Ok(LuaValue::String(lua.create_string(value)?))
            }
            _ => Ok(LuaValue::Nil),
        }
    })?;
    fallback!(lua, &compositor, "noctalia", &["compositor"])?;
    table.set("compositor", compositor)?;

    let nightlight = lua.create_table()?;
    set_fn!(lua, &nightlight, "state", |lua, _: Args| {
        let host = host(lua)?;
        from_json(lua, &host.snapshot.host_path("nightlight.state"))
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
        host.collector.emit(Op::notify(&title, &body));
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "notifyError", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let title = string_arg(lua, &values, 0);
        let body = string_arg(lua, &values, 1);
        host.collector.emit(Op::notify_error(&title, &body));
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
    set_fn!(lua, &table, "removeFile", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let path = string_arg(lua, &values, 0);
        host.collector.emit(Op::RemoveFile { path });
        Ok(LuaValue::Boolean(true))
    })?;
    set_fn!(lua, &table, "copyToClipboard", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        host.collector.emit(Op::copy(&string_arg(lua, &values, 0)));
        Ok(LuaValue::Boolean(true))
    })?;
    set_fn!(lua, &table, "openPath", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        host.collector
            .emit(Op::open_path(&string_arg(lua, &values, 0)));
        Ok(LuaValue::Boolean(true))
    })?;
    set_fn!(lua, &table, "setWallpaper", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        host.collector
            .emit(Op::set_wallpaper(&string_arg(lua, &values, 0)));
        Ok(LuaValue::Boolean(true))
    })?;
    set_fn!(lua, &table, "http", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let request = json_arg(lua, &values, 0)?;
        let callback = callback_marker(lua, &values, 1)?;
        host.collector.emit(Op::http(request, callback.as_deref()));
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "runAsync", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let command = json_arg(lua, &values, 0)?;
        let callback = callback_marker(lua, &values, 1)?;
        host.collector.emit(Op::run(command, callback.as_deref()));
        Ok(LuaValue::Nil)
    })?;
    set_fn!(lua, &table, "runStream", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let command = json_arg(lua, &values, 0)?;
        let id = callback_id(lua, &values, 1)?;
        host.collector.emit(Op::run_stream(command, &id));
        Ok(LuaValue::Boolean(true))
    })?;
    set_fn!(lua, &table, "writeFile", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let path = string_arg(lua, &values, 0);
        let content = json_arg(lua, &values, 1)?;
        host.collector.emit(Op::host_call(
            "noctalia",
            &["writeFile"],
            vec![JsonValue::String(path), content],
        ));
        Ok(LuaValue::Boolean(true))
    })?;
    set_fn!(lua, &table, "mkdirAll", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let path = string_arg(lua, &values, 0);
        host.collector.emit(Op::host_call(
            "noctalia",
            &["mkdirAll"],
            vec![JsonValue::String(path)],
        ));
        Ok(LuaValue::Boolean(true))
    })?;
    set_fn!(lua, &table, "renameFile", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let from = string_arg(lua, &values, 0);
        let to = string_arg(lua, &values, 1);
        host.collector.emit(Op::host_call(
            "noctalia",
            &["renameFile"],
            vec![JsonValue::String(from), JsonValue::String(to)],
        ));
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
        from_json(lua, &host.snapshot.setting(&key))
    })?;
    set_fn!(lua, &table, "getColor", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let role = string_arg(lua, &values, 0);
        from_json(lua, &host.snapshot.color(&role))
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
        from_json(lua, &host.snapshot.host_path("outputs"))
    })?;
    set_fn!(lua, &table, "focusedOutputName", |lua, _: Args| {
        let host = host(lua)?;
        from_json(lua, &host.snapshot.host_path("focusedOutputName"))
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
        Ok(LuaValue::String(
            lua.create_string(host.snapshot.plugin_dir())?,
        ))
    })?;
    set_fn!(lua, &table, "pluginDataDir", |lua, _: Args| {
        let host = host(lua)?;
        Ok(LuaValue::String(
            lua.create_string(host.snapshot.plugin_data_dir())?,
        ))
    })?;
    set_fn!(lua, &table, "getenv", |lua, args: Args| {
        let values = args.to_vec();
        let key = string_arg(lua, &values, 0);
        match env::var(key) {
            Ok(value) => Ok(LuaValue::String(lua.create_string(value)?)),
            Err(_) => Ok(LuaValue::Nil),
        }
    })?;
    set_fn!(lua, &table, "expandPath", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let path = expand_path(&host, &string_arg(lua, &values, 0));
        Ok(LuaValue::String(
            lua.create_string(path.to_string_lossy().to_string())?,
        ))
    })?;
    set_fn!(lua, &table, "commandExists", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let command = string_arg(lua, &values, 0);
        let exists = if command.contains(std::path::MAIN_SEPARATOR) {
            PathBuf::from(&command).is_file()
        } else {
            env::var_os("PATH").is_some_and(|paths| {
                env::split_paths(&paths).any(|dir| dir.join(&command).is_file())
            })
        };
        let _ = host;
        Ok(LuaValue::Boolean(exists))
    })?;
    set_fn!(lua, &table, "fileExists", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let path = string_arg(lua, &values, 0);
        Ok(LuaValue::Boolean(expand_path(&host, &path).exists()))
    })?;
    set_fn!(lua, &table, "readFile", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let path = string_arg(lua, &values, 0);
        match fs::read_to_string(expand_path(&host, &path)) {
            Ok(text) => Ok(LuaValue::String(lua.create_string(text)?)),
            Err(_) => Ok(LuaValue::Nil),
        }
    })?;
    set_fn!(lua, &table, "formatTime", |lua, args: Args| {
        let values = args.to_vec();
        let format = string_arg(lua, &values, 0);
        let pattern = format
            .strip_prefix("{:")
            .and_then(|value| value.strip_suffix('}'))
            .unwrap_or(&format);
        let text = Local::now().format(pattern).to_string();
        Ok(LuaValue::String(lua.create_string(text)?))
    })?;
    set_fn!(lua, &table, "listDir", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let path = string_arg(lua, &values, 0);
        let result = lua.create_table()?;
        match fs::read_dir(expand_path(&host, &path)) {
            Ok(entries) => {
                for (index, entry) in entries.enumerate() {
                    if let Ok(entry) = entry {
                        result.raw_set(
                            index + 1,
                            LuaValue::String(lua.create_string(
                                entry.path().to_string_lossy().to_string(),
                            )?),
                        )?;
                    }
                }
                Ok(LuaValue::Table(result))
            }
            Err(_) => Ok(LuaValue::Nil),
        }
    })?;
    set_fn!(lua, &table, "fileInfo", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let path = string_arg(lua, &values, 0);
        file_info_json(lua, &host, &path)
    })?;
    set_fn!(lua, &table, "tr", |lua, args: Args| {
        let host = host(lua)?;
        let values = args.to_vec();
        let key = string_arg(lua, &values, 0);
        let value = values.get(1).cloned().unwrap_or(LuaValue::Nil);
        let json = to_json(lua, value, FunctionRole::Call)?;
        let text = host.tr(&key, &params(&json));
        Ok(LuaValue::String(lua.create_string(text)?))
    })?;
    set_fn!(lua, &table, "trp", |lua, args: Args| {
        let text = lua
            .globals()
            .get::<Table>("noctalia")?
            .get::<Function>("tr")?
            .call::<String>(args)?;
        Ok(LuaValue::String(lua.create_string(text)?))
    })?;
    fallback!(lua, &table, "noctalia", &[])?;
    Ok(table)
}

fn file_info_json(lua: &Lua, host: &Host, path: &str) -> LuaResult<LuaValue> {
    let metadata = match fs::metadata(expand_path(host, path)) {
        Ok(value) => value,
        Err(_) => return Ok(LuaValue::Nil),
    };
    let table = lua.create_table()?;
    table.set("is_file", metadata.is_file())?;
    table.set("is_dir", metadata.is_dir())?;
    let size =
        mlua::Integer::try_from(metadata.len()).unwrap_or(mlua::Integer::MAX);
    table.set("size", size)?;
    Ok(LuaValue::Table(table))
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
    globals.set("layer", make_layer_table(lua, "layer")?)?;
    globals.set("ui", make_ui_table(lua)?)?;
    Ok(())
}
