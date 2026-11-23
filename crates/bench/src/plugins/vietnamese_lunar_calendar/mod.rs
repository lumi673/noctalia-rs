mod lunar;

use super::{Case, Result, wrapper};
use chrono::{Datelike, Duration, NaiveDate};
use noctalia_rs::Op;
use serde_json::{Value, json};

const SOURCES: &[(&str, &str)] = &[("lunar.luau", include_str!("lunar.luau"))];

const BODY: &str = r#"
local lunar = require("./lunar.luau")
function update()
  local out = {}
  for index, date in ipairs(fixture) do
    local value = lunar.solarToLunar(date[1], date[2], date[3], 7)
    out[index] = string.format("%d/%d/%d/%s", value.day, value.month, value.year, tostring(value.leap))
  end
  noctalia.log(json.encode(out))
end
"#;

fn fixture() -> Result<Value> {
    let start =
        NaiveDate::from_ymd_opt(2024, 1, 1).ok_or("invalid fixture start")?;
    let mut dates = Vec::new();
    for offset in 0..730 {
        let date = start + Duration::days(offset);
        dates.push(json!([date.day(), date.month(), date.year()]));
    }
    Ok(Value::Array(dates))
}

pub fn case() -> Result<Case> {
    let fixture = fixture()?;
    Ok(Case {
        workload: "plugin_vietnamese_lunar_calendar",
        luau_surface: "plugin_vietnamese_lunar_calendar_luau",
        rust_surface: "plugin_vietnamese_lunar_calendar_rust",
        modules: SOURCES,
        wrapper: wrapper(&fixture, BODY)?,
        fixture,
        rust: Box::new(|fixture: &Value| {
            let dates =
                fixture.as_array().ok_or("lunar fixture is not an array")?;
            let mut out = Vec::with_capacity(dates.len());
            for date in dates {
                let values = date
                    .as_array()
                    .ok_or("lunar fixture date is not an array")?;
                let day =
                    values[0].as_i64().ok_or("lunar day is not numeric")?;
                let month =
                    values[1].as_i64().ok_or("lunar month is not numeric")?;
                let year =
                    values[2].as_i64().ok_or("lunar year is not numeric")?;
                let value = lunar::solar_to_lunar(day, month, year, 7.0);
                out.push(format!(
                    "{}/{}/{}/{}",
                    value.day, value.month, value.year, value.leap
                ));
            }
            Ok(vec![Op::log(serde_json::to_string(&out)?)])
        }),
    })
}
