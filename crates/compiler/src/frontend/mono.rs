//! Monomorphization and trait dispatch for the typed frontend.
//!
//! Generic calls are specialized from their arguments. A method that belongs
//! to a user trait is dispatched from the proven receiver type by inlining the
//! one impl that matches it. Unproven receivers and missing impls are errors
//! with a source position.

use std::collections::{BTreeMap, HashMap};

use proc_macro2::Span;
use quote::ToTokens;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit_mut::{self, VisitMut};
use syn::{Expr, File, GenericArgument, Item, Pat, Stmt, Token, Type};

use super::consts::ConstEnv;
use super::types::{self, FnSig, MethodDef, Signatures, Traits, Ty};
use crate::Error;

/// What the typed frontend resolved and generated.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LoweringReport {
    pub specializations: Vec<String>,
    pub dispatches: Vec<String>,
}

/// Lowers generics and trait calls in every file.
pub(super) fn lower(
    files: &mut [File],
    env: &ConstEnv,
    traits: &Traits,
) -> Result<LoweringReport, Error> {
    let mut report = LoweringReport::default();
    let mut generic_names = std::collections::BTreeSet::new();
    for file in files.iter() {
        for item in &file.items {
            if let Item::Fn(function) = item
                && !function.sig.generics.params.is_empty()
                && !generic_names.insert(function.sig.ident.to_string())
            {
                return Err(Error::at(
                    function.sig.ident.span(),
                    format!(
                        "generic function `{}` is defined more than once; module-aware specialization is not supported",
                        function.sig.ident
                    ),
                ));
            }
        }
    }
    let originals = files
        .iter()
        .flat_map(|file| {
            file.items.iter().filter_map(|item| match item {
                Item::Fn(function) => {
                    Some((function.sig.ident.to_string(), function.clone()))
                }
                _ => None,
            })
        })
        .collect::<BTreeMap<_, _>>();
    for file in files.iter_mut() {
        specialize_file(file, env, &originals, &mut report)?;
    }
    let method_defs = types::methods(files);
    let signatures = Signatures::new(files);
    for file in files.iter_mut() {
        dispatch_file(file, &method_defs, &signatures, traits, &mut report)?;
    }
    Ok(report)
}

fn specialize_file(
    file: &mut File,
    env: &ConstEnv,
    originals: &BTreeMap<String, syn::ItemFn>,
    report: &mut LoweringReport,
) -> Result<(), Error> {
    for round in 0..32 {
        let signatures = Signatures::new(std::slice::from_ref(file));
        let mut generated = Vec::new();
        let mut error = None;
        for item in &mut file.items {
            let Item::Fn(function) = item else {
                continue;
            };
            if function
                .attrs
                .iter()
                .any(|attr| attr.path().is_ident("luau"))
            {
                continue;
            }
            let scope =
                types::locals(&function.sig, &function.block, &signatures);
            let mut specializer = Specializer {
                signatures: &signatures,
                env,
                scope: &scope,
                originals,
                generated: &mut generated,
                report,
                error: &mut error,
            };
            specializer.visit_block_mut(&mut function.block);
            if let Some(error) = error.take() {
                return Err(error);
            }
        }
        if generated.is_empty() {
            return Ok(());
        }
        if round == 31 {
            return Err(Error::at(
                Span::call_site(),
                "generic specialization did not converge",
            ));
        }
        for function in generated {
            file.items.push(Item::Fn(function));
        }
    }
    Ok(())
}

struct Specializer<'a> {
    signatures: &'a Signatures,
    env: &'a ConstEnv,
    scope: &'a HashMap<String, Ty>,
    originals: &'a BTreeMap<String, syn::ItemFn>,
    generated: &'a mut Vec<syn::ItemFn>,
    report: &'a mut LoweringReport,
    error: &'a mut Option<Error>,
}

impl Specializer<'_> {
    fn specialize(&mut self, call: &mut syn::ExprCall) {
        let callee = match call.func.as_ref() {
            Expr::Path(path) => path.clone(),
            _ => return,
        };
        let Some(name) = callee.path.get_ident().map(ToString::to_string)
        else {
            return;
        };
        let Some(sig) = self.signatures.get(&name) else {
            return;
        };
        if !sig.generic() {
            return;
        }
        if let Err(error) = self.specialize_call(&name, sig, call, &callee) {
            *self.error = Some(error);
        }
    }

    fn specialize_call(
        &mut self,
        name: &str,
        sig: &FnSig,
        call: &mut syn::ExprCall,
        callee: &syn::ExprPath,
    ) -> Result<(), Error> {
        let mut types = BTreeMap::new();
        let mut consts = BTreeMap::new();
        if let Some(segment) = callee.path.segments.last()
            && let syn::PathArguments::AngleBracketed(args) = &segment.arguments
        {
            let mut type_names = sig.types.iter();
            let mut const_names = sig.consts.iter();
            for argument in &args.args {
                match argument {
                    GenericArgument::Type(ty) => {
                        let Some(param) = type_names.next() else {
                            return Err(Error::at(
                                call.span(),
                                format!(
                                    "`{name}` has no type parameter for this argument"
                                ),
                            ));
                        };
                        types.insert(param.clone(), ty.clone());
                    }
                    GenericArgument::Const(expr) => {
                        let Some(param) = const_names.next() else {
                            return Err(Error::at(
                                call.span(),
                                format!(
                                    "`{name}` has no const parameter for this argument"
                                ),
                            ));
                        };
                        let value = self.env.eval(expr, &BTreeMap::new())?;
                        consts.insert(param.clone(), expr.clone());
                        let _ = value;
                    }
                    _ => {
                        return Err(Error::at(
                            call.span(),
                            "only type and const generic arguments are supported",
                        ));
                    }
                }
            }
        }
        for (param, argument) in sig.params.iter().zip(call.args.iter()) {
            let wanted = match param {
                Ty::Generic(name) => Some((name.clone(), false)),
                Ty::Ref(inner) => match inner.as_ref() {
                    Ty::Generic(name) => Some((name.clone(), true)),
                    _ => None,
                },
                _ => None,
            };
            let Some((param_name, borrowed)) = wanted else {
                continue;
            };
            if types.contains_key(&param_name) {
                continue;
            }
            let inferred = types::infer(argument, self.scope, self.signatures);
            let inferred = if borrowed {
                match inferred {
                    Ty::Ref(inner) => *inner,
                    other => other,
                }
            } else {
                inferred
            };
            let ty = type_from_ty(&inferred).ok_or_else(|| {
                Error::at(
                    argument.span(),
                    format!("cannot prove `{param_name}` for `{name}`"),
                )
            })?;
            types.insert(param_name, ty);
        }
        for param in &sig.types {
            if !types.contains_key(param) {
                return Err(Error::at(
                    call.span(),
                    format!(
                        "cannot prove type parameter `{param}` of `{name}`"
                    ),
                ));
            }
        }
        for param in &sig.consts {
            if !consts.contains_key(param) {
                return Err(Error::at(
                    call.span(),
                    format!(
                        "const parameter `{param}` of `{name}` must be explicit"
                    ),
                ));
            }
        }
        let original = self.originals.get(name).ok_or_else(|| {
            Error::at(call.span(), format!("no definition for `{name}`"))
        })?;
        let mut specialized = original.clone();
        let generated_name = format!(
            "__mono_{}_{}",
            name,
            self.report.specializations.len().saturating_add(1)
        );
        specialized.sig.ident =
            syn::Ident::new(&generated_name, original.sig.ident.span());
        specialized.sig.generics.params.clear();
        specialized.sig.generics.where_clause = None;
        let mut substitution = Substitute {
            types: &types,
            consts: &consts,
        };
        substitution.visit_signature_mut(&mut specialized.sig);
        substitution.visit_block_mut(&mut specialized.block);
        let mut rewritten = call.clone();
        if let Expr::Path(path) = rewritten.func.as_mut()
            && let Some(segment) = path.path.segments.last_mut()
        {
            segment.ident =
                syn::Ident::new(&generated_name, segment.ident.span());
            segment.arguments = syn::PathArguments::None;
        }
        *call = rewritten;
        self.generated.push(specialized);
        self.report.specializations.push(generated_name);
        Ok(())
    }
}

impl VisitMut for Specializer<'_> {
    fn visit_expr_mut(&mut self, node: &mut Expr) {
        visit_mut::visit_expr_mut(self, node);
        if let Expr::Call(call) = node {
            self.specialize(call);
        }
    }
    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        if mac.path.is_ident("vec")
            && let Ok(mut elements) =
                Punctuated::<Expr, Token![,]>::parse_terminated
                    .parse2(mac.tokens.clone())
        {
            for element in &mut elements {
                visit_mut::visit_expr_mut(self, element);
            }
            mac.tokens = elements.to_token_stream();
            return;
        }
        visit_mut::visit_macro_mut(self, mac);
    }
}

fn type_from_ty(ty: &Ty) -> Option<Type> {
    match ty {
        Ty::Named(name) => syn::parse_str::<Type>(name).ok(),
        Ty::Str => syn::parse_str::<Type>("&str").ok(),
        Ty::Bool => syn::parse_str::<Type>("bool").ok(),
        Ty::Int => syn::parse_str::<Type>("i32").ok(),
        Ty::Void => syn::parse_str::<Type>("()").ok(),
        Ty::Ref(inner) => {
            let inner = type_from_ty(inner)?;
            Some(syn::parse_quote!(&#inner))
        }
        Ty::Tuple(items) => {
            let items =
                items.iter().map(type_from_ty).collect::<Option<Vec<_>>>()?;
            Some(syn::parse_quote!((#(#items),*)))
        }
        _ => None,
    }
}

struct Substitute<'a> {
    types: &'a BTreeMap<String, Type>,
    consts: &'a BTreeMap<String, Expr>,
}

impl VisitMut for Substitute<'_> {
    fn visit_type_mut(&mut self, node: &mut Type) {
        visit_mut::visit_type_mut(self, node);
        if let Type::Path(path) = node
            && let Some(ident) = path.path.get_ident()
            && let Some(replacement) = self.types.get(&ident.to_string())
        {
            *node = replacement.clone();
        }
    }

    fn visit_expr_mut(&mut self, node: &mut Expr) {
        if let Expr::Path(path) = node
            && let Some(ident) = path.path.get_ident()
            && let Some(replacement) = self.consts.get(&ident.to_string())
        {
            *node = replacement.clone();
            return;
        }
        visit_mut::visit_expr_mut(self, node);
    }
}

fn dispatch_file(
    file: &mut File,
    method_defs: &BTreeMap<(String, String), MethodDef>,
    signatures: &Signatures,
    traits: &Traits,
    report: &mut LoweringReport,
) -> Result<(), Error> {
    for item in &mut file.items {
        let Item::Fn(function) = item else {
            continue;
        };
        let scope = types::locals(&function.sig, &function.block, signatures);
        let mut dispatcher = Dispatch {
            scope: &scope,
            signatures,
            method_defs,
            traits,
            report,
            error: None,
        };
        dispatcher.visit_block_mut(&mut function.block);
        if let Some(error) = dispatcher.error.take() {
            return Err(error);
        }
    }
    Ok(())
}

struct Dispatch<'a> {
    scope: &'a HashMap<String, Ty>,
    signatures: &'a Signatures,
    method_defs: &'a BTreeMap<(String, String), MethodDef>,
    traits: &'a Traits,
    report: &'a mut LoweringReport,
    error: Option<Error>,
}

impl Dispatch<'_> {
    fn dispatch(&mut self, call: &syn::ExprMethodCall) -> Option<Expr> {
        let name = call.method.to_string();
        if !self.traits.is_method(&name) {
            return None;
        }
        let receiver =
            types::infer(&call.receiver, self.scope, self.signatures);
        let Some(class) = receiver.base().map(ToOwned::to_owned) else {
            self.error.get_or_insert(Error::at(
                call.method.span(),
                format!("cannot prove the receiver type of `{name}`"),
            ));
            return None;
        };
        let Some(def) = self.method_defs.get(&(class.clone(), name.clone()))
        else {
            self.error.get_or_insert(Error::at(
                call.method.span(),
                format!("no impl of `{name}` for `{class}`"),
            ));
            return None;
        };
        if def.params.len() != call.args.len() {
            self.error.get_or_insert(Error::at(
                call.method.span(),
                format!(
                    "`{name}` takes {} arguments, got {}",
                    def.params.len(),
                    call.args.len()
                ),
            ));
            return None;
        }
        let mut finder = ReturnFinder { found: false };
        let mut body = def.body.clone();
        finder.visit_block_mut(&mut body);
        if finder.found {
            self.error.get_or_insert(Error::at(
                call.method.span(),
                "trait methods with `return` are not inlined by the typed frontend",
            ));
            return None;
        }
        let self_name = format!("__self_{}", self.report.dispatches.len());
        let mut body = def.body.clone();
        let mut replace = ReplaceSelf {
            from: "self".to_owned(),
            to: self_name.clone(),
        };
        replace.visit_block_mut(&mut body);
        let receiver = strip_reference(&call.receiver);
        let mut statements = Vec::with_capacity(
            body.stmts
                .len()
                .saturating_add(def.params.len().saturating_add(1)),
        );
        statements.push(local(&self_name, receiver, call.method.span()));
        for (param, argument) in def.params.iter().zip(&call.args) {
            statements.push(local(param, argument.clone(), call.method.span()));
        }
        statements.extend(body.stmts);
        self.report.dispatches.push(format!("{class}::{name}"));
        Some(Expr::Block(syn::ExprBlock {
            attrs: Vec::new(),
            label: None,
            block: syn::Block {
                brace_token: Default::default(),
                stmts: statements,
            },
        }))
    }
}

fn local(name: &str, expression: Expr, span: proc_macro2::Span) -> Stmt {
    Stmt::Local(syn::Local {
        attrs: Vec::new(),
        let_token: Default::default(),
        modifiers: Default::default(),
        pat: Pat::Ident(syn::PatIdent {
            attrs: Vec::new(),
            by_ref: None,
            mutability: None,
            ident: syn::Ident::new(name, span),
            subpat: None,
        }),
        init: Some(syn::LocalInit {
            eq_token: Default::default(),
            expr: Box::new(expression),
            diverge: None,
        }),
        semi_token: Default::default(),
    })
}

impl VisitMut for Dispatch<'_> {
    fn visit_expr_mut(&mut self, node: &mut Expr) {
        visit_mut::visit_expr_mut(self, node);
        let Expr::MethodCall(call) = node else {
            return;
        };
        if let Some(replacement) = self.dispatch(call) {
            *node = replacement;
        }
    }
    fn visit_macro_mut(&mut self, mac: &mut syn::Macro) {
        if mac.path.is_ident("vec")
            && let Ok(mut elements) =
                Punctuated::<Expr, Token![,]>::parse_terminated
                    .parse2(mac.tokens.clone())
        {
            for element in &mut elements {
                visit_mut::visit_expr_mut(self, element);
            }
            mac.tokens = elements.to_token_stream();
            return;
        }
        visit_mut::visit_macro_mut(self, mac);
    }
}

struct ReturnFinder {
    found: bool,
}

impl VisitMut for ReturnFinder {
    fn visit_expr_return_mut(&mut self, _node: &mut syn::ExprReturn) {
        self.found = true;
    }
}

struct ReplaceSelf {
    from: String,
    to: String,
}

impl VisitMut for ReplaceSelf {
    fn visit_expr_path_mut(&mut self, node: &mut syn::ExprPath) {
        if node
            .path
            .get_ident()
            .is_some_and(|ident| ident == self.from.as_str())
        {
            let span = node.path.span();
            node.path = syn::Path::from(syn::Ident::new(&self.to, span));
            return;
        }
        visit_mut::visit_expr_path_mut(self, node);
    }
}

fn strip_reference(expr: &Expr) -> Expr {
    match expr {
        Expr::Reference(reference) => (*reference.expr).clone(),
        Expr::Paren(paren) => strip_reference(&paren.expr),
        Expr::Group(group) => strip_reference(&group.expr),
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specializes_a_generic_call_from_its_argument() {
        let mut file = syn::parse_file(
            "fn twice<T: Clone>(value: T) -> (T, T) { (value.clone(), value) } fn main() { let a = twice(1i32); let _ = a; }",
        )
        .expect("parse");
        let env = ConstEnv::new(std::slice::from_ref(&file));
        let traits = Traits::new(std::slice::from_ref(&file));
        let report = lower(std::slice::from_mut(&mut file), &env, &traits)
            .expect("lower");
        assert_eq!(report.specializations.len(), 1, "{report:?}");
        let source = quote::quote!(#file).to_string();
        assert!(source.contains("__mono_twice_1"), "{source}");
    }

    #[test]
    fn dispatches_a_trait_method_by_receiver_type() {
        let mut file = syn::parse_file(
            "trait Label { fn label(&self) -> &'static str; } struct A; struct B; impl Label for A { fn label(&self) -> &'static str { \"a\" } } impl Label for B { fn label(&self) -> &'static str { \"b\" } } fn main() { let a = A; let b = B; let _ = a.label(); let _ = b.label(); }",
        )
        .expect("parse");
        let env = ConstEnv::new(std::slice::from_ref(&file));
        let traits = Traits::new(std::slice::from_ref(&file));
        let report = lower(std::slice::from_mut(&mut file), &env, &traits)
            .expect("lower");
        assert_eq!(report.dispatches.len(), 2, "{report:?}");
        let source = quote::quote!(#file).to_string();
        assert!(source.contains("\"a\""), "{source}");
        assert!(source.contains("\"b\""), "{source}");
    }
    #[test]
    fn binds_trait_method_arguments_in_the_inlined_body() {
        let mut file = syn::parse_file(
            "trait Add { fn add(&self, b: i32) -> i32; } struct A; impl Add for A { fn add(&self, b: i32) -> i32 { b + 1 } } fn main() { let a = A; let _ = a.add(2); }",
        )
        .expect("parse");
        let env = ConstEnv::new(std::slice::from_ref(&file));
        let traits = Traits::new(std::slice::from_ref(&file));
        lower(std::slice::from_mut(&mut file), &env, &traits).expect("lower");
        let source = quote::quote!(#file).to_string();
        assert!(source.contains("b = 2"), "{source}");
        assert!(source.contains("b + 1"), "{source}");
    }

    #[test]
    fn rejects_duplicate_generic_function_names() {
        let mut files = vec![
            syn::parse_file("fn wrap<T>(value: T) -> T { value }")
                .expect("first"),
            syn::parse_file("fn wrap<T>(value: T) -> T { value }")
                .expect("second"),
        ];
        let env = ConstEnv::new(&files);
        let traits = Traits::new(&files);
        let error = lower(&mut files, &env, &traits).expect_err("duplicate");
        assert!(error.message.contains("more than once"), "{error:?}");
    }
}
