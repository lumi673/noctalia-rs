//! Lowering for `#[derive(State)]` helpers.
//!
//! The compiler reads Rust source before proc macros run, so the derive is
//! interpreted from the struct definition. Every helper is a direct host state
//! call plus a typed table built from fields with proven Luau literals. An
//! unprovable default stops the build with the field's source position.

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::{Expr, ExprLit, Lit, Type, UnOp};

use super::*;
use crate::program::{StateDef, StateFieldDef};

impl<'a> Emitter<'a> {
    /// Associated calls such as `Counter::load(&ctx)`.
    pub(super) fn derived_state_call(
        &mut self,
        call: &ExprCall,
        args: &[&Expr],
    ) -> Result<OptionalValue, Error> {
        let Expr::Path(path) = &*call.func else {
            return Ok(None);
        };
        let Some(segments) = path_segments(&path.path) else {
            return Ok(None);
        };
        if segments.len() < 2 {
            return Ok(None);
        }
        let method = segments.last().map_or("", String::as_str);
        let class = segments[segments.len() - 2].as_str();
        let Some(def) = self.program.state_def(&self.module, class).cloned()
        else {
            return Ok(None);
        };
        if !matches!(method, "load" | "defaults" | "from_value" | "watch") {
            return Ok(None);
        }
        if matches!(method, "load" | "defaults" | "from_value") {
            self.pending_class = Some(class.to_owned());
        }
        self.state_call(&def, method, args, call.span()).map(Some)
    }

    /// Instance calls such as `counter.save(&mut ops)`.
    pub(super) fn derived_state_method(
        &mut self,
        call: &ExprMethodCall,
    ) -> Result<OptionalValue, Error> {
        if call.method != "save" {
            return Ok(None);
        }
        let Some(class) = self.class_of_receiver(&call.receiver) else {
            return Ok(None);
        };
        let Some(def) = self.program.state_def(&self.module, &class).cloned()
        else {
            return Ok(None);
        };
        let (receiver, _) = self.expr(&call.receiver)?;
        let source = self.temp();
        self.line(&format!("local {source} = {receiver}"));
        let table = self.state_table(&def, &source)?;
        self.line(&format!("noctalia.state.set({}, {table})", quote(&def.key)));
        Ok(Some((String::new(), Ty::VOID)))
    }

    fn state_call(
        &mut self,
        def: &StateDef,
        method: &str,
        args: &[&Expr],
        span: proc_macro2::Span,
    ) -> Result<(String, Ty), Error> {
        if let Some((error_span, message)) = &def.error {
            return Err(Error::at(*error_span, message.clone()));
        }
        match method {
            "defaults" => {
                self.expect_args(method, args.len(), 0, span)?;
                self.default_table(def).map(|table| (table, Ty::TABLE))
            }
            "from_value" => {
                self.expect_args(method, args.len(), 1, span)?;
                self.decode(def, args[0])
                    .map(|temporary| (temporary, Ty::TABLE))
            }
            "load" => {
                self.expect_args(method, args.len(), 1, span)?;
                let source = format!("noctalia.state.get({})", quote(&def.key));
                let value = self.merge(def, &source)?;
                Ok((value, Ty::TABLE))
            }
            "watch" => {
                self.expect_args(method, args.len(), 2, span)?;
                let callback = args[1];
                let marker = if self.strict() {
                    self.strict_callback(callback, callback.span())?
                } else {
                    let id = self.expr(callback)?.0;
                    format!("{{ __host_callback = {id} }}")
                };
                self.line(&format!(
                    "noctalia.state.watch({}, {marker})",
                    quote(&def.key)
                ));
                Ok((String::new(), Ty::VOID))
            }
            _ => unreachable!("matched methods only"),
        }
    }

    fn expect_args(
        &self,
        method: &str,
        found: usize,
        wanted: usize,
        span: Span,
    ) -> Result<(), Error> {
        if found == wanted {
            return Ok(());
        }
        Err(Error::at(
            span,
            format!(
                "State::{method} takes {wanted} arguments but {found} were passed"
            ),
        ))
    }

    /// A table with one proven default per emitted field.
    fn default_table(&self, def: &StateDef) -> Result<String, Error> {
        let mut fields = Vec::new();
        for field in &def.fields {
            if field.skip {
                continue;
            }
            if let Some(literal) = field_literal(field)? {
                fields.push(format!("[{}] = {literal}", quote(&field.name)));
            }
        }
        Ok(format!("{{ {} }}", fields.join(", ")))
    }

    /// Copies a state snapshot into a fresh table, filling absent fields from
    /// the derived defaults. The host value is read exactly once.
    fn merge(&mut self, def: &StateDef, source: &str) -> Result<String, Error> {
        let input = self.temp();
        self.line(&format!("local {input} = {source}"));
        let output = self.temp();
        self.line(&format!("local {output} = {{}}"));
        self.merge_fields(def, &input, &output)?;
        Ok(output)
    }

    fn decode(
        &mut self,
        def: &StateDef,
        value: &Expr,
    ) -> Result<String, Error> {
        let (source, _) = self.expr(value)?;
        self.merge(def, &source)
    }

    fn merge_fields(
        &mut self,
        def: &StateDef,
        input: &str,
        output: &str,
    ) -> Result<(), Error> {
        if let Some((error_span, message)) = &def.error {
            return Err(Error::at(*error_span, message.clone()));
        }
        for field in &def.fields {
            let label = quote(&field.name);
            let literal = field_literal(field)?;
            if field.skip {
                if let Some(literal) = literal {
                    self.line(&format!("{output}[{label}] = {literal}"));
                }
                continue;
            }
            let read = format!("{input}[{label}]");
            let kind = literal_kind(field, field.span)?;
            let guard = format!(
                "type({input}) == \"table\" and {} and {read} ~= nil",
                type_check(&read, kind)
            );
            match literal {
                Some(literal) => self.line(&format!(
                    "if {guard} then {output}[{label}] = {read} else {output}[{label}] = {literal} end"
                )),
                None => self.line(&format!(
                    "if {guard} then {output}[{label}] = {read} end"
                )),
            }
        }
        Ok(())
    }

    fn state_table(
        &self,
        def: &StateDef,
        source: &str,
    ) -> Result<String, Error> {
        if let Some((error_span, message)) = &def.error {
            return Err(Error::at(*error_span, message.clone()));
        }
        let mut fields = Vec::new();
        for field in &def.fields {
            if field.skip {
                continue;
            }
            let _ = field_literal(field)?;
            let label = quote(&field.name);
            fields.push(format!("[{label}] = {source}[{label}]"));
        }
        Ok(format!("{{ {} }}", fields.join(", ")))
    }
}

/// The Luau literal for a field default, or `None` when the key is absent.
fn field_literal(field: &StateFieldDef) -> Result<Option<String>, Error> {
    let inner = option_inner(&field.ty);
    let ty = inner.unwrap_or(&field.ty);
    let name = type_text(ty).to_ascii_lowercase();
    let kind = literal_kind(field, field.span)?;
    let value = if inner.is_some() {
        match field.default.as_ref() {
            None => return Ok(None),
            Some(expr) if is_none_path(expr) => return Ok(None),
            Some(expr) => {
                let inner = some_inner(expr).ok_or_else(|| {
                    Error::at(
                        expr.span(),
                        "Option state default must be None or Some(literal)",
                    )
                })?;
                literal(inner, &name, kind, field.span)?
            }
        }
    } else {
        match field.default.as_ref() {
            Some(expr) => literal(expr, &name, kind, field.span)?,
            None => type_default(kind),
        }
    };
    Ok(Some(value))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LiteralKind {
    Bool,
    Int,
    Float,
    Str,
    List,
    Map,
}

fn type_check(value: &str, kind: LiteralKind) -> String {
    match kind {
        LiteralKind::Bool => format!("type({value}) == \"boolean\""),
        LiteralKind::Int => {
            format!("type({value}) == \"number\" and {value} % 1 == 0")
        }
        LiteralKind::Float => format!("type({value}) == \"number\""),
        LiteralKind::List | LiteralKind::Map => {
            format!("type({value}) == \"table\"")
        }
        LiteralKind::Str => format!("type({value}) == \"string\""),
    }
}

fn literal_kind(
    field: &StateFieldDef,
    span: Span,
) -> Result<LiteralKind, Error> {
    let ty = option_inner(&field.ty).unwrap_or(&field.ty);
    let name = type_text(ty).to_ascii_lowercase();
    match name.as_str() {
        "bool" => Ok(LiteralKind::Bool),
        "i8" | "i16" | "i32" | "i64" | "isize" | "u8" | "u16" | "u32"
        | "u64" | "usize" => Ok(LiteralKind::Int),
        "f32" | "f64" => Ok(LiteralKind::Float),
        "string" | "str" => Ok(LiteralKind::Str),
        "vec" => Ok(LiteralKind::List),
        "hashmap" | "btreemap" | "map" => Ok(LiteralKind::Map),
        _ => Err(Error::at(
            span,
            format!("cannot prove a Luau default for field of type `{name}`"),
        )),
    }
}

fn option_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    if path.path.segments.last()?.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) =
        &path.path.segments.last()?.arguments
    else {
        return None;
    };
    match arguments.args.first()? {
        syn::GenericArgument::Type(inner) => Some(inner),
        _ => None,
    }
}

fn is_none_path(expr: &Expr) -> bool {
    matches!(expr, Expr::Path(path) if path.path.is_ident("None"))
}

fn some_inner(expr: &Expr) -> Option<&Expr> {
    let Expr::Call(call) = expr else {
        return None;
    };
    if !path_last_name(&call.func).is_some_and(|name| name.ends_with("Some")) {
        return None;
    }
    call.args.first()
}

fn literal(
    expr: &Expr,
    name: &str,
    kind: LiteralKind,
    span: Span,
) -> Result<String, Error> {
    match kind {
        LiteralKind::Bool => match expr {
            Expr::Lit(ExprLit {
                lit: Lit::Bool(value),
                ..
            }) => Ok(value.value.to_string()),
            other => Err(Error::at(
                other.span(),
                "state default must be a boolean literal",
            )),
        },
        LiteralKind::Int => integer_literal(expr, name, span),
        LiteralKind::Float => float_literal(expr, span),
        LiteralKind::Str => match expr {
            Expr::Lit(ExprLit {
                lit: Lit::Str(value),
                ..
            }) => Ok(quote(&value.value())),
            other => Err(Error::at(
                other.span(),
                "state default must be a string literal",
            )),
        },
        LiteralKind::List | LiteralKind::Map => {
            if empty_constructor(expr) {
                if kind == LiteralKind::List {
                    Ok(empty_array())
                } else {
                    Ok("{}".to_owned())
                }
            } else {
                Err(Error::at(
                    expr.span(),
                    "state default for a container must be an empty constructor",
                ))
            }
        }
    }
}

fn integer_literal(
    expr: &Expr,
    name: &str,
    span: Span,
) -> Result<String, Error> {
    let (negative, lit) = match expr {
        Expr::Lit(ExprLit {
            lit: value @ Lit::Int(_),
            ..
        }) => (false, value),
        Expr::Unary(unary) if matches!(unary.op, UnOp::Neg(_)) => {
            match &*unary.expr {
                Expr::Lit(ExprLit {
                    lit: value @ Lit::Int(_),
                    ..
                }) => (true, value),
                other => {
                    return Err(Error::at(
                        other.span(),
                        "state default must be an integer literal",
                    ));
                }
            }
        }
        other => {
            return Err(Error::at(
                other.span(),
                "state default must be an integer literal",
            ));
        }
    };
    let Lit::Int(number) = lit else {
        return Err(Error::at(
            span,
            "state default must be an integer literal",
        ));
    };
    let value = number.base10_parse::<i128>().map_err(|error| {
        Error::at(number.span(), format!("invalid integer default: {error}"))
    })?;
    let value = if negative { -value } else { value };
    let unsigned = name.starts_with('u');
    if unsigned && value < 0 {
        return Err(Error::at(
            span,
            format!("default {value} does not fit in `{name}`"),
        ));
    }
    if value < i64::MIN as i128 || value > i64::MAX as i128 {
        return Err(Error::at(
            span,
            format!("default {value} has no exact Luau integer literal"),
        ));
    }
    Ok(value.to_string())
}

fn float_literal(expr: &Expr, span: Span) -> Result<String, Error> {
    let Expr::Lit(ExprLit {
        lit: Lit::Float(value),
        ..
    }) = expr
    else {
        return Err(Error::at(
            expr.span(),
            "state default must be a float literal",
        ));
    };
    let number = value.base10_parse::<f64>().map_err(|error| {
        Error::at(value.span(), format!("invalid float default: {error}"))
    })?;
    if !number.is_finite() {
        return Err(Error::at(span, "state default must be finite"));
    }
    let text = format!("{number:?}");
    Ok(if text.contains('.') || text.contains('e') {
        text
    } else {
        format!("{text}.0")
    })
}

fn type_default(kind: LiteralKind) -> String {
    match kind {
        LiteralKind::Bool => "false".to_owned(),
        LiteralKind::Int => "0".to_owned(),
        LiteralKind::Float => "0.0".to_owned(),
        LiteralKind::Str => "\"\"".to_owned(),
        LiteralKind::List => empty_array(),
        LiteralKind::Map => "{}".to_owned(),
    }
}

fn empty_constructor(expr: &Expr) -> bool {
    match expr {
        Expr::Call(call) => {
            call.args.is_empty()
                && path_last_name(&call.func).is_some_and(|name| {
                    name == "new" || name.ends_with("::new")
                })
        }
        Expr::Macro(mac) => {
            macro_name(mac) == "vec" && mac.mac.tokens.is_empty()
        }
        _ => false,
    }
}
