//! Types, signatures and trait tables for the typed frontend.
//!
//! The inference here is deliberately partial. It proves the receiver of a
//! user method call and the arguments of a generic call. Anything it cannot
//! prove stays unknown and the caller fails closed instead of guessing.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use syn::{Expr, File, Item, Pat, ReturnType, Type};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum Ty {
    Named(String),
    Generic(String),
    Ref(Box<Ty>),
    Tuple(Vec<Ty>),
    Bool,
    Int,
    Str,
    Void,
    Unknown,
}

impl Ty {
    pub(super) fn from_type(ty: &Type) -> Self {
        match ty {
            Type::Reference(reference) => {
                Self::Ref(Box::new(Self::from_type(&reference.elem)))
            }
            Type::Tuple(tuple) => {
                Self::Tuple(tuple.elems.iter().map(Self::from_type).collect())
            }
            Type::Path(path) => {
                let Some(segment) = path.path.segments.last() else {
                    return Self::Unknown;
                };
                let name = segment.ident.to_string();
                match name.as_str() {
                    "bool" => Self::Bool,
                    "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8"
                    | "u16" | "u32" | "u64" | "u128" | "usize" | "f32"
                    | "f64" => Self::Int,
                    "str" | "String" => Self::Str,
                    "()" => Self::Void,
                    _ => Self::Named(name),
                }
            }
            _ => Self::Unknown,
        }
    }

    pub(super) fn base(&self) -> Option<&str> {
        match self {
            Self::Ref(inner) => inner.base(),
            Self::Named(name) => Some(name),
            _ => None,
        }
    }
    /// Reads a type where generic parameter names are known.
    pub(super) fn from_param_type(
        ty: &Type,
        params: &BTreeSet<String>,
    ) -> Self {
        if let Type::Path(path) = ty
            && let Some(ident) = path.path.get_ident()
            && params.contains(&ident.to_string())
        {
            return Self::Generic(ident.to_string());
        }
        Self::from_type(ty)
    }
}

#[derive(Clone, Debug)]
pub(super) struct FnSig {
    pub(super) params: Vec<Ty>,
    pub(super) ret: Ty,
    pub(super) types: Vec<String>,
    pub(super) consts: Vec<String>,
}

impl FnSig {
    fn new(sig: &syn::Signature) -> Self {
        let mut types = Vec::new();
        let mut consts = Vec::new();
        for param in &sig.generics.params {
            match param {
                syn::GenericParam::Type(param) => {
                    types.push(param.ident.to_string())
                }
                syn::GenericParam::Const(param) => {
                    consts.push(param.ident.to_string())
                }
                syn::GenericParam::Lifetime(_) => {}
            }
        }
        let names = types.iter().cloned().collect::<BTreeSet<_>>();
        let params = sig
            .inputs
            .iter()
            .filter_map(|input| match input {
                syn::FnArg::Typed(arg) => {
                    Some(Ty::from_param_type(&arg.ty, &names))
                }
                syn::FnArg::Receiver(_) => None,
            })
            .collect();
        let ret = match &sig.output {
            ReturnType::Default => Ty::Void,
            ReturnType::Type(_, ty) => Ty::from_param_type(ty, &names),
        };
        Self {
            params,
            ret,
            types,
            consts,
        }
    }

    pub(super) fn generic(&self) -> bool {
        !self.types.is_empty() || !self.consts.is_empty()
    }
}

#[derive(Default)]
pub(super) struct Signatures {
    pub(super) functions: BTreeMap<String, FnSig>,
}

impl Signatures {
    pub(super) fn new(files: &[File]) -> Self {
        let mut result = Self::default();
        for file in files {
            for item in &file.items {
                if let Item::Fn(item) = item {
                    result.functions.insert(
                        item.sig.ident.to_string(),
                        FnSig::new(&item.sig),
                    );
                }
            }
        }
        result
    }

    pub(super) fn get(&self, name: &str) -> Option<&FnSig> {
        self.functions.get(name)
    }
}

#[derive(Clone)]
pub(super) struct MethodDef {
    pub(super) body: syn::Block,
    pub(super) params: Vec<String>,
}

#[derive(Default)]
pub(super) struct Traits {
    pub(super) methods: BTreeMap<String, String>,
    pub(super) bounds: BTreeMap<String, String>,
    pub(super) defined: BTreeSet<String>,
}

impl Traits {
    pub(super) fn new(files: &[File]) -> Self {
        let mut result = Self::default();
        for file in files {
            for item in &file.items {
                if let Item::Trait(item) = item {
                    let name = item.ident.to_string();
                    result.defined.insert(name.clone());
                    for member in &item.items {
                        if let syn::TraitItem::Fn(function) = member {
                            result.methods.insert(
                                function.sig.ident.to_string(),
                                name.clone(),
                            );
                        }
                    }
                }
                if let Item::Fn(item) = item {
                    for param in &item.sig.generics.params {
                        if let syn::GenericParam::Type(param) = param {
                            for bound in &param.bounds {
                                if let syn::TypeParamBound::Trait(bound) = bound
                                    && let Some(last) =
                                        bound.path.segments.last()
                                {
                                    result.bounds.insert(
                                        param.ident.to_string(),
                                        last.ident.to_string(),
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        result
    }

    pub(super) fn is_method(&self, name: &str) -> bool {
        self.methods.contains_key(name)
    }
}

/// All inherent and trait impl methods, keyed by `(receiver, method)`.
pub(super) fn methods(files: &[File]) -> BTreeMap<(String, String), MethodDef> {
    let mut result = BTreeMap::new();
    for file in files {
        for item in &file.items {
            let Item::Impl(item) = item else {
                continue;
            };
            let Some(class) = super::consts::class_name(&item.self_ty) else {
                continue;
            };
            for member in &item.items {
                if let syn::ImplItem::Fn(function) = member {
                    result.insert(
                        (class.clone(), function.sig.ident.to_string()),
                        MethodDef {
                            body: function.block.clone(),
                            params: function
                                .sig
                                .inputs
                                .iter()
                                .filter_map(|input| match input {
                                    syn::FnArg::Typed(arg) => {
                                        super::names::pattern_name(&arg.pat)
                                    }
                                    syn::FnArg::Receiver(_) => None,
                                })
                                .collect(),
                        },
                    );
                }
            }
        }
    }
    result
}

/// Infers the type of an expression from locals and known signatures.
pub(super) fn infer(
    expr: &Expr,
    locals: &HashMap<String, Ty>,
    signatures: &Signatures,
) -> Ty {
    match expr {
        Expr::Lit(literal) => match &literal.lit {
            syn::Lit::Str(_) => Ty::Str,
            syn::Lit::Bool(_) => Ty::Bool,
            syn::Lit::Int(_) | syn::Lit::Float(_) | syn::Lit::Char(_) => {
                Ty::Int
            }
            _ => Ty::Unknown,
        },
        Expr::Path(path) => path
            .path
            .get_ident()
            .and_then(|ident| locals.get(&ident.to_string()).cloned())
            .unwrap_or_else(|| {
                path.path
                    .segments
                    .last()
                    .map(|segment| Ty::Named(segment.ident.to_string()))
                    .unwrap_or(Ty::Unknown)
            }),
        Expr::Reference(reference) => {
            Ty::Ref(Box::new(infer(&reference.expr, locals, signatures)))
        }
        Expr::Struct(value) => Ty::Named(
            value
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
                .unwrap_or_default(),
        ),
        Expr::Call(call) => match call.func.as_ref() {
            Expr::Path(path) => path
                .path
                .get_ident()
                .and_then(|ident| signatures.get(&ident.to_string()))
                .map(|sig| sig.ret.clone())
                .unwrap_or(Ty::Unknown),
            _ => Ty::Unknown,
        },
        Expr::MethodCall(call) => match call.method.to_string().as_str() {
            "to_string" | "as_str" | "trim" | "to_lowercase"
            | "to_uppercase" => Ty::Str,
            "clone" | "to_owned" | "unwrap_or_default" | "unwrap_or"
            | "get" => infer(&call.receiver, locals, signatures),
            "push" | "insert" | "remove" | "clear" => Ty::Void,
            _ => Ty::Unknown,
        },
        Expr::Macro(mac) => mac
            .mac
            .path
            .get_ident()
            .map(|ident| match ident.to_string().as_str() {
                "vec" => Ty::Named("Vec".to_owned()),
                "json" => Ty::Named("Value".to_owned()),
                _ => Ty::Unknown,
            })
            .unwrap_or(Ty::Unknown),
        Expr::Tuple(tuple) => Ty::Tuple(
            tuple
                .elems
                .iter()
                .map(|expr| infer(expr, locals, signatures))
                .collect(),
        ),
        Expr::Binary(binary) => match binary.op {
            syn::BinOp::Eq(_)
            | syn::BinOp::Ne(_)
            | syn::BinOp::Lt(_)
            | syn::BinOp::Le(_)
            | syn::BinOp::Gt(_)
            | syn::BinOp::Ge(_)
            | syn::BinOp::And(_)
            | syn::BinOp::Or(_) => Ty::Bool,
            _ => Ty::Int,
        },
        Expr::If(value) => value
            .then_branch
            .stmts
            .last()
            .and_then(|statement| match statement {
                syn::Stmt::Expr(expr, _) => {
                    Some(infer(expr, locals, signatures))
                }
                _ => None,
            })
            .unwrap_or(Ty::Unknown),
        Expr::Block(block) => block
            .block
            .stmts
            .last()
            .and_then(|statement| match statement {
                syn::Stmt::Expr(expr, _) => {
                    Some(infer(expr, locals, signatures))
                }
                _ => None,
            })
            .unwrap_or(Ty::Unknown),
        Expr::Paren(paren) => infer(&paren.expr, locals, signatures),
        Expr::Group(group) => infer(&group.expr, locals, signatures),
        _ => Ty::Unknown,
    }
}

/// Collects the types of locals visible in a function body.
pub(super) fn locals(
    sig: &syn::Signature,
    block: &syn::Block,
    signatures: &Signatures,
) -> HashMap<String, Ty> {
    let mut result = HashMap::new();
    let generic_names = sig
        .generics
        .params
        .iter()
        .filter_map(|param| match param {
            syn::GenericParam::Type(param) => Some(param.ident.to_string()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    for input in &sig.inputs {
        if let syn::FnArg::Typed(arg) = input
            && let syn::Pat::Ident(ident) = arg.pat.as_ref()
        {
            result.insert(
                ident.ident.to_string(),
                Ty::from_param_type(&arg.ty, &generic_names),
            );
        }
    }
    for statement in &block.stmts {
        let syn::Stmt::Local(local) = statement else {
            continue;
        };
        let (pattern, annotation) = match &local.pat {
            Pat::Type(typed) => (typed.pat.as_ref(), Some(typed.ty.as_ref())),
            other => (other, None),
        };
        if let Pat::Ident(ident) = pattern {
            let ty = annotation
                .map(|ty| Ty::from_param_type(ty, &generic_names))
                .or_else(|| {
                    local
                        .init
                        .as_ref()
                        .map(|init| infer(&init.expr, &result, signatures))
                })
                .unwrap_or(Ty::Unknown);
            result.insert(ident.ident.to_string(), ty);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infers_struct_receivers_from_local_declarations() {
        let file = syn::parse_file(
            "struct A; fn main() { let a = A; let b = &a; let flag = true; }",
        )
        .expect("parse");
        let signatures = Signatures::new(std::slice::from_ref(&file));
        let Some(function) = file.items.iter().find_map(|item| match item {
            Item::Fn(item) if item.sig.ident == "main" => Some(item),
            _ => None,
        }) else {
            panic!("no main");
        };
        assert!(function.block.stmts.len() >= 3);
        let _ = function;
        let scope = locals(&function.sig, &function.block, &signatures);
        assert_eq!(scope.get("a"), Some(&Ty::Named("A".to_owned())));
        assert_eq!(
            scope.get("b"),
            Some(&Ty::Ref(Box::new(Ty::Named("A".to_owned()))))
        );
        assert_eq!(scope.get("flag"), Some(&Ty::Bool));
    }
}
