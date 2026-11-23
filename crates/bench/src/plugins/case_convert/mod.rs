mod cases;

use super::{Case, Result, wrapper};
use noctalia_rs::Op;
use serde_json::{Value, json};

const INPUT: &str = "parseHTTPResponse to kebab-case, NASA rocks, café au lait, S.W.A.T. team, user-name, 2.5 and 1,000";

const SOURCES: &[(&str, &str)] = &[("cases.luau", include_str!("cases.luau"))];

const BODY: &str = r#"
local cases = require("./cases.luau")
function update()
  local results = cases.convertAll(fixture, true)
  local out = {}
  for index, result in results do
    out[index] = result.key .. "=" .. result.value
  end
  noctalia.log(json.encode(out))
end
"#;

pub fn case() -> Result<Case> {
    let fixture = json!(INPUT);
    Ok(Case {
        workload: "plugin_case_convert",
        luau_surface: "plugin_case_convert_luau",
        rust_surface: "plugin_case_convert_rust",
        modules: SOURCES,
        wrapper: wrapper(&fixture, BODY)?,
        fixture,
        rust: Box::new(|fixture: &Value| {
            let input =
                fixture.as_str().ok_or("case convert fixture is not text")?;
            let results = cases::convert_all(input, true);
            let out = results
                .iter()
                .map(|row| format!("{}={}", row.key, row.value))
                .collect::<Vec<_>>();
            Ok(vec![Op::log(serde_json::to_string(&out)?)])
        }),
    })
}
