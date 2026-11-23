use std::cmp::Ordering;
use std::sync::{Arc, Mutex, MutexGuard};

pub const CATEGORIES: [&str; 8] = [
    "video",
    "images",
    "audio",
    "archives",
    "documents",
    "code",
    "packages",
    "other",
];
pub const COLORS_DARK: [&str; 8] = [
    "#7aa2f7", "#9ece6a", "#e0af68", "#f7768e", "#bb9af7", "#2ac3de",
    "#ff9e64", "#73daca",
];
pub const COLORS_LIGHT: [&str; 8] = [
    "#3b5fc0", "#4f8a2b", "#b07708", "#c2405a", "#7c4dbd", "#0e7490",
    "#c25a12", "#2f8f7a",
];
pub const EXT: [&[&str]; 7] = [
    &["mkv", "mp4", "avi", "mov", "webm", "m4v", "wmv", "flv"],
    &[
        "jpg", "jpeg", "png", "gif", "webp", "svg", "bmp", "tif", "tiff",
        "heic", "raw",
    ],
    &["mp3", "flac", "wav", "ogg", "opus", "m4a", "aac"],
    &["zip", "tar", "gz", "xz", "bz2", "zst", "7z", "rar", "tgz"],
    &[
        "pdf", "doc", "docx", "odt", "xls", "xlsx", "ods", "ppt", "pptx",
        "txt", "md", "epub",
    ],
    &[
        "lua", "luau", "py", "js", "ts", "c", "h", "cpp", "hpp", "rs", "go",
        "java", "sh", "json", "toml", "yaml", "yml",
    ],
    &[
        "deb", "rpm", "pkg", "apk", "appimage", "iso", "img", "jar", "whl",
    ],
];

#[derive(Clone, Debug)]
pub struct FileRec {
    pub n: String,
    pub s: f64,
    pub k: usize,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: String,
    pub n: String,
    pub s: f64,
    pub f: f64,
    pub u: f64,
    pub nf: f64,
    pub c: [f64; 8],
    pub d: Vec<NodeRef>,
    pub t: Vec<FileRec>,
    pub leaf: bool,
}

pub type NodeRef = Arc<Mutex<Node>>;

pub fn lock(node: &NodeRef) -> MutexGuard<'_, Node> {
    node.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub bytes: f64,
    pub kind: String,
    pub node: Option<NodeRef>,
    pub files: Option<f64>,
    pub category: Option<usize>,
    pub index: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct Slice {
    pub label: String,
    pub bytes: f64,
    pub kind: String,
    pub index: Option<usize>,
    pub node: Option<NodeRef>,
    pub color: Option<String>,
}

pub fn child(node: &NodeRef, name: &str) -> Option<NodeRef> {
    lock(node)
        .d
        .iter()
        .find(|item| lock(item).n == name)
        .cloned()
}

pub fn entries(
    node: &NodeRef,
    mode: &str,
    category: Option<usize>,
) -> Vec<Entry> {
    let node = lock(node);
    let mut out = Vec::new();
    if mode == "types" && category.is_some() {
        let category = category.unwrap();
        for file in &node.t {
            if file.k == category {
                out.push(Entry {
                    name: file.n.clone(),
                    bytes: file.s,
                    kind: "file".to_owned(),
                    node: None,
                    files: None,
                    category: Some(category),
                    index: None,
                });
            }
        }
    } else {
        for dir in &node.d {
            let inner = lock(dir);
            out.push(Entry {
                name: inner.n.clone(),
                bytes: inner.s,
                kind: "dir".to_owned(),
                node: Some(Arc::clone(dir)),
                files: Some(inner.nf),
                category: None,
                index: None,
            });
        }
        if node.f > 0.0 {
            let kept: f64 = node.d.iter().map(|dir| lock(dir).nf).sum();
            out.push(Entry {
                name: String::new(),
                bytes: node.f,
                kind: "loose".to_owned(),
                node: None,
                files: Some(node.nf - kept),
                category: None,
                index: None,
            });
        }
    }
    drop(node);
    sort(out, "size", true)
}
pub fn common_dir(entries: &[Entry], count: usize) -> String {
    if count < 1 {
        return String::new();
    }
    let mut prefix = match entries[0].name.rfind('/') {
        Some(index) => entries[0].name[..=index].to_owned(),
        None => String::new(),
    };
    for entry in entries.iter().take(count).skip(1) {
        while !prefix.is_empty() && !entry.name.starts_with(&prefix) {
            let shortened = &prefix[..prefix.len() - 1];
            prefix = match shortened.rfind('/') {
                Some(index) => shortened[..=index].to_owned(),
                None => String::new(),
            };
        }
    }
    prefix
}

fn sort_value(entry: &Entry, key: &str) -> String {
    match key {
        "name" => {
            let last = entry.name.rsplit('/').next().unwrap_or("");
            last.to_lowercase()
        }
        "path" => entry.name.to_lowercase(),
        _ => String::new(),
    }
}

pub fn sort(entries: Vec<Entry>, key: &str, descending: bool) -> Vec<Entry> {
    let mut indexed: Vec<(usize, String, Entry)> = entries
        .into_iter()
        .enumerate()
        .map(|(position, entry)| {
            let value = sort_value(&entry, key);
            (position, value, entry)
        })
        .collect();
    indexed.sort_by(|a, b| {
        let (pos_a, value_a, entry_a) = a;
        let (pos_b, value_b, entry_b) = b;
        if key == "size" {
            if entry_a.bytes != entry_b.bytes {
                return if descending {
                    entry_b
                        .bytes
                        .partial_cmp(&entry_a.bytes)
                        .unwrap_or(Ordering::Equal)
                } else {
                    entry_a
                        .bytes
                        .partial_cmp(&entry_b.bytes)
                        .unwrap_or(Ordering::Equal)
                };
            }
        } else if value_a != value_b {
            return if descending {
                value_b.cmp(value_a)
            } else {
                value_a.cmp(value_b)
            };
        }
        if entry_a.name != entry_b.name {
            return entry_a.name.cmp(&entry_b.name);
        }
        pos_a.cmp(pos_b)
    });
    indexed.into_iter().map(|(_, _, entry)| entry).collect()
}

pub fn jq_program(
    depth: i64,
    top_n: i64,
    min_bytes: i64,
    rel_min: f64,
) -> String {
    let mut ext = Vec::new();
    for (index, list) in EXT.iter().enumerate() {
        for name in *list {
            ext.push(format!("\"{name}\":{}", index + 1));
        }
    }
    let program = r#"@EXT@ as $E
| def cat: split(".") as $p
  | if ($p | length) < 2 or (($p | length) == 2 and $p[0] == "") then 8
    else ($E[$p[-1] | ascii_downcase] // 8) end;
  def size: (.dsize // 0) as $d | if .hlnkc and (.nlink // 1) > 1 then ($d / .nlink | floor) else $d end;
  def node($lvl):
    (.[0] | size) as $own
    | [.[1:][] | select(type == "array") | node($lvl + 1)] as $kids
    | [.[1:][] | select(type == "object" and (.excluded | not)) | {n: .name, s: size, k: (.name | cat)}] as $files
    | ($own + ([$files[].s, $kids[].s] | add // 0)) as $s
    | (if $lvl < @DEPTH@ then [$kids[] | select(.s >= ([@MIN@, @REL@ * $s] | max))] else [] end) as $keep
    | {n: .[0].name, s: $s, f: ($s - ([$keep[].s] | add // 0)), d: $keep,
       nf: (($files | length) + ([$kids[].nf] | add // 0)),
       u: ((if .[0].read_error == true then 1 else 0 end) + ([$kids[].u] | add // 0)),
       c: (reduce $kids[].c as $x (reduce $files[] as $x ([0, 0, 0, 0, 0, 0, 0, $own]; setpath([$x.k - 1]; .[$x.k - 1] + $x.s));
             [range(8) as $j | .[$j] + $x[$j]])),
       t: ([$files[] | select(.s > 0)] + [$kids[] | .n as $p | .t[] | .n = $p + "/" + .n]
           | group_by(.k) | map(sort_by(-.s, .n) | .[:@TOP@]) | add // [])}
    | if $lvl == @DEPTH2@ then .leaf = true else . end;
  def flat($id): ({id: $id} + del(.d)), (.d | to_entries[] | .key as $k | .value | flat($id + "/" + ($k | tostring)));
  .[3] | node(0) | ({v: 4, root: .n}, flat("0"))
"#;
    program
        .replace("@EXT@", &ext.join(","))
        .replace("@DEPTH@", &depth.to_string())
        .replace("@DEPTH2@", &depth.to_string())
        .replace("@MIN@", &min_bytes.to_string())
        .replace("@REL@", &format!("{rel_min:.6}"))
        .replace("@TOP@", &top_n.to_string())
}

fn slice_rows(node: &NodeRef, mode: &str) -> Vec<Entry> {
    if mode == "types" {
        let node = lock(node);
        let mut rows = Vec::new();
        for index in 0..8 {
            let bytes = node.c[index];
            if bytes > 0.0 {
                rows.push(Entry {
                    name: CATEGORIES[index].to_owned(),
                    bytes,
                    kind: "category".to_owned(),
                    node: None,
                    files: None,
                    category: None,
                    index: Some(index + 1),
                });
            }
        }
        return sort(rows, "size", true);
    }
    entries(node, "folders", None)
}

pub fn slices(node: &NodeRef, mode: &str, max_slices: usize) -> Vec<Slice> {
    let node_borrow = lock(node);
    if node_borrow.s <= 0.0 {
        return Vec::new();
    }
    drop(node_borrow);
    let list = slice_rows(node, mode);
    let kept = if list.len() > max_slices {
        max_slices - 1
    } else {
        list.len()
    };
    let mut sum = 0.0;
    let mut out = Vec::new();
    for (index, entry) in list.iter().enumerate().take(kept) {
        sum += entry.bytes;
        let slice_index = entry.index.or(if entry.kind == "dir" {
            Some(index + 1)
        } else {
            None
        });
        out.push(Slice {
            label: entry.name.clone(),
            bytes: entry.bytes,
            kind: entry.kind.clone(),
            index: slice_index,
            node: entry.node.clone(),
            color: slice_index.map(|value| {
                COLORS_DARK[((value - 1) % COLORS_DARK.len())].to_owned()
            }),
        });
    }
    let total = lock(node).s;
    if total > sum {
        out.push(Slice {
            label: "other".to_owned(),
            bytes: total - sum,
            kind: "other".to_owned(),
            index: None,
            node: None,
            color: None,
        });
    }
    out
}
