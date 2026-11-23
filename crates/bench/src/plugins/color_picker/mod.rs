mod color;
mod common;

use super::{Case, Result, wrapper};
use noctalia_rs::Op;
use serde_json::{Value, json};

const SOURCES: &[(&str, &str)] = &[("color.luau", include_str!("color.luau"))];

const BODY: &str = r#"
local color = require("./color.luau")
function update()
  local out = {}
  for index, hex in ipairs(fixture.colors) do
    local alpha = fixture.alphas[index]
    local r, g, b = color.hexToRgb(hex)
    local h, s, l = color.rgbToHsl(r, g, b)
    out[index] = string.format("%s|%s|%s|%s|%s", hex,
      color.formatHex(hex, alpha), color.formatRgb(hex, alpha),
      color.formatHsl(hex, alpha), color.hslToHex(h, s, l))
  end
  noctalia.log(json.encode(out))
end
"#;

fn fixture() -> Value {
    let mut colors = Vec::new();
    let mut alphas = Vec::new();
    for index in 0..128usize {
        let r = (index * 37 + 11) % 256;
        let g = (index * 91 + 43) % 256;
        let b = (index * 151 + 97) % 256;
        colors.push(format!("#{r:02X}{g:02X}{b:02X}"));
        alphas.push(if index % 2 == 0 { 0.25 } else { 1.0 });
    }
    json!({ "colors": colors, "alphas": alphas })
}

pub fn case() -> Result<Case> {
    let fixture = fixture();
    Ok(Case {
        workload: "plugin_color_picker",
        luau_surface: "plugin_color_picker_luau",
        rust_surface: "plugin_color_picker_rust",
        modules: SOURCES,
        wrapper: wrapper(&fixture, BODY)?,
        fixture,
        rust: Box::new(|fixture: &Value| {
            let colors = fixture
                .get("colors")
                .and_then(Value::as_array)
                .ok_or("color fixture has no colors")?;
            let alphas = fixture
                .get("alphas")
                .and_then(Value::as_array)
                .ok_or("color fixture has no alphas")?;
            let mut out = Vec::with_capacity(colors.len());
            for (hex, alpha) in colors.iter().zip(alphas) {
                let hex =
                    hex.as_str().ok_or("color fixture hex is not text")?;
                let alpha = alpha
                    .as_f64()
                    .ok_or("color fixture alpha is not numeric")?;
                let (r, g, b) = color::hex_to_rgb(hex);
                let (h, s, l) = color::rgb_to_hsl(r, g, b);
                out.push(format!(
                    "{}|{}|{}|{}|{}",
                    hex,
                    color::format_hex(hex, alpha),
                    color::format_rgb(hex, alpha),
                    color::format_hsl(hex, alpha),
                    color::hsl_to_hex(h, s, l)
                ));
            }
            Ok(vec![Op::log(serde_json::to_string(&out)?)])
        }),
    })
}
