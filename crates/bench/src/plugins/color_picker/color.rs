use noctalia_rs::Op;

use super::common::lua_sub;

fn is_ws(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn is_digit(byte: u8) -> bool {
    byte.is_ascii_digit()
}

fn is_hex(byte: u8) -> bool {
    byte.is_ascii_hexdigit()
}

fn skip_ws0(bytes: &[u8], pos: &mut usize) {
    while *pos < bytes.len() && is_ws(bytes[*pos]) {
        *pos += 1;
    }
}

fn skip_ws1(bytes: &[u8], pos: &mut usize) -> bool {
    let start = *pos;
    skip_ws0(bytes, pos);
    *pos > start
}

fn take_digits(bytes: &[u8], pos: &mut usize) -> Option<String> {
    let start = *pos;
    while *pos < bytes.len() && is_digit(bytes[*pos]) {
        *pos += 1;
    }
    if *pos == start {
        return None;
    }
    Some(String::from_utf8_lossy(&bytes[start..*pos]).into_owned())
}

fn take_alpha(bytes: &[u8], pos: &mut usize) -> String {
    let start = *pos;
    while *pos < bytes.len() && (is_digit(bytes[*pos]) || bytes[*pos] == b'.') {
        *pos += 1;
    }
    String::from_utf8_lossy(&bytes[start..*pos]).into_owned()
}

fn hex_byte(hex: &str, start: usize, end: usize) -> f64 {
    let part = lua_sub(hex, start, end);
    u8::from_str_radix(&part, 16).unwrap_or_default() as f64
}

pub fn hex_to_rgb(hex: &str) -> (f64, f64, f64) {
    (
        hex_byte(hex, 2, 3),
        hex_byte(hex, 4, 5),
        hex_byte(hex, 6, 7),
    )
}

pub fn rgb_to_hsl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let r = r / 255.0;
    let g = g / 255.0;
    let b = b / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let mut h = 0.0;
    let mut s = 0.0;
    let l = (max + min) / 2.0;

    if max != min {
        let d = max - min;
        s = if l > 0.5 {
            d / (2.0 - max - min)
        } else {
            d / (max + min)
        };
        if max == r {
            h = (g - b) / d + if g < b { 6.0 } else { 0.0 };
        } else if max == g {
            h = (b - r) / d + 2.0;
        } else {
            h = (r - g) / d + 4.0;
        }
        h /= 6.0;
    }

    (
        (h * 360.0 + 0.5).floor(),
        (s * 100.0 + 0.5).floor(),
        (l * 100.0 + 0.5).floor(),
    )
}

fn hue_to_rgb(p: f64, q: f64, mut t: f64) -> f64 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        return p + (q - p) * 6.0 * t;
    }
    if t < 1.0 / 2.0 {
        return q;
    }
    if t < 2.0 / 3.0 {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    p
}

pub fn hsl_to_hex(h: f64, s: f64, l: f64) -> String {
    let sf = s / 100.0;
    let lf = l / 100.0;

    let (r, g, b) = if sf == 0.0 {
        (lf, lf, lf)
    } else {
        let q = if lf < 0.5 {
            lf * (1.0 + sf)
        } else {
            lf + sf - lf * sf
        };
        let p = 2.0 * lf - q;
        let hn = h / 360.0;
        (
            hue_to_rgb(p, q, hn + 1.0 / 3.0),
            hue_to_rgb(p, q, hn),
            hue_to_rgb(p, q, hn - 1.0 / 3.0),
        )
    };

    format!(
        "#{:02X}{:02X}{:02X}",
        (r * 255.0 + 0.5).floor() as i64,
        (g * 255.0 + 0.5).floor() as i64,
        (b * 255.0 + 0.5).floor() as i64
    )
}

pub fn format_hex(hex: &str, alpha: f64) -> String {
    if alpha < 1.0 {
        let alpha_val = (alpha * 255.0 + 0.5).floor() as i64;
        if hex.bytes().any(|byte| byte.is_ascii_lowercase()) {
            return format!("{hex}{alpha_val:02x}");
        }
        return format!("{hex}{alpha_val:02X}");
    }
    hex.to_owned()
}

pub fn format_rgb(hex: &str, alpha: f64) -> String {
    let (r, g, b) = hex_to_rgb(hex);
    if alpha < 1.0 {
        format!(
            "rgba({:.0}, {:.0}, {:.0}, {alpha:.2})",
            r.floor(),
            g.floor(),
            b.floor()
        )
    } else {
        format!("rgb({:.0}, {:.0}, {:.0})", r.floor(), g.floor(), b.floor())
    }
}

pub fn format_hsl(hex: &str, alpha: f64) -> String {
    let (r, g, b) = hex_to_rgb(hex);
    let (h, s, l) = rgb_to_hsl(r, g, b);
    if alpha < 1.0 {
        format!("hsla({h:.0}, {s:.0}%, {l:.0}%, {alpha:.2})")
    } else {
        format!("hsl({h:.0}, {s:.0}%, {l:.0}%)")
    }
}

fn match_picker_hsl(text: &str) -> Option<(String, String, String)> {
    let bytes = text.as_bytes();
    let mut pos = 0;
    let h = take_digits(bytes, &mut pos)?;
    if !skip_ws1(bytes, &mut pos) {
        return None;
    }
    let s = take_digits(bytes, &mut pos)?;
    if pos < bytes.len() && bytes[pos] == b'%' {
        pos += 1;
    }
    if !skip_ws1(bytes, &mut pos) {
        return None;
    }
    let l = take_digits(bytes, &mut pos)?;
    if pos < bytes.len() && bytes[pos] == b'%' {
        pos += 1;
    }
    if pos != bytes.len() {
        return None;
    }
    Some((h, s, l))
}

fn match_picker_rgb(text: &str) -> Option<(String, String, String)> {
    let bytes = text.as_bytes();
    let mut pos = 0;
    let r = take_digits(bytes, &mut pos)?;
    if !skip_ws1(bytes, &mut pos) {
        return None;
    }
    let g = take_digits(bytes, &mut pos)?;
    if !skip_ws1(bytes, &mut pos) {
        return None;
    }
    let b = take_digits(bytes, &mut pos)?;
    if pos != bytes.len() {
        return None;
    }
    Some((r, g, b))
}

pub fn parse_picker_output(output: &str, ops: &mut Vec<Op>) -> Option<String> {
    let output = output.trim_end_matches(char::is_whitespace);
    let bytes = output.as_bytes();

    if bytes.len() >= 7 {
        for start in 0..=bytes.len() - 7 {
            if bytes[start] == b'#'
                && bytes[start + 1..start + 7].iter().all(|b| is_hex(*b))
            {
                let hex = &output[start..start + 7];
                ops.push(Op::log(format!("[COLOR PICKER] hex = {hex}")));
                return Some(hex.to_uppercase());
            }
        }
    }

    if let Some((h, s, l)) = match_picker_hsl(output) {
        ops.push(Op::log(format!("[COLOR PICKER] hsl = {h}, {s}, {l}")));
        let hn = h.parse::<f64>().unwrap_or(f64::INFINITY);
        let sn = s.parse::<f64>().unwrap_or(f64::INFINITY);
        let ln = l.parse::<f64>().unwrap_or(f64::INFINITY);
        if hn <= 360.0 && sn <= 100.0 && ln <= 100.0 {
            return Some(hsl_to_hex(hn, sn, ln));
        }
    }

    if let Some((r, g, b)) = match_picker_rgb(output) {
        ops.push(Op::log(format!("[COLOR PICKER] hsl = {r}, {g}, {b}")));
        let rn = r.parse::<f64>().unwrap_or(f64::INFINITY);
        let gn = g.parse::<f64>().unwrap_or(f64::INFINITY);
        let bn = b.parse::<f64>().unwrap_or(f64::INFINITY);
        if rn <= 255.0 && gn <= 255.0 && bn <= 255.0 {
            return Some(format!(
                "#{:02X}{:02X}{:02X}",
                rn.floor() as i64,
                gn.floor() as i64,
                bn.floor() as i64
            ));
        }
    }

    None
}

pub fn parse_hex_text(text: &str) -> (Option<String>, Option<f64>) {
    let clean = text.strip_prefix('#').unwrap_or(text);
    if clean.is_empty() || !clean.bytes().all(is_hex) {
        return (None, None);
    }

    if clean.len() == 6 {
        return (Some(format!("#{}", clean.to_uppercase())), None);
    }
    if clean.len() == 8 {
        let hex = format!("#{}", clean[..6].to_uppercase());
        let alpha = u8::from_str_radix(&clean[6..8], 16)
            .map(|value| value as f64 / 255.0)
            .ok();
        return (Some(hex), alpha);
    }
    (None, None)
}

pub fn parse_rgb_text(text: &str) -> (Option<String>, Option<f64>) {
    let bytes = text.as_bytes();
    let mut pos = 0;
    skip_ws0(bytes, &mut pos);
    let Some(r) = take_digits(bytes, &mut pos) else {
        return (None, None);
    };
    skip_ws0(bytes, &mut pos);
    if pos >= bytes.len() || bytes[pos] != b',' {
        return (None, None);
    }
    pos += 1;
    skip_ws0(bytes, &mut pos);
    let Some(g) = take_digits(bytes, &mut pos) else {
        return (None, None);
    };
    skip_ws0(bytes, &mut pos);
    if pos >= bytes.len() || bytes[pos] != b',' {
        return (None, None);
    }
    pos += 1;
    skip_ws0(bytes, &mut pos);
    let Some(b) = take_digits(bytes, &mut pos) else {
        return (None, None);
    };
    skip_ws0(bytes, &mut pos);
    if pos < bytes.len() && bytes[pos] == b',' {
        pos += 1;
    }
    skip_ws0(bytes, &mut pos);
    let alpha_text = take_alpha(bytes, &mut pos);

    let rn = r.parse::<f64>().unwrap_or_default();
    let gn = g.parse::<f64>().unwrap_or_default();
    let bn = b.parse::<f64>().unwrap_or_default();
    if rn > 255.0 || gn > 255.0 || bn > 255.0 {
        return (None, None);
    }

    let hex = format!(
        "#{:02X}{:02X}{:02X}",
        rn.floor() as i64,
        gn.floor() as i64,
        bn.floor() as i64
    );
    let mut alpha = None;
    if !alpha_text.is_empty() {
        if let Some(value) = alpha_text.parse::<f64>().ok() {
            alpha = Some(if value > 1.0 { value / 100.0 } else { value });
        }
    }
    (Some(hex), alpha)
}

pub fn parse_hsl_text(text: &str) -> (Option<String>, Option<f64>) {
    let bytes = text.as_bytes();
    let mut pos = 0;
    skip_ws0(bytes, &mut pos);
    let Some(h) = take_digits(bytes, &mut pos) else {
        return (None, None);
    };
    skip_ws0(bytes, &mut pos);
    if pos >= bytes.len() || bytes[pos] != b',' {
        return (None, None);
    }
    pos += 1;
    skip_ws0(bytes, &mut pos);
    let Some(s) = take_digits(bytes, &mut pos) else {
        return (None, None);
    };
    if pos < bytes.len() && bytes[pos] == b'%' {
        pos += 1;
    }
    skip_ws0(bytes, &mut pos);
    if pos >= bytes.len() || bytes[pos] != b',' {
        return (None, None);
    }
    pos += 1;
    skip_ws0(bytes, &mut pos);
    let Some(l) = take_digits(bytes, &mut pos) else {
        return (None, None);
    };
    if pos < bytes.len() && bytes[pos] == b'%' {
        pos += 1;
    }
    skip_ws0(bytes, &mut pos);
    if pos < bytes.len() && bytes[pos] == b',' {
        pos += 1;
    }
    skip_ws0(bytes, &mut pos);
    let alpha_text = take_alpha(bytes, &mut pos);

    let hn = h.parse::<f64>().unwrap_or_default();
    let sn = s.parse::<f64>().unwrap_or_default();
    let ln = l.parse::<f64>().unwrap_or_default();
    let hex = hsl_to_hex(hn, sn, ln);

    let mut alpha = None;
    if !alpha_text.is_empty() {
        if let Some(value) = alpha_text.parse::<f64>().ok() {
            alpha = Some(if value > 1.0 { value / 100.0 } else { value });
        }
    }
    (Some(hex), alpha)
}
