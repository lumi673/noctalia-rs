//! Const evaluation for the typed frontend.
//!
//! A const is lowered only when its integer width, signedness and value are
//! known. Overflow, division by zero, unknown paths and unsupported forms are
//! positioned errors instead of silently wrapping.

use std::collections::BTreeMap;

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::visit_mut::{self, VisitMut};
use syn::{BinOp, Expr, File, Item, Lit, Type, UnOp};

use crate::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum IntType {
    I8,
    I16,
    I32,
    I64,
    Isize,
    U8,
    U16,
    U32,
    U64,
    Usize,
    U128,
}

impl IntType {
    fn min(self) -> i128 {
        match self {
            Self::I8 => i8::MIN.into(),
            Self::I16 => i16::MIN.into(),
            Self::I32 => i32::MIN.into(),
            Self::I64 => i64::MIN.into(),
            Self::Isize => i64::MIN.into(),
            _ => 0,
        }
    }

    fn max(self) -> i128 {
        match self {
            Self::I8 => i8::MAX.into(),
            Self::I16 => i16::MAX.into(),
            Self::I32 => i32::MAX.into(),
            Self::I64 | Self::Isize => i64::MAX.into(),
            Self::U8 => u8::MAX.into(),
            Self::U16 => u16::MAX.into(),
            Self::U32 => u32::MAX.into(),
            Self::U64 | Self::Usize => u64::MAX.into(),
            Self::U128 => i128::MAX,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum ConstValue {
    Int(i128, IntType, bool),
    Bool(bool),
    Str(String),
}

impl ConstValue {
    fn int(&self) -> Option<(i128, IntType, bool)> {
        match self {
            Self::Int(value, ty, explicit) => Some((*value, *ty, *explicit)),
            _ => None,
        }
    }
    fn int_value(&self) -> i128 {
        match self {
            Self::Int(value, _, _) => *value,
            _ => 0,
        }
    }

    fn boolean(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The value rendered as a Luau literal.
    pub(super) fn literal(&self) -> String {
        match self {
            Self::Int(value, _, _) => value.to_string(),
            Self::Bool(true) => "true".to_owned(),
            Self::Bool(false) => "false".to_owned(),
            Self::Str(text) => crate::quote(text),
        }
    }
}

#[derive(Default)]
pub(super) struct ConstEnv {
    values: BTreeMap<String, Expr>,
    associated: BTreeMap<(String, String), Expr>,
    types: BTreeMap<String, IntType>,
}

impl ConstEnv {
    pub(super) fn new(files: &[File]) -> Self {
        let mut env = Self::default();
        for file in files {
            for item in &file.items {
                match item {
                    Item::Const(item) => {
                        let name = item.ident.to_string();
                        if let Some(ty) = int_type(&item.ty) {
                            env.types.insert(name.clone(), ty);
                        }
                        env.values.insert(name, (*item.expr).clone());
                    }
                    Item::Impl(item) => {
                        let Some(class) = class_name(&item.self_ty) else {
                            continue;
                        };
                        for member in &item.items {
                            if let syn::ImplItem::Const(value) = member {
                                env.associated.insert(
                                    (class.clone(), value.ident.to_string()),
                                    value.expr.clone(),
                                );
                                if let Some(ty) = int_type(&value.ty) {
                                    env.types
                                        .insert(value.ident.to_string(), ty);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        env
    }

    fn coerce(
        &self,
        value: ConstValue,
        name: &str,
        span: Span,
    ) -> Result<ConstValue, Error> {
        self.coerce_declared(value, self.types.get(name).copied(), span)
    }

    pub(super) fn coerce_declared(
        &self,
        value: ConstValue,
        ty: Option<IntType>,
        span: Span,
    ) -> Result<ConstValue, Error> {
        if let (Some(ty), ConstValue::Int(number, _, _)) = (ty, &value) {
            checked(*number, ty, true, span)
        } else {
            Ok(value)
        }
    }

    pub(super) fn eval(
        &self,
        expr: &Expr,
        substitutions: &BTreeMap<String, ConstValue>,
    ) -> Result<ConstValue, Error> {
        self.eval_inner(expr, substitutions, 0)
    }

    fn eval_inner(
        &self,
        expr: &Expr,
        substitutions: &BTreeMap<String, ConstValue>,
        depth: usize,
    ) -> Result<ConstValue, Error> {
        if depth > 32 {
            return Err(Error::at(expr.span(), "const evaluation is too deep"));
        }
        match expr {
            Expr::Lit(literal) => eval_literal(&literal.lit),
            Expr::Group(group) => {
                self.eval_inner(&group.expr, substitutions, depth + 1)
            }
            Expr::Paren(paren) => {
                self.eval_inner(&paren.expr, substitutions, depth + 1)
            }
            Expr::Block(block) => block
                .block
                .stmts
                .last()
                .and_then(|statement| match statement {
                    syn::Stmt::Expr(expr, _) => Some(expr),
                    _ => None,
                })
                .ok_or_else(|| {
                    Error::at(expr.span(), "const block has no value")
                })
                .and_then(|tail| {
                    self.eval_inner(tail, substitutions, depth + 1)
                }),
            Expr::Path(path) => {
                let segments = path
                    .path
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect::<Vec<_>>();
                let name = segments.last().ok_or_else(|| {
                    Error::at(expr.span(), "const path has no name")
                })?;
                if let Some(value) = substitutions.get(name) {
                    return Ok(value.clone());
                }
                let evaluated = if segments.len() >= 2 {
                    let class = &segments[segments.len().saturating_sub(2)];
                    match self.associated.get(&(class.clone(), name.clone())) {
                        Some(value) => {
                            self.eval_inner(value, substitutions, depth + 1)?
                        }
                        None => {
                            return Err(Error::at(
                                expr.span(),
                                format!(
                                    "unknown associated const `{class}::{name}`"
                                ),
                            ));
                        }
                    }
                } else {
                    let value = self.values.get(name).ok_or_else(|| {
                        Error::at(
                            expr.span(),
                            format!("unknown const `{name}`"),
                        )
                    })?;
                    self.eval_inner(value, substitutions, depth + 1)?
                };
                self.coerce(evaluated, name, expr.span())
            }
            Expr::Unary(unary) => {
                let value =
                    self.eval_inner(&unary.expr, substitutions, depth + 1)?;
                match unary.op {
                    UnOp::Neg(_) => {
                        let (number, ty, explicit) =
                            value.int().ok_or_else(|| {
                                Error::at(
                                    expr.span(),
                                    "`-` needs an integer const",
                                )
                            })?;
                        checked(number.saturating_neg(), ty, explicit, expr)
                    }
                    UnOp::Not(_) => match value {
                        ConstValue::Bool(value) => Ok(ConstValue::Bool(!value)),
                        ConstValue::Int(number, ty, explicit) => {
                            checked(!number, ty, explicit, expr)
                        }
                        ConstValue::Str(_) => Err(Error::at(
                            expr.span(),
                            "`!` needs an integer or boolean const",
                        )),
                    },
                    _ => Err(Error::at(
                        expr.span(),
                        "unary const operator is not supported",
                    )),
                }
            }
            Expr::Binary(binary) => {
                self.eval_binary(binary, substitutions, depth)
            }
            Expr::Cast(cast) => {
                let value =
                    self.eval_inner(&cast.expr, substitutions, depth + 1)?;
                let ty = int_type(&cast.ty).ok_or_else(|| {
                    Error::at(
                        cast.ty.span(),
                        "const cast needs an integer type",
                    )
                })?;
                let number = value.int().ok_or_else(|| {
                    Error::at(expr.span(), "const cast needs an integer value")
                })?;
                checked(number.0, ty, true, expr)
            }
            _ => Err(Error::at(
                expr.span(),
                "expression is not a proven constant",
            )),
        }
    }

    fn eval_binary(
        &self,
        binary: &syn::ExprBinary,
        substitutions: &BTreeMap<String, ConstValue>,
        depth: usize,
    ) -> Result<ConstValue, Error> {
        if matches!(binary.op, BinOp::And(_) | BinOp::Or(_)) {
            let left =
                self.eval_inner(&binary.left, substitutions, depth + 1)?;
            let short_circuit = match binary.op {
                BinOp::And(_) => left.boolean() == Some(false),
                BinOp::Or(_) => left.boolean() == Some(true),
                _ => false,
            };
            if short_circuit {
                return Ok(ConstValue::Bool(!left.boolean().unwrap_or(false)));
            }
            let right =
                self.eval_inner(&binary.right, substitutions, depth + 1)?;
            return Ok(ConstValue::Bool(right.boolean().ok_or_else(|| {
                Error::at(binary.right.span(), "logical const needs booleans")
            })?));
        }
        let left = self.eval_inner(&binary.left, substitutions, depth + 1)?;
        let right = self.eval_inner(&binary.right, substitutions, depth + 1)?;
        if let (
            Some((a, ty, left_explicit)),
            Some((b, other, right_explicit)),
        ) = (left.int(), right.int())
        {
            let (a, b, ty) = if ty == other {
                (a, b, ty)
            } else if !left_explicit && ty == IntType::I32 {
                let a =
                    checked(a, other, true, binary.left.span())?.int_value();
                (a, b, other)
            } else if !right_explicit && other == IntType::I32 {
                (
                    a,
                    checked(b, ty, true, binary.right.span())?.int_value(),
                    ty,
                )
            } else {
                return Err(Error::at(
                    binary.span(),
                    "const operands have different integer types",
                ));
            };
            let explicit = left_explicit || right_explicit;
            return eval_int_binary(
                &binary.op,
                a,
                b,
                ty,
                explicit,
                binary.span(),
            );
        }
        match (&binary.op, left, right) {
            (BinOp::Eq(_), ConstValue::Str(left), ConstValue::Str(right)) => {
                Ok(ConstValue::Bool(left == right))
            }
            (BinOp::Ne(_), ConstValue::Str(left), ConstValue::Str(right)) => {
                Ok(ConstValue::Bool(left != right))
            }
            _ => Err(Error::at(
                binary.span(),
                "const comparison has unproven operands",
            )),
        }
    }
}

/// Evaluates each const declaration before it can be folded. A const whose
/// value is not proven stops the build at its own position.
pub(super) fn validate_file(file: &File, env: &ConstEnv) -> Result<(), Error> {
    for item in &file.items {
        if let Item::Const(item) = item {
            let value = env.eval(&item.expr, &BTreeMap::new())?;
            env.coerce_declared(value, int_type(&item.ty), item.expr.span())?;
        }
    }
    Ok(())
}

fn eval_literal(literal: &Lit) -> Result<ConstValue, Error> {
    match literal {
        Lit::Int(value) => {
            let text = value.base10_digits().replace('_', "");
            let (number, ty, explicit) = parse_int(&text, value.suffix())
                .ok_or_else(|| {
                    Error::at(value.span(), "integer const is out of range")
                })?;
            checked(number, ty, explicit, value)
        }
        Lit::Bool(value) => Ok(ConstValue::Bool(value.value)),
        Lit::Str(value) => Ok(ConstValue::Str(value.value())),
        _ => Err(Error::at(
            literal.span(),
            "literal is not a proven constant",
        )),
    }
}

fn parse_int(text: &str, suffix: &str) -> Option<(i128, IntType, bool)> {
    let ty = match suffix {
        "" => IntType::I32,
        "i8" => IntType::I8,
        "i16" => IntType::I16,
        "i32" => IntType::I32,
        "i64" => IntType::I64,
        "isize" => IntType::Isize,
        "u8" => IntType::U8,
        "u16" => IntType::U16,
        "u32" => IntType::U32,
        "u64" => IntType::U64,
        "usize" => IntType::Usize,
        "u128" => IntType::U128,
        _ => return None,
    };
    let number = if let Some(hex) = text.strip_prefix("0x") {
        i128::from_str_radix(hex, 16).ok()?
    } else if let Some(binary) = text.strip_prefix("0b") {
        i128::from_str_radix(binary, 2).ok()?
    } else if let Some(octal) = text.strip_prefix("0o") {
        i128::from_str_radix(octal, 8).ok()?
    } else {
        text.parse().ok()?
    };
    Some((number, ty, !suffix.is_empty()))
}

fn checked(
    value: i128,
    ty: IntType,
    explicit: bool,
    node: impl Spanned,
) -> Result<ConstValue, Error> {
    if value < ty.min() || value > ty.max() {
        return Err(Error::at(
            node.span(),
            format!("const `{value}` overflows {}", type_name(ty)),
        ));
    }
    Ok(ConstValue::Int(value, ty, explicit))
}

fn type_name(ty: IntType) -> &'static str {
    match ty {
        IntType::I8 => "i8",
        IntType::I16 => "i16",
        IntType::I32 => "i32",
        IntType::I64 => "i64",
        IntType::Isize => "isize",
        IntType::U8 => "u8",
        IntType::U16 => "u16",
        IntType::U32 => "u32",
        IntType::U64 => "u64",
        IntType::Usize => "usize",
        IntType::U128 => "u128",
    }
}

fn eval_int_binary(
    op: &BinOp,
    left: i128,
    right: i128,
    ty: IntType,
    explicit: bool,
    span: Span,
) -> Result<ConstValue, Error> {
    let value = match op {
        BinOp::Add(_) => left.checked_add(right),
        BinOp::Sub(_) => left.checked_sub(right),
        BinOp::Mul(_) => left.checked_mul(right),
        BinOp::Div(_) | BinOp::Rem(_) if right == 0 => {
            return Err(Error::at(span, "const division by zero"));
        }
        BinOp::Div(_) => left.checked_div(right),
        BinOp::Rem(_) => left.checked_rem(right),
        BinOp::BitAnd(_) => Some(left & right),
        BinOp::BitOr(_) => Some(left | right),
        BinOp::BitXor(_) => Some(left ^ right),
        BinOp::Shl(_) | BinOp::Shr(_) => {
            let shift = u32::try_from(right)
                .map_err(|_| Error::at(span, "const shift is out of range"))?;
            if shift >= 128 {
                return Err(Error::at(span, "const shift is out of range"));
            }
            let shift =
                u32::from(u8::try_from(shift).map_err(|_| {
                    Error::at(span, "const shift is out of range")
                })?);
            if matches!(op, BinOp::Shl(_)) {
                left.checked_shl(shift)
            } else {
                left.checked_shr(shift)
            }
        }
        BinOp::Eq(_) => return Ok(ConstValue::Bool(left == right)),
        BinOp::Ne(_) => return Ok(ConstValue::Bool(left != right)),
        BinOp::Lt(_) => return Ok(ConstValue::Bool(left < right)),
        BinOp::Le(_) => return Ok(ConstValue::Bool(left <= right)),
        BinOp::Gt(_) => return Ok(ConstValue::Bool(left > right)),
        BinOp::Ge(_) => return Ok(ConstValue::Bool(left >= right)),
        _ => {
            return Err(Error::at(span, "const operator is not supported"));
        }
    };
    let value =
        value.ok_or_else(|| Error::at(span, "const arithmetic overflow"))?;
    checked(value, ty, explicit, span)
}

pub(super) fn int_type(ty: &Type) -> Option<IntType> {
    let Type::Path(path) = ty else {
        return None;
    };
    let name = path.path.segments.last()?.ident.to_string();
    match name.as_str() {
        "i8" => Some(IntType::I8),
        "i16" => Some(IntType::I16),
        "i32" => Some(IntType::I32),
        "i64" => Some(IntType::I64),
        "isize" => Some(IntType::Isize),
        "u8" => Some(IntType::U8),
        "u16" => Some(IntType::U16),
        "u32" => Some(IntType::U32),
        "u64" => Some(IntType::U64),
        "usize" => Some(IntType::Usize),
        "u128" => Some(IntType::U128),
        _ => None,
    }
}

pub(super) fn class_name(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

/// Folds proven const paths and expressions into literals.
pub(super) fn fold_file(file: &mut File, env: &ConstEnv) -> Result<(), Error> {
    let mut folder = Folder { env, error: None };
    folder.visit_file_mut(file);
    match folder.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

struct Folder<'a> {
    env: &'a ConstEnv,
    error: Option<Error>,
}

impl Folder<'_> {
    fn fold(&mut self, expr: &Expr) -> Option<Expr> {
        let substitutions = BTreeMap::new();
        let value = match self.env.eval(expr, &substitutions) {
            Ok(value) => value,
            Err(_) => return None,
        };
        syn::parse_str::<Expr>(&value.literal()).ok()
    }
}

impl VisitMut for Folder<'_> {
    fn visit_expr_mut(&mut self, node: &mut Expr) {
        visit_mut::visit_expr_mut(self, node);
        if let Some(folded) = self.fold(node) {
            *node = folded;
        }
    }

    fn visit_type_mut(&mut self, node: &mut Type) {
        visit_mut::visit_type_mut(self, node);
        if let Type::Array(array) = node
            && let Some(value) = self.fold(&array.len)
        {
            array.len = value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn evaluate(source: &str, expression: &str) -> Result<ConstValue, Error> {
        let file = syn::parse_file(source).expect("parse");
        let env = ConstEnv::new(std::slice::from_ref(&file));
        let expr = syn::parse_str::<Expr>(expression).expect("expr");
        env.eval(&expr, &BTreeMap::new())
    }

    #[test]
    fn evaluates_checked_arithmetic() {
        let source = "const N: u8 = 200 + 55; const HALF: usize = N / 51;";
        assert_eq!(
            evaluate(source, "N").expect("value"),
            ConstValue::Int(255, IntType::U8, true)
        );
        assert_eq!(
            evaluate(source, "HALF").expect("value"),
            ConstValue::Int(5, IntType::Usize, true)
        );
    }

    #[test]
    fn rejects_u8_overflow_at_a_position() {
        let error = evaluate("const N: u8 = 255; const M: u8 = N + 1;", "M")
            .expect_err("overflow");
        assert!(error.message.contains("overflows u8"), "{error:?}");
        assert!(error.line > 0);
    }

    #[test]
    fn rejects_unsigned_underflow() {
        let error =
            evaluate("const N: u8 = 0 - 1;", "N").expect_err("underflow");
        assert!(error.message.contains("overflows u8"), "{error:?}");
    }
    #[test]
    fn validates_the_declared_width_of_an_initializer() {
        let file = syn::parse_file("const X: u8 = 255 + 1;").expect("parse");
        let env = ConstEnv::new(std::slice::from_ref(&file));
        let error = validate_file(&file, &env).expect_err("direct overflow");
        assert!(error.message.contains("overflows u8"), "{error:?}");
        assert_eq!(error.line, 1);
    }

    #[test]
    fn folds_a_const_into_the_source() {
        let mut file =
            syn::parse_file("const N: usize = 3; fn f() { let _ = N; }")
                .expect("parse");
        let env = ConstEnv::new(std::slice::from_ref(&file));
        fold_file(&mut file, &env).expect("fold");
        let output = quote::quote!(#file).to_string();
        assert!(output.contains("let _ = 3"), "{output}");
    }
}
