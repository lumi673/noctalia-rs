//! Parsing of `#[derive(Config)]` structs shared by the compiler, the derive
//! macro and the build's manifest validation.
//!
//! The compiler reads plugin source before macro expansion, so generated impls
//! are invisible to it. All three sides parse the same source attributes here.

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::{
    Data, DeriveInput, Expr, ExprLit, ExprUnary, Fields, Lit, Meta, Token,
    UnOp, punctuated::Punctuated,
};

/// The typed config values a strict table read can lower without guessing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigKind {
    Str,
    Bool,
    I32,
    U32,
    F64,
}

impl ConfigKind {
    /// The manifest setting types this field accepts.
    #[must_use]
    pub const fn setting_types(self) -> &'static [&'static str] {
        match self {
            Self::Str => {
                &["string", "select", "file", "folder", "glyph", "color"]
            }
            Self::Bool => &["bool"],
            Self::I32 | Self::U32 => &["int"],
            Self::F64 => &["double"],
        }
    }
}

/// A literal default, as written in `#[config(default = ...)]`.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigLiteral {
    Str(String),
    Bool(bool),
    Int(i128),
    Float(f64),
}

impl ConfigLiteral {
    /// The Luau literal for a default, used in generated table reads.
    #[must_use]
    pub fn lua(&self) -> String {
        match self {
            Self::Str(value) => crate::quote(value),
            Self::Bool(value) => value.to_string(),
            Self::Int(value) => value.to_string(),
            Self::Float(value) => format!("{value:?}"),
        }
    }

    /// The integer range a config field can hold.
    #[must_use]
    pub const fn integer_range(kind: ConfigKind) -> Option<(i128, i128)> {
        match kind {
            ConfigKind::I32 => Some((i32::MIN as i128, i32::MAX as i128)),
            ConfigKind::U32 => Some((0, u32::MAX as i128)),
            ConfigKind::Str | ConfigKind::Bool | ConfigKind::F64 => None,
        }
    }
}

/// One typed config field.
#[derive(Clone, Debug)]
pub struct ConfigField {
    pub name: String,
    pub key: String,
    pub kind: ConfigKind,
    pub default: ConfigLiteral,
    pub span: Span,
}

/// A struct that derives `Config`.
#[derive(Clone, Debug)]
pub struct ConfigDef {
    pub name: String,
    pub fields: Vec<ConfigField>,
}

/// Parses an `ItemStruct`, as found by the compiler and build scanner.
pub fn parse_struct(item: &syn::ItemStruct) -> syn::Result<Option<ConfigDef>> {
    let input = DeriveInput {
        attrs: item.attrs.clone(),
        vis: item.vis.clone(),
        ident: item.ident.clone(),
        generics: item.generics.clone(),
        data: Data::Struct(syn::DataStruct {
            struct_token: item.struct_token,
            fields: item.fields.clone(),
            semi_token: item.semi_token,
        }),
    };
    parse(&input)
}

/// Parses a derive input, returning `None` when it does not derive `Config`.
pub fn parse(input: &DeriveInput) -> syn::Result<Option<ConfigDef>> {
    if !derives_config(&input.attrs) {
        return Ok(None);
    }
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new_spanned(
            input,
            "Config can only be derived for a struct with named fields",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(syn::Error::new_spanned(
            &data.fields,
            "Config needs a struct with named fields",
        ));
    };
    let mut parsed = Vec::new();
    for field in &fields.named {
        let Some(name) = field.ident.as_ref() else {
            continue;
        };
        let kind = kind(&field.ty)?;
        let mut key = None;
        let mut default = None;
        for attr in &field.attrs {
            if !attr.path().is_ident("config") {
                continue;
            }
            let Meta::List(list) = &attr.meta else {
                return Err(syn::Error::new_spanned(
                    attr,
                    "expected #[config(key = ..., default = ...)]",
                ));
            };
            let items = list
                .parse_args_with(
                    Punctuated::<Meta, Token![,]>::parse_terminated,
                )
                .map_err(|error| {
                    syn::Error::new_spanned(attr, error.to_string())
                })?;
            for item in items {
                let Meta::NameValue(value) = item else {
                    return Err(syn::Error::new_spanned(
                        item,
                        "config attributes must be key = value pairs",
                    ));
                };
                let label = value
                    .path
                    .get_ident()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                match label.as_str() {
                    "key" if key.is_none() => {
                        key = Some(string_expr(&value.value)?);
                    }
                    "default" if default.is_none() => {
                        default = Some(literal_expr(&value.value, kind)?);
                    }
                    "key" | "default" => {
                        return Err(syn::Error::new_spanned(
                            value.value,
                            format!("duplicate config {label}"),
                        ));
                    }
                    other => {
                        return Err(syn::Error::new_spanned(
                            value.value,
                            format!("unknown config attribute {other}"),
                        ));
                    }
                }
            }
        }
        let default = default.ok_or_else(|| {
            syn::Error::new_spanned(
                field,
                "config field needs #[config(default = ...)]",
            )
        })?;
        parsed.push(ConfigField {
            name: name.to_string(),
            key: key.unwrap_or_else(|| name.to_string()),
            kind,
            default,
            span: field.span(),
        });
    }
    if parsed.is_empty() {
        return Err(syn::Error::new_spanned(
            &data.fields,
            "Config needs at least one named field",
        ));
    }
    let mut seen = std::collections::BTreeSet::new();
    for field in &parsed {
        if !seen.insert(field.key.clone()) {
            return Err(syn::Error::new(
                field.span,
                format!("duplicate config key `{}`", field.key),
            ));
        }
    }
    Ok(Some(ConfigDef {
        name: input.ident.to_string(),
        fields: parsed,
    }))
}

/// Parses every `Config` struct in a file, including inline modules.
pub fn parse_file(file: &syn::File) -> syn::Result<Vec<ConfigDef>> {
    fn visit(items: &[syn::Item], out: &mut Vec<ConfigDef>) -> syn::Result<()> {
        for item in items {
            match item {
                syn::Item::Struct(item_struct) => {
                    if let Some(def) = parse_struct(item_struct)? {
                        out.push(def);
                    }
                }
                syn::Item::Mod(item_mod) => {
                    if let Some((_, inner)) = &item_mod.content {
                        visit(inner, out)?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    let mut out = Vec::new();
    visit(&file.items, &mut out)?;
    Ok(out)
}

fn derives_config(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| {
        let Meta::List(list) = &attr.meta else {
            return false;
        };
        if !list.path.is_ident("derive") {
            return false;
        }
        list.tokens.to_string().split(',').any(|name| {
            name.rsplit("::")
                .next()
                .is_some_and(|part| part.trim() == "Config")
        })
    })
}

fn kind(ty: &syn::Type) -> syn::Result<ConfigKind> {
    let syn::Type::Path(path) = ty else {
        return Err(syn::Error::new_spanned(
            ty,
            "unsupported config type: expected String, bool, i32, u32 or f64",
        ));
    };
    let Some(segment) = path.path.segments.last() else {
        return Err(syn::Error::new_spanned(
            ty,
            "unsupported config type: expected String, bool, i32, u32 or f64",
        ));
    };
    match segment.ident.to_string().as_str() {
        "String" => Ok(ConfigKind::Str),
        "bool" => Ok(ConfigKind::Bool),
        "i32" => Ok(ConfigKind::I32),
        "u32" => Ok(ConfigKind::U32),
        "f64" => Ok(ConfigKind::F64),
        _ => Err(syn::Error::new_spanned(
            ty,
            "unsupported config type: expected String, bool, i32, u32 or f64",
        )),
    }
}

fn string_expr(expr: &Expr) -> syn::Result<String> {
    let Expr::Lit(ExprLit {
        lit: Lit::Str(value),
        ..
    }) = strip(expr)
    else {
        return Err(syn::Error::new_spanned(
            expr,
            "config key must be a string literal",
        ));
    };
    Ok(value.value())
}

fn literal_expr(expr: &Expr, kind: ConfigKind) -> syn::Result<ConfigLiteral> {
    let mut sign = 1_i128;
    let mut current = strip(expr);
    if let Expr::Unary(ExprUnary {
        op: UnOp::Neg(_),
        expr,
        ..
    }) = current
    {
        sign = -1;
        current = strip(expr);
    }
    let Expr::Lit(ExprLit { lit, .. }) = current else {
        return Err(syn::Error::new_spanned(
            expr,
            "config default must be a literal",
        ));
    };
    match (kind, lit) {
        (ConfigKind::Str, Lit::Str(value)) => {
            Ok(ConfigLiteral::Str(value.value()))
        }
        (ConfigKind::Bool, Lit::Bool(value)) => {
            Ok(ConfigLiteral::Bool(value.value))
        }
        (ConfigKind::I32 | ConfigKind::U32, Lit::Int(value)) => {
            let parsed = value.base10_parse::<i128>()?.saturating_mul(sign);
            let range = ConfigLiteral::integer_range(kind)
                .expect("integer config kinds have a range");
            if !(range.0..=range.1).contains(&parsed) {
                return Err(syn::Error::new_spanned(
                    expr,
                    format!("config default is outside the {kind:?} range"),
                ));
            }
            Ok(ConfigLiteral::Int(parsed))
        }
        (ConfigKind::F64, Lit::Int(value)) => {
            let parsed = value.base10_parse::<f64>()?;
            Ok(ConfigLiteral::Float(parsed * sign as f64))
        }
        (ConfigKind::F64, Lit::Float(value)) => {
            let parsed = value.base10_parse::<f64>()?;
            Ok(ConfigLiteral::Float(parsed * sign as f64))
        }
        _ => Err(syn::Error::new_spanned(
            expr,
            format!("config default does not match {kind:?}"),
        )),
    }
}

fn strip(expr: &Expr) -> &Expr {
    match expr {
        Expr::Group(group) => strip(&group.expr),
        Expr::Paren(paren) => strip(&paren.expr),
        _ => expr,
    }
}
