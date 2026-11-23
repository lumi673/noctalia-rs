use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

#[derive(Clone, Copy)]
pub struct Conversion {
    pub key: &'static str,
    pub name: &'static str,
    pub category: &'static str,
}

#[derive(Clone)]
pub struct ConversionResult {
    pub key: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub value: String,
}

struct Ctx {
    text: String,
    tokens: Vec<String>,
    words: Vec<String>,
    preserve: bool,
}

type ConvertFn = fn(&Ctx) -> String;

struct Definition {
    conversion: Conversion,
    convert: ConvertFn,
}

fn char_bytes(first: u8) -> usize {
    if first < 0xC0 {
        1
    } else if first < 0xE0 {
        2
    } else if first < 0xF0 {
        3
    } else {
        4
    }
}

fn is_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn is_lower(byte: u8) -> bool {
    byte.is_ascii_lowercase()
}

fn is_upper(byte: u8) -> bool {
    byte.is_ascii_uppercase()
}

fn is_alpha(byte: u8) -> bool {
    byte.is_ascii_alphabetic()
}

fn is_digit(byte: u8) -> bool {
    byte.is_ascii_digit()
}

fn is_alnum(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
}

fn upper_byte(byte: u8) -> u8 {
    if is_lower(byte) { byte - 0x20 } else { byte }
}

fn lower_byte(byte: u8) -> u8 {
    if is_upper(byte) { byte + 0x20 } else { byte }
}

fn as_text(bytes: Vec<u8>) -> String {
    String::from_utf8_lossy(&bytes).into_owned()
}

fn map_case(bytes: &[u8], to_upper: bool) -> Vec<u8> {
    if !bytes.iter().any(|&byte| byte >= 0x80) {
        return bytes
            .iter()
            .map(|&byte| {
                if to_upper {
                    upper_byte(byte)
                } else {
                    lower_byte(byte)
                }
            })
            .collect();
    }

    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    let length = bytes.len();
    while index < length {
        let lead = bytes[index];
        if lead < 0x80 {
            let mapped = if to_upper {
                upper_byte(lead)
            } else {
                lower_byte(lead)
            };
            out.push(mapped);
            index += 1;
        } else if lead == 0xC3 && index < length - 1 {
            let mut trail = bytes[index + 1];
            if to_upper && (0xA0..=0xBE).contains(&trail) && trail != 0xB7 {
                trail -= 0x20;
            } else if !to_upper
                && (0x80..=0x9E).contains(&trail)
                && trail != 0x97
            {
                trail += 0x20;
            }
            out.push(lead);
            out.push(trail);
            index += 2;
        } else {
            let size = char_bytes(lead);
            let end = (index + size).min(length);
            out.extend_from_slice(&bytes[index..end]);
            index += size;
        }
    }
    out
}

pub fn upper(text: &str) -> String {
    as_text(map_case(text.as_bytes(), true))
}

pub fn lower(text: &str) -> String {
    as_text(map_case(text.as_bytes(), false))
}

pub fn trim(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut start = 0;
    while start < bytes.len() && is_space(bytes[start]) {
        start += 1;
    }
    let mut end = bytes.len();
    while end > start && is_space(bytes[end - 1]) {
        end -= 1;
    }
    as_text(bytes[start..end].to_vec())
}

fn gsub_lower_upper(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if index + 1 < bytes.len()
            && is_lower(bytes[index])
            && is_upper(bytes[index + 1])
        {
            out.push(bytes[index]);
            out.push(b' ');
            out.push(bytes[index + 1]);
            index += 2;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

fn gsub_digit_upper(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if index + 1 < bytes.len()
            && is_digit(bytes[index])
            && is_upper(bytes[index + 1])
        {
            out.push(bytes[index]);
            out.push(b' ');
            out.push(bytes[index + 1]);
            index += 2;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

fn gsub_upper_upper_lower(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if index + 2 < bytes.len()
            && is_upper(bytes[index])
            && is_upper(bytes[index + 1])
            && is_lower(bytes[index + 2])
        {
            out.push(bytes[index]);
            out.push(b' ');
            out.push(bytes[index + 1]);
            out.push(bytes[index + 2]);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

fn is_separator(byte: u8) -> bool {
    is_space(byte)
        || matches!(
            byte,
            b'_' | b'-' | b'.' | b'/' | b'\\' | b':' | b',' | b';' | b'|'
        )
}

fn gsub_separators(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if is_separator(bytes[index]) {
            out.push(b' ');
            while index < bytes.len() && is_separator(bytes[index]) {
                index += 1;
            }
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

fn park_byte(byte: u8) -> u8 {
    match byte {
        b',' => 1,
        b'.' => 2,
        b';' => 3,
        b':' => 4,
        b'-' => 5,
        _ => byte,
    }
}

fn restore_byte(byte: u8) -> u8 {
    match byte {
        1 => b',',
        2 => b'.',
        3 => b';',
        4 => b':',
        5 => b'-',
        _ => byte,
    }
}

fn park_run(run: &[u8]) -> Vec<u8> {
    run.iter().map(|&byte| park_byte(byte)).collect()
}

fn is_prose_punctuation(byte: u8) -> bool {
    matches!(byte, b',' | b'.' | b';' | b':')
}

fn gsub_word_hyphen(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if index + 2 < bytes.len()
            && is_alnum(bytes[index])
            && bytes[index + 1] == b'-'
            && is_alnum(bytes[index + 2])
        {
            out.push(bytes[index]);
            out.push(5);
            index += 2;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

fn gsub_initials(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let frontier = index == 0 || !is_alnum(bytes[index - 1]);
        if !frontier
            || index + 2 >= bytes.len()
            || !is_alpha(bytes[index])
            || bytes[index + 1] != b'.'
            || !is_alpha(bytes[index + 2])
        {
            out.push(bytes[index]);
            index += 1;
            continue;
        }

        let mut end = index + 3;
        while end < bytes.len() && (is_alpha(bytes[end]) || bytes[end] == b'.')
        {
            end += 1;
        }
        let run = &bytes[index..end];

        let mut leftover = Vec::new();
        let mut cursor = 0;
        while cursor < run.len() {
            if cursor + 1 < run.len()
                && is_alpha(run[cursor])
                && run[cursor + 1] == b'.'
            {
                cursor += 2;
            } else {
                leftover.push(run[cursor]);
                cursor += 1;
            }
        }

        if leftover.len() <= 1 {
            out.extend_from_slice(&park_run(run));
        } else {
            out.extend_from_slice(run);
        }
        index = end;
    }
    out
}

fn gsub_punctuation_before_space(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if is_prose_punctuation(bytes[index]) {
            let mut end = index;
            while end < bytes.len() && is_prose_punctuation(bytes[end]) {
                end += 1;
            }
            if end < bytes.len() && is_space(bytes[end]) {
                out.extend_from_slice(&park_run(&bytes[index..end]));
                index = end;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    out
}

fn gsub_trailing_punctuation(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let mut start = out.len();
    while start > 0 && is_prose_punctuation(out[start - 1]) {
        start -= 1;
    }
    if start < out.len() {
        let parked = park_run(&out[start..]);
        out[start..].copy_from_slice(&parked);
    }
    out
}

fn gsub_decimal_separator(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if index + 2 < bytes.len()
            && is_digit(bytes[index])
            && matches!(bytes[index + 1], b'.' | b',')
            && is_digit(bytes[index + 2])
        {
            out.push(bytes[index]);
            out.push(park_byte(bytes[index + 1]));
            index += 2;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    out
}

fn park_prose(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    if bytes.iter().any(|&byte| is_space(byte)) {
        bytes = gsub_word_hyphen(&bytes);
    }
    bytes = gsub_initials(&bytes);
    bytes = gsub_punctuation_before_space(&bytes);
    bytes = gsub_trailing_punctuation(&bytes);
    bytes = gsub_decimal_separator(&bytes);
    bytes
}

fn restore_controls(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(|&byte| restore_byte(byte)).collect()
}

pub fn tokenize(text: &str, prose: bool) -> Vec<String> {
    let bytes = if prose {
        park_prose(text)
    } else {
        text.as_bytes().to_vec()
    };
    let spaced = gsub_separators(&bytes);
    let spaced = gsub_lower_upper(&spaced);
    let spaced = gsub_digit_upper(&spaced);
    let spaced = gsub_upper_upper_lower(&spaced);

    let mut tokens = Vec::new();
    let mut index = 0;
    while index < spaced.len() {
        if is_space(spaced[index]) {
            index += 1;
            continue;
        }
        let mut end = index;
        while end < spaced.len() && !is_space(spaced[end]) {
            end += 1;
        }
        let token = if prose {
            as_text(restore_controls(&spaced[index..end]))
        } else {
            as_text(spaced[index..end].to_vec())
        };
        tokens.push(token);
        index = end;
    }
    tokens
}

fn split_first(word: &str) -> (&str, &str) {
    let Some(&lead) = word.as_bytes().first() else {
        return ("", "");
    };
    let size = char_bytes(lead).min(word.len());
    (&word[..size], &word[size..])
}

fn upper_first(word: &str) -> String {
    let (first, rest) = split_first(word);
    format!("{}{rest}", upper(first))
}

fn lower_first(word: &str) -> String {
    let (first, rest) = split_first(word);
    format!("{}{rest}", lower(first))
}

fn norm(word: &str, preserve: bool) -> String {
    if preserve {
        word.to_owned()
    } else {
        lower(word)
    }
}

fn join(
    tokens: &[String],
    separator: &str,
    transform: impl Fn(&str, usize, usize) -> String,
) -> String {
    let total = tokens.len();
    let mut parts = Vec::with_capacity(total);
    for (index, token) in tokens.iter().enumerate() {
        parts.push(transform(token, index + 1, total));
    }
    parts.join(separator)
}

fn swap_case(text: &str) -> String {
    let bytes = text
        .as_bytes()
        .iter()
        .map(|&byte| {
            if is_alpha(byte) {
                if is_upper(byte) {
                    lower_byte(byte)
                } else {
                    upper_byte(byte)
                }
            } else {
                byte
            }
        })
        .collect();
    as_text(bytes)
}

fn alternating_case(text: &str) -> String {
    let mut out = Vec::with_capacity(text.len());
    let mut letters = 0;
    for &byte in text.as_bytes() {
        if is_alpha(byte) {
            letters += 1;
            if letters % 2 == 1 {
                out.push(upper_byte(byte));
            } else {
                out.push(lower_byte(byte));
            }
        } else {
            out.push(byte);
        }
    }
    as_text(out)
}

fn camel(ctx: &Ctx) -> String {
    join(&ctx.tokens, "", |token, index, _| {
        let word = norm(token, ctx.preserve);
        if index == 1 {
            lower_first(&word)
        } else {
            upper_first(&word)
        }
    })
}

fn pascal(ctx: &Ctx) -> String {
    join(&ctx.tokens, "", |token, _, _| {
        upper_first(&norm(token, ctx.preserve))
    })
}

fn kebab(ctx: &Ctx) -> String {
    join(&ctx.tokens, "-", |token, _, _| lower(token))
}

fn snake(ctx: &Ctx) -> String {
    join(&ctx.tokens, "_", |token, _, _| lower(token))
}

fn constant(ctx: &Ctx) -> String {
    join(&ctx.tokens, "_", |token, _, _| upper(token))
}

fn dot(ctx: &Ctx) -> String {
    join(&ctx.tokens, ".", |token, _, _| lower(token))
}

fn path(ctx: &Ctx) -> String {
    join(&ctx.tokens, "/", |token, _, _| lower(token))
}

fn pascal_snake(ctx: &Ctx) -> String {
    join(&ctx.tokens, "_", |token, _, _| {
        upper_first(&norm(token, ctx.preserve))
    })
}

fn kebab_upper(ctx: &Ctx) -> String {
    join(&ctx.tokens, "-", |token, _, _| upper(token))
}

fn header(ctx: &Ctx) -> String {
    join(&ctx.tokens, "-", |token, _, _| {
        upper_first(&norm(token, ctx.preserve))
    })
}

fn small_words() -> &'static HashSet<&'static str> {
    static SMALL_WORDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    SMALL_WORDS.get_or_init(|| {
        [
            "a", "an", "and", "as", "at", "but", "by", "en", "for", "from",
            "if", "in", "into", "nor", "of", "on", "onto", "or", "over", "per",
            "so", "the", "to", "up", "v", "via", "vs", "with", "yet",
        ]
        .into_iter()
        .collect()
    })
}

fn title(ctx: &Ctx) -> String {
    join(&ctx.words, " ", |token, index, total| {
        let lowered = lower(token);
        if index > 1 && index < total {
            let bytes = lowered.as_bytes();
            let mut end = bytes.len();
            while end > 0 && is_prose_punctuation(bytes[end - 1]) {
                end -= 1;
            }
            if small_words().contains(&lowered[..end]) {
                return lowered;
            }
        }
        upper_first(&norm(token, ctx.preserve))
    })
}

fn capital(ctx: &Ctx) -> String {
    join(&ctx.words, " ", |token, _, _| {
        upper_first(&norm(token, ctx.preserve))
    })
}

fn sentence(ctx: &Ctx) -> String {
    join(&ctx.words, " ", |token, index, _| {
        let word = norm(token, ctx.preserve);
        if index == 1 { upper_first(&word) } else { word }
    })
}

fn no_case(ctx: &Ctx) -> String {
    join(&ctx.words, " ", |token, _, _| lower(token))
}

fn lower_case(ctx: &Ctx) -> String {
    lower(&ctx.text)
}

fn upper_case(ctx: &Ctx) -> String {
    upper(&ctx.text)
}

fn swap(ctx: &Ctx) -> String {
    swap_case(&ctx.text)
}

fn alternating(ctx: &Ctx) -> String {
    alternating_case(&ctx.text)
}

static DEFINITIONS: &[Definition] = &[
    Definition {
        conversion: Conversion {
            key: "camel",
            name: "camelCase",
            category: "Programming",
        },
        convert: camel,
    },
    Definition {
        conversion: Conversion {
            key: "pascal",
            name: "PascalCase",
            category: "Programming",
        },
        convert: pascal,
    },
    Definition {
        conversion: Conversion {
            key: "kebab",
            name: "kebab-case",
            category: "Programming",
        },
        convert: kebab,
    },
    Definition {
        conversion: Conversion {
            key: "snake",
            name: "snake_case",
            category: "Programming",
        },
        convert: snake,
    },
    Definition {
        conversion: Conversion {
            key: "constant",
            name: "CONSTANT_CASE",
            category: "Programming",
        },
        convert: constant,
    },
    Definition {
        conversion: Conversion {
            key: "dot",
            name: "dot.case",
            category: "Programming",
        },
        convert: dot,
    },
    Definition {
        conversion: Conversion {
            key: "path",
            name: "path/case",
            category: "Programming",
        },
        convert: path,
    },
    Definition {
        conversion: Conversion {
            key: "pascal_snake",
            name: "Pascal_Snake_Case",
            category: "Programming",
        },
        convert: pascal_snake,
    },
    Definition {
        conversion: Conversion {
            key: "kebab_upper",
            name: "KEBAB-UPPER-CASE",
            category: "Programming",
        },
        convert: kebab_upper,
    },
    Definition {
        conversion: Conversion {
            key: "header",
            name: "Header-Case",
            category: "Programming",
        },
        convert: header,
    },
    Definition {
        conversion: Conversion {
            key: "title",
            name: "Title Case",
            category: "Text",
        },
        convert: title,
    },
    Definition {
        conversion: Conversion {
            key: "capital",
            name: "Capital Case",
            category: "Text",
        },
        convert: capital,
    },
    Definition {
        conversion: Conversion {
            key: "sentence",
            name: "Sentence case",
            category: "Text",
        },
        convert: sentence,
    },
    Definition {
        conversion: Conversion {
            key: "no_case",
            name: "no case",
            category: "Text",
        },
        convert: no_case,
    },
    Definition {
        conversion: Conversion {
            key: "lower",
            name: "lower case",
            category: "Text",
        },
        convert: lower_case,
    },
    Definition {
        conversion: Conversion {
            key: "upper",
            name: "UPPER CASE",
            category: "Text",
        },
        convert: upper_case,
    },
    Definition {
        conversion: Conversion {
            key: "swap",
            name: "sWAP cASE",
            category: "Fun",
        },
        convert: swap,
    },
    Definition {
        conversion: Conversion {
            key: "alternating",
            name: "AlTeRnAtInG cAsE",
            category: "Fun",
        },
        convert: alternating,
    },
];

pub fn category_ranks(order: &str) -> HashMap<String, usize> {
    let categories = ["Programming", "Text", "Fun"];
    let mut known = HashMap::new();
    for category in categories {
        known.insert(lower(category), category.to_owned());
    }

    let mut ranks = HashMap::new();
    let mut rank = 0;
    for name in order.split(',').filter(|part| !part.is_empty()) {
        let trimmed = lower(&trim(name));
        let Some(category) = known.get(&trimmed).cloned() else {
            continue;
        };
        if !ranks.contains_key(&category) {
            rank += 1;
            ranks.insert(category, rank);
        }
    }
    for category in categories {
        if !ranks.contains_key(category) {
            rank += 1;
            ranks.insert(category.to_owned(), rank);
        }
    }
    ranks
}

pub fn convert_all(text: &str, preserve: bool) -> Vec<ConversionResult> {
    let trimmed = trim(text);
    let ctx = Ctx {
        text: trimmed.clone(),
        tokens: tokenize(&trimmed, false),
        words: tokenize(&trimmed, true),
        preserve,
    };

    let mut results = Vec::new();
    for definition in DEFINITIONS {
        let value = (definition.convert)(&ctx);
        if !value.is_empty() {
            results.push(ConversionResult {
                key: definition.conversion.key,
                name: definition.conversion.name,
                category: definition.conversion.category,
                value,
            });
        }
    }
    results
}
