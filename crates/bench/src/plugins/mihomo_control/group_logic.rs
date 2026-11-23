use serde_json::{Map, Value, json};

use super::util::{clamp, floor, lua_eq, number, string, tostring, truthy};

pub const PROXIES_INLINE_DECODE_LIMIT: usize = 65536;
pub const CONNECTIONS_INLINE_DECODE_LIMIT: usize = 8192;
const DEFAULT_TEST_URL: &str = "https://www.gstatic.com/generate_204";

fn obj() -> Map<String, Value> {
    Map::new()
}

pub fn clamp_interval(seconds: Option<&Value>) -> f64 {
    clamp(number(seconds).unwrap_or(2.0), 1.0, 60.0)
}

pub fn normalize_group_order(raw: Option<&Value>) -> Vec<String> {
    let Some(Value::Array(items)) = raw else {
        return Vec::new();
    };
    let mut order = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for item in items {
        if let Some(name) = string(Some(item)) {
            if !name.is_empty() && seen.insert(name.to_owned()) {
                order.push(name.to_owned());
            }
        }
    }
    order
}

pub fn apply_group_order(groups: &mut [Value], group_order: Option<&Value>) {
    let mut rank = std::collections::HashMap::new();
    if let Some(Value::Array(order)) = group_order {
        for (index, name) in order.iter().enumerate() {
            if let Some(name) = string(Some(name)) {
                rank.insert(name.to_owned(), index);
            }
        }
    }
    groups.sort_by(|a, b| {
        let a_name = string(a.get("name")).unwrap_or("");
        let b_name = string(b.get("name")).unwrap_or("");
        match (rank.get(a_name), rank.get(b_name)) {
            (Some(a_rank), Some(b_rank)) => a_rank.cmp(b_rank),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a_name.cmp(b_name),
        }
    });
}

pub fn reorder_groups(
    groups: &[Value],
    group_name: Option<&str>,
    insertion_index: Option<&Value>,
) -> Option<(Vec<Value>, Vec<String>)> {
    let from_index = groups
        .iter()
        .position(|group| string(group.get("name")) == group_name)?;
    let insert = number(insertion_index)?;

    let mut copy = groups.to_vec();
    let mut insert_at = floor(insert) as i64;
    let moved = copy.remove(from_index);
    if (from_index as i64 + 1) < insert_at {
        insert_at -= 1;
    }
    insert_at = insert_at.clamp(1, copy.len() as i64 + 1);
    copy.insert((insert_at - 1) as usize, moved);

    let order = copy
        .iter()
        .filter_map(|group| string(group.get("name")).map(str::to_owned))
        .collect();
    Some((copy, order))
}

pub fn traffic_changed(previous: Option<&Value>, next: Option<&Value>) -> bool {
    let (Some(Value::Object(previous)), Some(Value::Object(next))) =
        (previous, next)
    else {
        return true;
    };
    let compare = |key: &str| -> bool {
        let previous_value = previous.get(key).cloned().unwrap_or(Value::Null);
        let next_value = number(next.get(key))
            .map(Value::from)
            .unwrap_or(previous_value);
        lua_eq(&next_value, previous.get(key).unwrap_or(&Value::Null))
    };
    !(compare("up")
        && compare("down")
        && compare("upTotal")
        && compare("downTotal"))
}

pub fn connections_body_needs_external_decode(body: Option<&str>) -> bool {
    body.is_some_and(|value| value.len() > CONNECTIONS_INLINE_DECODE_LIMIT)
}

pub fn parse_connections_summary(data: Option<&Value>) -> Option<Value> {
    let Value::Object(data) = data? else {
        return None;
    };
    let count =
        number(data.get("count")).or_else(|| match data.get("connections") {
            Some(Value::Array(connections)) => Some(connections.len() as f64),
            _ => None,
        });
    Some(json!({
        "count": count.unwrap_or(0.0),
        "downloadTotal": number(data.get("downloadTotal")).unwrap_or(0.0),
        "uploadTotal": number(data.get("uploadTotal")).unwrap_or(0.0),
        "memory": number(data.get("memory")).unwrap_or(0.0),
    }))
}

pub fn proxies_body_needs_external_decode(body: Option<&str>) -> bool {
    body.is_some_and(|value| value.len() > PROXIES_INLINE_DECODE_LIMIT)
}

fn member(name: &str, delay: Option<f64>) -> Value {
    let mut row = obj();
    row.insert("name".to_owned(), Value::String(name.to_owned()));
    if let Some(delay) = delay {
        row.insert("delay".to_owned(), Value::from(delay));
    }
    Value::Object(row)
}

fn group_row(
    name: String,
    info: &Map<String, Value>,
    members: Vec<Value>,
    default_test_url: Option<&str>,
) -> Value {
    let default = default_test_url.unwrap_or(DEFAULT_TEST_URL);
    let group_type = string(info.get("type")).unwrap_or("");
    let now = info
        .get("now")
        .filter(|value| !value.is_null())
        .map(|value| tostring(Some(value)));
    let test_url = string(info.get("testUrl"))
        .map(str::to_owned)
        .unwrap_or_else(|| default.to_owned());
    json!({
        "name": name,
        "type": group_type,
        "now": now,
        "hidden": truthy(info.get("hidden")),
        "testUrl": test_url,
        "members": members,
    })
}

pub fn build_groups_from_proxies(
    proxies: Option<&Value>,
    default_test_url: Option<&str>,
) -> Vec<Value> {
    let Some(Value::Object(proxies)) = proxies else {
        return Vec::new();
    };

    let mut proxy_delays = std::collections::HashMap::new();
    for (name, info) in proxies {
        if let Value::Object(info) = info {
            if let Some(Value::Array(history)) = info.get("history") {
                if let Some(last) = history.last() {
                    if let Value::Object(last) = last {
                        if last.contains_key("delay") {
                            proxy_delays.insert(
                                name.clone(),
                                number(last.get("delay")).unwrap_or(0.0),
                            );
                        }
                    }
                }
            }
        }
    }

    let mut groups = Vec::new();
    for (name, info) in proxies {
        let Value::Object(info) = info else {
            continue;
        };
        let Some(Value::Array(all)) = info.get("all") else {
            continue;
        };
        let members = all
            .iter()
            .map(|member_name| {
                let member_name = tostring(Some(member_name));
                let delay = proxy_delays.get(&member_name).copied();
                member(&member_name, delay)
            })
            .collect();
        groups.push(group_row(name.clone(), info, members, default_test_url));
    }
    groups
}

pub fn build_groups_from_slim_proxies(
    slim: Option<&Value>,
    default_test_url: Option<&str>,
) -> Vec<Value> {
    let Some(Value::Object(slim)) = slim else {
        return Vec::new();
    };
    let Some(Value::Array(raw_groups)) = slim.get("groups") else {
        return Vec::new();
    };
    let delays = match slim.get("delays") {
        Some(Value::Object(delays)) => delays,
        _ => &EMPTY_OBJECT,
    };
    let default = default_test_url.unwrap_or(DEFAULT_TEST_URL);
    let mut groups = Vec::new();

    for raw in raw_groups {
        let Value::Object(group) = raw else {
            continue;
        };
        let Some(name) = string(group.get("name")) else {
            continue;
        };
        let members = match group.get("members") {
            Some(Value::Array(members)) => members,
            _ => &EMPTY_ARRAY,
        };
        let members = members
            .iter()
            .map(|member_name| {
                let member_name = tostring(Some(member_name));
                let delay = number(delays.get(&member_name));
                member(&member_name, delay)
            })
            .collect();
        groups.push(group_row(name.to_owned(), group, members, Some(default)));
    }
    groups
}

static EMPTY_OBJECT: std::sync::LazyLock<Map<String, Value>> =
    std::sync::LazyLock::new(Map::new);
static EMPTY_ARRAY: std::sync::LazyLock<Vec<Value>> =
    std::sync::LazyLock::new(Vec::new);

pub fn normalize_port(value: Option<&Value>) -> Option<i64> {
    let port = number(value)?;
    if port < 1.0 || port > 65535.0 {
        return None;
    }
    Some(floor(port) as i64)
}

pub fn normalize_server(raw: Option<&Value>) -> Option<Value> {
    let Value::Object(raw) = raw? else {
        return None;
    };
    let id = string(raw.get("id"))
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let host = string(raw.get("host"))
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    let port = normalize_port(raw.get("port"));
    let (id, host, port) = (id?, host?, port?);

    let name = string(raw.get("name"))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| {
            format!("{host}:{}", super::util::format_number(port as f64))
        });
    let test_url = string(raw.get("testUrl"))
        .or_else(|| string(raw.get("test_url")))
        .unwrap_or("")
        .to_owned();
    let secret = string(raw.get("secret")).unwrap_or("").to_owned();
    let use_https = truthy(raw.get("useHttps")) || truthy(raw.get("use_https"));
    let insecure = truthy(raw.get("allowInsecureTls"))
        || truthy(raw.get("allow_insecure_tls"));

    Some(json!({
        "id": id,
        "name": name,
        "host": host,
        "port": port,
        "secret": secret,
        "useHttps": use_https,
        "allowInsecureTls": insecure,
        "testUrl": test_url,
    }))
}

pub fn public_server_view(server: Option<&Value>) -> Option<Value> {
    let Value::Object(server) = server? else {
        return None;
    };
    let test_url = string(server.get("testUrl")).unwrap_or("").to_owned();
    let secret = string(server.get("secret")).unwrap_or("").to_owned();
    Some(json!({
        "id": server.get("id").cloned().unwrap_or(Value::Null),
        "name": server.get("name").cloned().unwrap_or(Value::Null),
        "host": server.get("host").cloned().unwrap_or(Value::Null),
        "port": server.get("port").cloned().unwrap_or(Value::Null),
        "useHttps": truthy(server.get("useHttps")),
        "allowInsecureTls": truthy(server.get("allowInsecureTls")),
        "testUrl": test_url,
        "hasSecret": !secret.is_empty(),
    }))
}

pub fn server_from_plugin_settings(
    id: &str,
    name: &str,
    cfg: Option<&Value>,
) -> Option<Value> {
    let Value::Object(cfg) = cfg? else {
        return None;
    };
    let host = string(cfg.get("host"))
        .filter(|value| !value.is_empty())
        .unwrap_or("127.0.0.1")
        .to_owned();
    let port = normalize_port(cfg.get("port")).unwrap_or(9090);
    let secret = string(cfg.get("secret")).unwrap_or("").to_owned();
    let test_url = string(cfg.get("test_url")).unwrap_or("").to_owned();
    normalize_server(Some(&json!({
        "id": id,
        "name": name,
        "host": host,
        "port": port,
        "secret": secret,
        "use_https": truthy(cfg.get("use_https")),
        "allow_insecure_tls": truthy(cfg.get("allow_insecure_tls")),
        "test_url": test_url,
    })))
}

pub fn normalize_servers_store(raw: Option<&Value>) -> Value {
    let mut store = json!({
        "activeId": "",
        "servers": [],
        "groupOrders": {},
    });
    let Value::Object(raw) = raw.unwrap_or(&Value::Null) else {
        return store;
    };
    if let Some(active_id) = string(raw.get("activeId")) {
        store
            .as_object_mut()
            .expect("object")
            .insert("activeId".to_owned(), Value::String(active_id.to_owned()));
    }
    if let Some(Value::Array(items)) = raw.get("servers") {
        let servers = store["servers"].as_array_mut().expect("array");
        for item in items {
            if let Some(server) = normalize_server(Some(item)) {
                servers.push(server);
            }
        }
    }
    if let Some(Value::Object(orders)) = raw.get("groupOrders") {
        let target = store["groupOrders"].as_object_mut().expect("object");
        for (id, order) in orders {
            if !id.is_empty() && matches!(order, Value::Array(_)) {
                let normalized = normalize_group_order(Some(order))
                    .into_iter()
                    .map(Value::String)
                    .collect();
                target.insert(id.clone(), Value::Array(normalized));
            }
        }
    }
    let servers = store["servers"].as_array().expect("array").clone();
    let active = string(store.get("activeId")).unwrap_or("").to_owned();
    if active.is_empty() && !servers.is_empty() {
        let first = string(servers[0].get("id")).unwrap_or("").to_owned();
        store
            .as_object_mut()
            .expect("object")
            .insert("activeId".to_owned(), Value::String(first));
    }
    let active = string(store.get("activeId")).unwrap_or("").to_owned();
    if find_server(Some(&Value::Array(servers.clone())), Some(&active))
        .is_none()
        && !servers.is_empty()
    {
        let first = string(servers[0].get("id")).unwrap_or("").to_owned();
        store
            .as_object_mut()
            .expect("object")
            .insert("activeId".to_owned(), Value::String(first));
    }
    store
}

pub fn find_server<'a>(
    servers: Option<&'a Value>,
    id: Option<&str>,
) -> Option<&'a Value> {
    let Value::Array(servers) = servers? else {
        return None;
    };
    let id = id.filter(|value| !value.is_empty())?;
    servers
        .iter()
        .find(|server| string(server.get("id")) == Some(id))
}

pub fn upsert_server(servers: Option<&Value>, server: &Value) -> Vec<Value> {
    let mut copy = Vec::new();
    let mut replaced = false;
    if let Some(Value::Array(items)) = servers {
        for item in items {
            if string(item.get("id")) == string(server.get("id")) {
                copy.push(server.clone());
                replaced = true;
            } else {
                copy.push(item.clone());
            }
        }
    }
    if !replaced {
        copy.push(server.clone());
    }
    copy
}

pub fn delete_server(servers: Option<&Value>, id: Option<&str>) -> Vec<Value> {
    let mut copy = Vec::new();
    if let Some(Value::Array(items)) = servers {
        for item in items {
            if string(item.get("id")) != id {
                copy.push(item.clone());
            }
        }
    }
    copy
}

pub fn public_servers_state(store: Option<&Value>) -> Value {
    let mut list = Vec::new();
    if let Some(Value::Object(store)) = store {
        if let Some(Value::Array(servers)) = store.get("servers") {
            for server in servers {
                if let Some(view) = public_server_view(Some(server)) {
                    list.push(view);
                }
            }
        }
    }
    json!({
        "activeId": store
            .and_then(|value| value.get("activeId"))
            .cloned()
            .unwrap_or(Value::String(String::new())),
        "list": list,
    })
}
