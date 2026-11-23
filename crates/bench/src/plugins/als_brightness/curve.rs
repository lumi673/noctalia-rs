use serde_json::Value;
use std::cmp::Ordering;

use super::util::{finite, lua_number, tonumber, trim_end, trim_start};

#[derive(Clone, Copy, Debug)]
pub struct Node {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug)]
pub struct Compiled {
    pub n: usize,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    h: Vec<f64>,
    d: Vec<f64>,
    m: Vec<f64>,
}

#[derive(Clone, Debug)]
struct Entry {
    label: String,
    text: Value,
    key: Option<String>,
}

pub const DEFAULT_BRIGHTNESS: [(&str, &str); 10] = [
    ("00000", "0:20.8"),
    ("00001", "1:30.7"),
    ("00003", "3:39.3"),
    ("00010", "10:50.6"),
    ("00033", "33:63.4"),
    ("00061", "61:70"),
    ("00560", "560:78.3"),
    ("01680", "1680:88.3"),
    ("04800", "4800:96.8"),
    ("14200", "14200:100"),
];

pub const DEFAULT_TEMPERATURE: [(&str, &str); 10] = [
    ("02000", "2000:5100"),
    ("02200", "2200:5240"),
    ("02500", "2500:5380"),
    ("03000", "3000:5520"),
    ("03500", "3500:5660"),
    ("04000", "4000:5800"),
    ("04500", "4500:5940"),
    ("05000", "5000:6080"),
    ("05500", "5500:6220"),
    ("06000", "6000:6500"),
];

pub const FIXED_BRIGHTNESS_Y: [f64; 10] =
    [20.8, 30.7, 39.3, 50.6, 63.4, 70.0, 78.3, 88.3, 96.8, 100.0];
pub const DEFAULT_BRIGHTNESS_X: [f64; 10] = [
    0.0, 1.0, 3.0, 10.0, 33.0, 61.0, 560.0, 1680.0, 4800.0, 14200.0,
];

pub const FIXED_TEMPERATURE_Y: [f64; 10] = [
    5100.0, 5240.0, 5380.0, 5520.0, 5660.0, 5800.0, 5940.0, 6080.0, 6220.0,
    6500.0,
];
pub const DEFAULT_TEMPERATURE_X: [f64; 10] = [
    2000.0, 2200.0, 2500.0, 3000.0, 3500.0, 4000.0, 4500.0, 5000.0, 5500.0,
    6000.0,
];

pub fn default_map(entries: &[(&str, &str)]) -> Value {
    let mut map = serde_json::Map::new();
    for (key, value) in entries {
        map.insert((*key).to_owned(), Value::String((*value).to_owned()));
    }
    Value::Object(map)
}

pub fn threshold_window(kind: &str, index: usize) -> Option<(f64, f64, f64)> {
    let defaults: &[f64] = match kind {
        "brightness" => &DEFAULT_BRIGHTNESS_X,
        "temperature" => &DEFAULT_TEMPERATURE_X,
        _ => return None,
    };
    let d = *defaults.get(index)?;
    if kind == "brightness" {
        let h = d.max(1.0);
        let mut lo = d - h;
        if lo < 0.0 {
            lo = 0.0;
        }
        let hi = 2.0 * h;
        Some((lo, hi, 1.0))
    } else {
        let mut lo = d - 500.0;
        if lo < 1500.0 {
            lo = 1500.0;
        }
        let mut hi = d + 500.0;
        if hi > 8000.0 {
            hi = 8000.0;
        }
        Some((lo, hi, 50.0))
    }
}

fn normalise(value: Option<&Value>) -> Option<Vec<Entry>> {
    let Some(value) = value else {
        return Some(Vec::new());
    };
    let mut entries = Vec::new();
    if let Value::String(text) = value {
        return Some(vec![Entry {
            label: "1".to_owned(),
            text: Value::String(text.clone()),
            key: None,
        }]);
    }
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for key in keys {
                entries.push(Entry {
                    label: key.clone(),
                    text: map.get(key).cloned().unwrap_or(Value::Null),
                    key: Some(key.clone()),
                });
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                entries.push(Entry {
                    label: (index + 1).to_string(),
                    text: item.clone(),
                    key: None,
                });
            }
        }
        _ => return None,
    }
    Some(entries)
}

fn read_node(
    text: &str,
    key: Option<&str>,
) -> (Option<f64>, Option<f64>, Option<String>) {
    if let Some((xs, ys)) = text.split_once(':') {
        if !xs.is_empty() {
            let x = tonumber(trim_end(xs));
            let y = tonumber(trim_start(ys));
            let mut note = None;
            if let (Some(x), Some(key)) = (x, key) {
                let keyed = tonumber(key);
                if let Some(keyed) = keyed {
                    if finite(keyed) && keyed != x {
                        note = Some(format!(
                            "row {:?} is keyed {} but its value says x={}; using the value (the row sorts by the key)",
                            key,
                            lua_number(keyed),
                            lua_number(x)
                        ));
                    }
                }
            }
            return (x, y, note);
        }
    }
    (key.and_then(|value| tonumber(value)), tonumber(text), None)
}

fn has_seen(seen: &[f64], value: f64) -> bool {
    seen.iter().any(|item| *item == value)
}

pub fn parse_nodes(
    value: Option<&Value>,
    map_x: Option<fn(f64) -> f64>,
) -> (Vec<Node>, Vec<String>) {
    let mut nodes = Vec::new();
    let mut problems = Vec::new();
    let Some(entries) = normalise(value) else {
        problems.push("curve is not a list".to_owned());
        return (nodes, problems);
    };
    let mut seen: Vec<f64> = Vec::new();
    for entry in &entries {
        let Value::String(text) = &entry.text else {
            problems.push(format!("entry {} is not a string", entry.label));
            continue;
        };
        let without_comment = match text.find('#') {
            Some(index) => &text[..index],
            None => text.as_str(),
        };
        let text = trim_start(trim_end(without_comment));
        if text.is_empty() {
            continue;
        }
        let (x, y, note) = read_node(text, entry.key.as_deref());
        if let Some(note) = note {
            problems.push(note);
        }
        let Some(x) = x.filter(|value| finite(*value)) else {
            problems.push(format!(
                "entry {}: {:?} has no reading; write it as \"reading:target\"",
                entry.label, entry.text
            ));
            continue;
        };
        let Some(y) = y.filter(|value| finite(*value)) else {
            problems.push(format!(
                "entry {}: cannot read 'x:y' numbers from {:?}",
                entry.label, entry.text
            ));
            continue;
        };
        let mapped = if let Some(map_x) = map_x {
            let mapped = map_x(x);
            if !finite(mapped) {
                problems.push(format!(
                    "entry {}: x={} is outside the range this curve can use",
                    entry.label,
                    lua_number(x)
                ));
                continue;
            }
            if has_seen(&seen, mapped) {
                problems.push(format!(
                    "entry {}: duplicate node at x={} (ignored)",
                    entry.label,
                    lua_number(x)
                ));
                continue;
            }
            seen.push(mapped);
            mapped
        } else {
            if has_seen(&seen, x) {
                problems.push(format!(
                    "entry {}: duplicate node at x={} (ignored)",
                    entry.label,
                    lua_number(x)
                ));
                continue;
            }
            seen.push(x);
            x
        };
        nodes.push(Node { x: mapped, y });
    }
    nodes.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(Ordering::Equal));
    (nodes, problems)
}

pub fn map_is_custom(value: Option<&Value>, defaults: &Value) -> bool {
    let (got, _) = parse_nodes(value, None);
    let (want, _) = parse_nodes(Some(defaults), None);
    if got.is_empty() {
        return false;
    }
    if got.len() != want.len() {
        return true;
    }
    for (actual, expected) in got.iter().zip(want.iter()) {
        if actual.x != expected.x || actual.y != expected.y {
            return true;
        }
    }
    false
}

pub fn build_nodes(
    thresholds: &[f64],
    fixed_ys: &[f64],
    map_x: Option<fn(f64) -> f64>,
) -> (Vec<Node>, Vec<String>) {
    let mut nodes = Vec::new();
    let mut problems = Vec::new();
    let mut n = fixed_ys.len();
    if thresholds.len() != n {
        problems.push(format!(
            "{} thresholds for {} fixed outputs (using the first {} of each)",
            thresholds.len(),
            fixed_ys.len(),
            thresholds.len().min(fixed_ys.len())
        ));
        n = n.min(thresholds.len());
    }
    for i in 0..n {
        let x = thresholds[i];
        let y = fixed_ys[i];
        if !finite(x) || !finite(y) {
            problems.push(format!(
                "node {}: threshold {} or output {} is not a number (node dropped)",
                i + 1,
                lua_number(x),
                lua_number(y)
            ));
        } else {
            nodes.push(Node { x, y });
        }
    }
    nodes.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap_or(Ordering::Equal));
    let mut repaired = 0;
    for i in 1..nodes.len() {
        if nodes[i].x <= nodes[i - 1].x {
            nodes[i].x = nodes[i - 1].x + 1.0;
            repaired += 1;
        }
    }
    if repaired > 0 {
        problems.push(format!(
            "{} threshold(s) collided when sorted; nodes re-spaced to strictly increasing",
            repaired
        ));
    }
    if let Some(map_x) = map_x {
        let mut kept = Vec::new();
        for (index, node) in nodes.iter_mut().enumerate() {
            let mapped = map_x(node.x);
            if !finite(mapped) {
                problems.push(format!(
                    "node {}: x={} is outside the range this curve can use",
                    index + 1,
                    lua_number(node.x)
                ));
            } else {
                node.x = mapped;
                kept.push(*node);
            }
        }
        nodes = kept;
    }
    (nodes, problems)
}

pub fn with_anchors(nodes: &[Node]) -> Vec<Node> {
    let n = nodes.len();
    if n == 0 {
        return nodes.to_vec();
    }
    let first = nodes[0];
    let last = nodes[n - 1];
    if !finite(first.x)
        || !finite(first.y)
        || !finite(last.x)
        || !finite(last.y)
    {
        return nodes.to_vec();
    }
    let mut out = Vec::with_capacity(n + 4);
    out.push(Node {
        x: first.x - 500.0,
        y: first.y,
    });
    out.push(Node {
        x: first.x - 300.0,
        y: first.y,
    });
    out.extend_from_slice(nodes);
    out.push(Node {
        x: last.x + 500.0,
        y: last.y,
    });
    out.push(Node {
        x: last.x + 1000.0,
        y: last.y,
    });
    out
}

pub fn compile(nodes: &[Node]) -> Option<Compiled> {
    let n = nodes.len();
    if n == 0 {
        return None;
    }
    let mut curve = Compiled {
        n,
        x: vec![0.0; n],
        y: vec![0.0; n],
        h: vec![0.0; n.saturating_sub(1)],
        d: vec![0.0; n.saturating_sub(1)],
        m: vec![0.0; n],
    };
    for (index, node) in nodes.iter().enumerate() {
        curve.x[index] = node.x;
        curve.y[index] = node.y;
        if !finite(node.x) || !finite(node.y) {
            return None;
        }
    }
    if n == 1 {
        curve.m[0] = 0.0;
        return Some(curve);
    }
    for i in 0..n - 1 {
        curve.h[i] = curve.x[i + 1] - curve.x[i];
        if curve.h[i] <= 0.0 {
            return None;
        }
        curve.d[i] = (curve.y[i + 1] - curve.y[i]) / curve.h[i];
    }
    curve.m[0] = curve.d[0];
    curve.m[n - 1] = curve.d[n - 2];
    for i in 1..n - 1 {
        let d0 = curve.d[i - 1];
        let d1 = curve.d[i];
        if d0 * d1 <= 0.0 {
            curve.m[i] = 0.0;
        } else {
            let h0 = curve.h[i - 1];
            let h1 = curve.h[i];
            let w1 = 2.0 * h1 + h0;
            let w2 = h1 + 2.0 * h0;
            curve.m[i] = (w1 + w2) / (w1 / d0 + w2 / d1);
        }
    }
    Some(curve)
}

pub fn eval(curve: Option<&Compiled>, x: f64) -> Option<f64> {
    let curve = curve?;
    if !finite(x) {
        return None;
    }
    let n = curve.n;
    if n == 1 {
        return Some(curve.y[0]);
    }
    if x <= curve.x[0] {
        return Some(curve.y[0]);
    }
    if x >= curve.x[n - 1] {
        return Some(curve.y[n - 1]);
    }
    let mut i = 0;
    while i < n - 2 && x > curve.x[i + 1] {
        i += 1;
    }
    let h = curve.h[i];
    let t = (x - curve.x[i]) / h;
    let t2 = t * t;
    let t3 = t2 * t;
    let h00 = 2.0 * t3 - 3.0 * t2 + 1.0;
    let h10 = t3 - 2.0 * t2 + t;
    let h01 = -2.0 * t3 + 3.0 * t2;
    let h11 = t3 - t2;
    Some(
        curve.y[i] * h00
            + h * curve.m[i] * h10
            + curve.y[i + 1] * h01
            + h * curve.m[i + 1] * h11,
    )
}

pub fn solve_x(curve: Option<&Compiled>, y_target: f64) -> Result<f64, String> {
    let Some(curve) = curve else {
        return Err("the curve has no nodes to invert".to_owned());
    };
    let n = curve.n;
    if n < 1
        || !finite(curve.x[0])
        || !finite(curve.y[0])
        || !finite(curve.x[n - 1])
        || !finite(curve.y[n - 1])
    {
        return Err("the curve has no nodes to invert".to_owned());
    }
    if !finite(y_target) {
        return Err("y_target is not a finite number".to_owned());
    }
    if y_target < curve.y[0] {
        return Err(format!(
            "y_target {} is below the first node's output {}",
            lua_number(y_target),
            lua_number(curve.y[0])
        ));
    }
    if y_target > curve.y[n - 1] {
        return Err(format!(
            "y_target {} is above the last node's output {}",
            lua_number(y_target),
            lua_number(curve.y[n - 1])
        ));
    }
    if y_target == curve.y[0] || n == 1 {
        return Ok(curve.x[0]);
    }
    for i in 0..n - 1 {
        if curve.y[i] <= y_target && y_target <= curve.y[i + 1] {
            if y_target == curve.y[i] {
                return Ok(curve.x[i]);
            }
            if y_target == curve.y[i + 1] {
                return Ok(curve.x[i + 1]);
            }
            let mut lo = curve.x[i];
            let mut hi = curve.x[i + 1];
            for _ in 0..200 {
                let mid = (lo + hi) / 2.0;
                if eval(Some(curve), mid).is_some_and(|value| value >= y_target)
                {
                    hi = mid;
                } else {
                    lo = mid;
                }
                if hi - lo <= 1e-9 * lo.abs().max(hi.abs()) {
                    break;
                }
            }
            return Ok(hi);
        }
    }
    Err(format!(
        "y_target {} is not bracketed by the node outputs",
        lua_number(y_target)
    ))
}

pub fn is_monotone(nodes: &[Node]) -> bool {
    for pair in nodes.windows(2) {
        if pair[1].y < pair[0].y {
            return false;
        }
    }
    true
}
