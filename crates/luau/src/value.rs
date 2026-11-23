use mlua::{Lua, Result as LuaResult, Table, Value as LuaValue};
use noctalia_rs::ui::{UiNode, node};
use serde_json::{Map, Value as JsonValue, json};

use crate::host::Collector;

const MAX_DEPTH: usize = 64;

#[derive(Clone, Copy)]
pub enum FunctionRole {
    Call,
    HostCallback,
}

pub struct Convert<'a> {
    lua: &'a Lua,
    role: FunctionRole,
    collector: &'a Collector,
}

impl<'a> Convert<'a> {
    pub fn new(
        lua: &'a Lua,
        role: FunctionRole,
        collector: &'a Collector,
    ) -> Self {
        Self {
            lua,
            role,
            collector,
        }
    }

    pub fn to_json(&self, value: LuaValue) -> LuaResult<JsonValue> {
        self.json_inner(value, 0)
    }

    fn json_inner(
        &self,
        value: LuaValue,
        depth: usize,
    ) -> LuaResult<JsonValue> {
        if depth > MAX_DEPTH {
            return Ok(JsonValue::Null);
        }
        match value {
            LuaValue::Nil => Ok(JsonValue::Null),
            LuaValue::Boolean(value) => Ok(JsonValue::Bool(value)),
            LuaValue::Integer(value) => Ok(json!(value)),
            LuaValue::Number(value) => Ok(json!(value)),
            LuaValue::String(value) => {
                let bytes = value.as_bytes();
                match std::str::from_utf8(&bytes) {
                    Ok(text) => Ok(JsonValue::String(text.to_owned())),
                    Err(_) => Ok(json!({
                        noctalia_rs::bytes::BYTES_KEY:
                            noctalia_rs::bytes::encode_hex(&bytes),
                    })),
                }
            }
            LuaValue::Function(function) => Ok(self.marker(&function)?),
            LuaValue::Table(table) => {
                if let Some(marker) = table_marker(&table)? {
                    return Ok(marker);
                }
                let length = table.raw_len();
                if is_sequence(&table, length) {
                    let mut values = Vec::with_capacity(length);
                    for index in 1..=length {
                        let item = table.raw_get::<LuaValue>(index)?;
                        values.push(self.json_inner(item, depth + 1)?);
                    }
                    Ok(JsonValue::Array(values))
                } else {
                    let mut object = Map::new();
                    for pair in table.pairs::<LuaValue, LuaValue>() {
                        let (key, item) = pair?;
                        let key = display(self.lua, key);
                        let item = self.json_inner(item, depth + 1)?;
                        object.insert(key, item);
                    }
                    Ok(JsonValue::Object(object))
                }
            }
            _ => Ok(JsonValue::Null),
        }
    }

    fn marker(&self, function: &mlua::Function) -> LuaResult<JsonValue> {
        let id = self.collector.register(function);
        match self.role {
            FunctionRole::Call => Ok(json!({ "__call": id })),
            FunctionRole::HostCallback => Ok(json!({ "__host_callback": id })),
        }
    }

    pub fn ui_node(&self, value: LuaValue) -> LuaResult<UiNode> {
        self.ui_inner(value, 0)
    }

    fn ui_inner(&self, value: LuaValue, depth: usize) -> LuaResult<UiNode> {
        if depth > MAX_DEPTH {
            return Ok(node("label", json!({ "text": "..." }), Vec::new()));
        }
        let LuaValue::Table(table) = value else {
            let text = display(self.lua, value);
            return Ok(node("label", json!({ "text": text }), Vec::new()));
        };
        let kind = match table.raw_get::<LuaValue>("kind")? {
            LuaValue::String(value) => value.to_string_lossy().to_string(),
            other => display(self.lua, other),
        };
        let props = match table.raw_get::<LuaValue>("props")? {
            LuaValue::Nil => json!({}),
            props => self.json_inner(props, depth + 1)?,
        };
        let mut children = Vec::new();
        let LuaValue::Table(children_table) =
            table.raw_get::<LuaValue>("children")?
        else {
            return Ok(node(&kind, props, children));
        };
        for index in 1..=children_table.raw_len() {
            let child = children_table.raw_get::<LuaValue>(index)?;
            children.push(self.ui_inner(child, depth + 1)?);
        }
        Ok(node(&kind, props, children))
    }
}

pub fn display(lua: &Lua, value: LuaValue) -> String {
    match lua.coerce_string(value.clone()) {
        Ok(Some(text)) => text.to_string_lossy().to_string(),
        _ => match value {
            LuaValue::Nil => "nil".to_owned(),
            other => format!("{other:?}"),
        },
    }
}

pub fn string_arg(lua: &Lua, args: &[LuaValue], index: usize) -> String {
    args.get(index)
        .map_or_else(String::new, |value| display(lua, value.clone()))
}

pub fn as_string(lua: &Lua, value: LuaValue) -> String {
    display(lua, value)
}

fn table_marker(table: &Table) -> LuaResult<Option<JsonValue>> {
    for (key, field) in
        [("__call", "__call"), ("__host_callback", "__host_callback")]
    {
        let value = table.raw_get::<LuaValue>(key)?;
        if let LuaValue::String(name) = value {
            return Ok(Some(
                json!({ field: name.to_string_lossy().to_string() }),
            ));
        }
    }
    Ok(None)
}

fn is_sequence(table: &Table, length: usize) -> bool {
    if length == 0 {
        return false;
    }
    let mut count = 0;
    let mut sequential = true;
    for pair in table.clone().pairs::<LuaValue, LuaValue>() {
        let Ok((key, _)) = pair else {
            return false;
        };
        count += 1;
        match key {
            LuaValue::Integer(index)
                if index >= 1 && index <= length as mlua::Integer => {}
            _ => sequential = false,
        }
    }
    sequential && count == length
}

pub fn from_json(lua: &Lua, value: &JsonValue) -> LuaResult<LuaValue> {
    match value {
        JsonValue::Null => Ok(LuaValue::Nil),
        JsonValue::Bool(value) => Ok(LuaValue::Boolean(*value)),
        JsonValue::Number(value) => {
            if let Some(integer) = value.as_i64() {
                match mlua::Integer::try_from(integer) {
                    Ok(value) => Ok(LuaValue::Integer(value)),
                    Err(_) => Ok(LuaValue::Number(integer as f64)),
                }
            } else {
                Ok(LuaValue::Number(value.as_f64().unwrap_or_default()))
            }
        }
        JsonValue::String(value) => {
            Ok(LuaValue::String(lua.create_string(value)?))
        }
        JsonValue::Array(values) => {
            let table = lua.create_table()?;
            for (index, item) in values.iter().enumerate() {
                table.raw_set(index + 1, from_json(lua, item)?)?;
            }
            Ok(LuaValue::Table(table))
        }
        JsonValue::Object(values) => {
            let table = lua.create_table()?;
            for (key, item) in values {
                table.set(key.as_str(), from_json(lua, item)?)?;
            }
            Ok(LuaValue::Table(table))
        }
    }
}

/// Converts a host query value into Lua, decoding bridge byte markers.
pub fn from_json_with_bytes(
    lua: &Lua,
    value: &JsonValue,
) -> LuaResult<LuaValue> {
    if let Some(bytes) = noctalia_rs::bytes::marker_bytes(value) {
        return Ok(LuaValue::String(lua.create_string(bytes)?));
    }
    match value {
        JsonValue::Array(values) => {
            let table = lua.create_table()?;
            for (index, item) in values.iter().enumerate() {
                table.raw_set(index + 1, from_json_with_bytes(lua, item)?)?;
            }
            Ok(LuaValue::Table(table))
        }
        JsonValue::Object(values) => {
            let table = lua.create_table()?;
            for (key, item) in values {
                table.set(key.as_str(), from_json_with_bytes(lua, item)?)?;
            }
            Ok(LuaValue::Table(table))
        }
        _ => from_json(lua, value),
    }
}

pub fn params(value: &JsonValue) -> Vec<(String, String)> {
    let Some(object) = value.as_object() else {
        return Vec::new();
    };
    object
        .iter()
        .map(|(key, item)| {
            let value = match item {
                JsonValue::String(value) => value.clone(),
                JsonValue::Null => String::new(),
                other => other.to_string(),
            };
            (key.clone(), value)
        })
        .collect()
}
