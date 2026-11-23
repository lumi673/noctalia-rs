mod group_logic;
mod util;

use super::{Case, Result, wrapper};
use noctalia_rs::Op;
use serde_json::{Map, Value, json};

const SOURCES: &[(&str, &str)] =
    &[("group_logic.luau", include_str!("group_logic.luau"))];

const BODY: &str = r#"
local logic = require("./group_logic.luau")
function update()
  local store = logic.normalize_servers_store(fixture)
  noctalia.log(json.encode(store))
end
"#;

fn fixture() -> Value {
    let mut servers = Vec::new();
    for index in 0..400 {
        servers.push(json!({
            "id": format!("server-{index:03}"),
            "name": if index % 3 == 0 { Value::Null } else { json!(format!("Node {index}")) },
            "host": format!("host-{index:03}.example"),
            "port": 1080 + index % 100,
            "secret": if index % 5 == 0 { "secret" } else { "" },
            "useHttps": index % 2 == 0,
            "allow_insecure_tls": index % 7 == 0,
            "testUrl": if index % 11 == 0 { "https://example.test" } else { "" }
        }));
    }

    let mut orders = Map::new();
    for index in 0..40 {
        let order = (0..20)
            .map(|slot| format!("server-{:03}", (index * 17 + slot * 3) % 400))
            .collect::<Vec<_>>();
        orders.insert(format!("group-{index:02}"), json!(order));
    }

    json!({
        "activeId": "server-000",
        "servers": servers,
        "groupOrders": orders
    })
}

pub fn case() -> Result<Case> {
    let fixture = fixture();
    Ok(Case {
        workload: "plugin_mihomo_control",
        luau_surface: "plugin_mihomo_control_luau",
        rust_surface: "plugin_mihomo_control_rust",
        modules: SOURCES,
        wrapper: wrapper(&fixture, BODY)?,
        fixture,
        rust: Box::new(|fixture: &Value| {
            let store = group_logic::normalize_servers_store(Some(fixture));
            Ok(vec![Op::log(serde_json::to_string(&store)?)])
        }),
    })
}
