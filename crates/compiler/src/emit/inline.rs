use super::*;

const MAX_INLINE_DEPTH: usize = 8;
const WRITE_BACK_MARKER: &str = "\u{1}wb";

type MutPredicate<'a> = &'a dyn Fn(&ExprCall, usize) -> bool;
impl<'a> Emitter<'a> {
    pub(super) fn helper(&self, call: &ExprCall) -> Option<Helper> {
        let Expr::Path(path) = &*call.func else {
            return None;
        };
        let segments = path_segments(&path.path)?;
        let resolved = self.program.resolve(&self.module, &segments)?;
        Some(Helper {
            module: resolved.module.clone(),
            function: resolved.function.clone(),
            name: segments.join("::"),
            class: None,
        })
    }

    pub(super) fn check_inline(
        &self,
        helper: &Helper,
        mode: Inline,
        returns: Returns,
        span: &proc_macro2::Span,
    ) -> Result<BodyCheck, Error> {
        if self.inline_stack.len() >= MAX_INLINE_DEPTH {
            return Err(Error::at(
                *span,
                format!(
                    "inlining `{}` exceeds the depth limit of {MAX_INLINE_DEPTH}",
                    helper.name
                ),
            ));
        }
        if self.inline_budget == 0 {
            return Err(Error::at(
                *span,
                format!(
                    "inlining `{}` exceeds the limit of {MAX_INLINE_CALLS} calls per body",
                    helper.name
                ),
            ));
        }
        let key = format!("{}::{}", helper.module, helper.name);
        if self
            .inline_stack
            .iter()
            .any(|entry| entry.split('\u{1}').next() == Some(key.as_str()))
        {
            return Err(Error::at(
                *span,
                format!("recursive helper `{}` cannot be inlined", helper.name),
            ));
        }
        match (mode, returns) {
            (Inline::Tail | Inline::Value, Returns::Value(_))
            | (Inline::Tail, Returns::Ops | Returns::Unit)
            | (Inline::Value, Returns::VecOps)
            | (Inline::Statement, Returns::Value(_) | Returns::VecOps) => {}
            (Inline::Statement, Returns::Unit)
            | (Inline::Ops, Returns::VecOps) => {}
            (Inline::Value, Returns::Ops) => {
                return Err(Error::at(
                    *span,
                    format!(
                        "helper `{}` returns `Result<Vec<Op>>` and can only be called in tail position",
                        helper.name
                    ),
                ));
            }
            (Inline::Tail, Returns::VecOps) => {
                return Err(Error::at(
                    *span,
                    format!(
                        "helper `{}` returns `Vec<Op>` and can only be used inside `Ok(...)`",
                        helper.name
                    ),
                ));
            }
            (_, Returns::Ops) => {
                return Err(Error::at(
                    *span,
                    format!(
                        "helper `{}` returns `Result<Vec<Op>>` and must be called in tail position",
                        helper.name
                    ),
                ));
            }
            (Inline::Ops, Returns::Unit | Returns::Value(_)) => {
                return Err(Error::at(
                    *span,
                    format!(
                        "helper `{}` does not build a `Vec<Op>`",
                        helper.name
                    ),
                ));
            }
            (Inline::Value, Returns::Unit) => {
                return Err(Error::at(
                    *span,
                    format!("helper `{}` returns no value", helper.name),
                ));
            }
        }
        check_body(&helper.function, mode)
    }

    pub(super) fn bindings(
        &mut self,
        helper: &Helper,
        call: &ExprCall,
    ) -> Result<Vec<Binding>, Error> {
        let params = typed_args(&helper.function);
        if params.len() != call.args.len() {
            return Err(Error::at(
                call.span(),
                format!(
                    "helper `{}` takes {} arguments but {} were passed",
                    helper.name,
                    params.len(),
                    call.args.len()
                ),
            ));
        }
        let mut bindings = Vec::new();
        for ((pattern, ty), argument) in params.iter().zip(call.args.iter()) {
            bindings.push(self.binding(pattern, ty, argument)?);
        }
        Ok(bindings)
    }

    pub(super) fn binding(
        &mut self,
        pattern: &Pat,
        ty: &Type,
        argument: &Expr,
    ) -> Result<Binding, Error> {
        let Pat::Ident(pattern) = pattern else {
            return Err(Error::at(
                ty.span(),
                "helper parameters must be plain identifiers",
            ));
        };
        let name = pattern.ident.to_string();
        if is_context(ty) {
            if !self.is_ctx_expr(strip_reference(argument)) {
                return Err(Error::at(
                    argument.span(),
                    format!(
                        "argument for `{name}` must be the context passed to the caller"
                    ),
                ));
            }
            return Ok(Binding::Context(name));
        }
        if is_ops_vec(ty) {
            let Expr::Path(path) = strip_reference(argument) else {
                return Err(Error::at(
                    argument.span(),
                    "expected a local `Vec<Op>` for this argument",
                ));
            };
            let Some(ident) = path.path.get_ident() else {
                return Err(Error::at(
                    argument.span(),
                    "expected a local `Vec<Op>` for this argument",
                ));
            };
            if self.lookup(&ident.to_string()) != Some(Ty::OPS) {
                return Err(Error::at(
                    argument.span(),
                    "expected a local `Vec<Op>` for this argument",
                ));
            }
            return Ok(Binding::Ops(name));
        }
        if let Type::Reference(reference) = ty
            && reference.mutability.is_some()
        {
            if matches!(
                &*reference.elem,
                Type::Reference(_)
                    | Type::ImplTrait(_)
                    | Type::TraitObject(_)
                    | Type::Ptr(_)
            ) {
                return Err(Error::at(
                    ty.span(),
                    "`&mut` helper parameters other than `&mut Vec<Op>` cannot be inlined",
                ));
            }
            if let Some(reason) = unsupported_lvalue(argument) {
                return Err(Error::at(
                    argument.span(),
                    format!(
                        "cannot preserve `&mut` aliasing for this argument: {reason}"
                    ),
                ));
            }
            let (text, value) = self.expr(argument)?;
            let target = mutable_lvalue(argument).then(|| text.clone());
            let operands = lvalue_operands(argument);
            let scalar = is_mut_scalar(&type_text(&reference.elem));
            return Ok(Binding::Mut {
                name,
                text,
                ty: value,
                target,
                scalar,
                operands,
                span: argument.span(),
            });
        }
        let (text, value) = self.expr(argument)?;
        let argument_null = value.null || text == "nil";
        let declared = type_hint(ty);
        let mut value = if declared.base == Base::Value {
            Ty {
                optional: declared.optional,
                ..Ty::VALUE
            }
        } else {
            value
        };
        value.null = argument_null && declared.base == Base::Value;
        if value.null {
            value.element = None;
        }
        let kind = static_kind(ty);
        if kind != StaticKind::Value
            && let Expr::Path(path) = strip_reference(argument)
            && let Some(name) = path.path.get_ident()
        {
            self.containers.insert(name.to_string(), kind);
        }
        Ok(Binding::Local {
            name,
            text,
            ty: value,
            container: kind,
        })
    }

    pub(super) fn inline_call(
        &mut self,
        call: &ExprCall,
        mode: Inline,
    ) -> Result<Inlined, Error> {
        let Some(helper) = self.helper(call) else {
            return Ok(None);
        };
        let bindings = self.bindings(&helper, call)?;
        self.inline_resolved(helper, mode, call.span(), bindings)
    }

    pub(super) fn inline_resolved(
        &mut self,
        helper: Helper,
        mode: Inline,
        span: proc_macro2::Span,
        bindings: Vec<Binding>,
    ) -> Result<Inlined, Error> {
        let returns = return_kind(&helper.function)?;
        let key = native_key(&helper);
        if let Some(known) = self.known_native.get(&key).cloned() {
            return self.native_call(
                &known,
                value_type(returns),
                mode,
                span,
                &bindings,
            );
        }
        if self.native_mode == NativeMode::Allow
            && let Some(cache_ms) = forced_native(&helper.function)?
        {
            let item = self.register_native(
                &helper,
                cache_ms,
                true,
                "marked `#[native]`",
                span,
            )?;
            return self.native_call(
                &item,
                value_type(returns),
                mode,
                span,
                &bindings,
            );
        }
        let check = self.check_inline(&helper, mode, returns, &span)?;
        let saved = self.clone();
        match self.inline(&helper, bindings.clone(), mode, returns, check) {
            Ok((text, _)) => {
                let ty =
                    self.last_value_ty.take().unwrap_or(value_type(returns));
                Ok(Some((text, ty)))
            }
            Err(error) => {
                *self = saved;
                if error.fatal {
                    return Err(error);
                }
                let demotable = self.native_mode == NativeMode::Allow
                    && mode == Inline::Value
                    && matches!(returns, Returns::Value(_));
                if !demotable {
                    return Err(error);
                }
                let item = self.register_native(
                    &helper,
                    0,
                    false,
                    &native_reason(&error),
                    span,
                )?;
                self.native_call(
                    &item,
                    value_type(returns),
                    mode,
                    span,
                    &bindings,
                )
            }
        }
    }

    fn ops_body(&mut self, block: &syn::Block) -> Result<(), Error> {
        let Some((last, rest)) = block.stmts.split_last() else {
            return Ok(());
        };
        for stmt in rest {
            self.stmt(stmt)?;
        }
        if let Stmt::Expr(Expr::Path(path), _) = last
            && let Some(ident) = path.path.get_ident()
            && self
                .lookup(&ident.to_string())
                .is_some_and(|ty| matches!(ty.base, Base::Ops | Base::List))
        {
            return Ok(());
        }
        self.stmt(last)
    }

    pub(super) fn inline(
        &mut self,
        helper: &Helper,
        bindings: Vec<Binding>,
        mode: Inline,
        returns: Returns,
        check: BodyCheck,
    ) -> Result<(String, Ty), Error> {
        let nested_with_writebacks = self
            .inline_stack
            .last()
            .is_some_and(|key| key.ends_with(WRITE_BACK_MARKER));
        let iife = check.needs_iife || nested_with_writebacks;
        let saved_class = self.self_class.clone();
        let temp = (mode == Inline::Value || (mode == Inline::Tail && iife))
            .then(|| self.temp());
        if let Some(name) = &temp {
            if iife {
                self.line(&format!("local {name}"));
                self.line(&format!("{name} = (function()"));
            } else {
                self.line(&format!("local {name}"));
                self.line("do");
            }
        } else if iife {
            self.line("do");
            self.indent = self.indent.saturating_add(1);
            self.line("(function()");
        } else {
            self.line("do");
        }
        self.indent = self.indent.saturating_add(1);
        self.push_scope();
        self.local_statics.push(HashMap::new());
        let saved_module =
            std::mem::replace(&mut self.module, helper.module.clone());
        let scope = self.module_constants(&helper.module);
        self.constant_scopes.push(scope);
        let saved_target = self.tail_target.clone();
        self.tail_target = if iife { None } else { temp.clone() };
        let saved_value_helper = self.value_helper;
        let assigned = assigned_params(&helper.function.block);
        let mut_names: std::collections::HashSet<String> = bindings
            .iter()
            .filter_map(|binding| match binding {
                Binding::Mut { name, .. } => Some(name.clone()),
                _ => None,
            })
            .collect();
        let mut_spans: Vec<(String, proc_macro2::Span)> = bindings
            .iter()
            .filter_map(|binding| match binding {
                Binding::Mut { name, span, .. } => Some((name.clone(), *span)),
                _ => None,
            })
            .collect();
        let dest_names: std::collections::HashSet<String> = bindings
            .iter()
            .filter_map(|binding| match binding {
                Binding::Mut { operands, .. } => Some(operands.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        let foreign = helper.module != self.home;
        let module = helper.module.clone();
        let attach = |error: Error| {
            if foreign {
                error.with_file(&module)
            } else {
                error
            }
        };
        let body_start = self.out.len();
        let mut write_backs = Vec::new();
        let mut string_params = Vec::new();
        let mut renames = Vec::new();
        for binding in bindings {
            match binding {
                Binding::Context(name) => self.push(&name, Ty::CONTEXT),
                Binding::Ops(name) => self.push(&name, Ty::OPS),
                Binding::Local {
                    name,
                    text,
                    ty,
                    container,
                } => {
                    let lua = self.bind_name(&name);
                    self.line(&format!("local {lua} = {text}"));
                    if container != StaticKind::Value {
                        self.containers.insert(name.clone(), container);
                    }
                    if let Some(class) = self.pending_class.take() {
                        self.classes.insert(name.clone(), class);
                    }
                    if let Some(shapes) = self
                        .tuple_expr_shapes
                        .get(&text)
                        .cloned()
                        .or_else(|| self.last_tuple_shape.take())
                    {
                        self.tuple_types.insert(name.clone(), shapes.clone());
                        self.tuple_expr_shapes.insert(name.clone(), shapes);
                    }
                    self.push(&name, ty);
                }
                Binding::Mut {
                    name,
                    text,
                    ty,
                    target,
                    scalar,
                    operands,
                    span,
                } => {
                    let local_name = if target.as_deref() == Some(name.as_str())
                    {
                        let fresh = self.temp();
                        renames.push((name.clone(), fresh.clone(), span));
                        fresh
                    } else {
                        name.clone()
                    };
                    let lua_name = self.bind_name(&local_name);
                    self.line(&format!("local {lua_name} = {text}"));
                    if scalar && ty.base == Base::Str {
                        string_params.push((
                            lua_name.clone(),
                            name.clone(),
                            span,
                        ));
                    }
                    if scalar || assigned.contains(&name) {
                        let target = target.ok_or_else(|| {
                            Error::at(
                                span,
                                format!(
                                    "cannot preserve `&mut` aliasing for the `{}` argument of `{}`",
                                    name, helper.name
                                ),
                            )
                        })?;
                        if operands.iter().any(|operand| {
                            operand != &name
                                && (assigned.contains(operand)
                                    || mut_names.contains(operand))
                        }) {
                            return Err(Error::at(
                                span,
                                format!(
                                    "cannot preserve `&mut` aliasing for the `{}` argument of `{}`",
                                    name, helper.name
                                ),
                            ));
                        }
                        let forwarded = call_args_mut(
                            &helper.function.block,
                            &name,
                            &|call, index| {
                                self.helper(call)
                                    .and_then(|target| {
                                        typed_args(&target.function)
                                            .get(index)
                                            .map(|(_, ty)| (*ty).clone())
                                    })
                                    .is_some_and(|ty| {
                                        matches!(
                                            ty,
                                            Type::Reference(reference)
                                                if reference.mutability.is_some()
                                        )
                                    })
                            },
                        );
                        if forwarded {
                            return Err(Error::at(
                                span,
                                format!(
                                    "cannot preserve `&mut` aliasing for the `{}` argument of `{}`: nested calls may mutate it",
                                    name, helper.name
                                ),
                            ));
                        }
                        write_backs.push((target, lua_name.clone()));
                    }
                    self.push(&local_name, ty);
                }
            }
        }
        for (name, _) in &mut_spans {
            if let Some(reborrow) = mut_reborrow(&helper.function.block, name) {
                return Err(attach(Error::at(
                    reborrow,
                    format!(
                        "cannot preserve `&mut` aliasing for the `{name}` argument of `{}`: mutable reborrows cannot be inlined",
                        helper.name
                    ),
                )));
            }
        }
        for (_, original, span) in &string_params {
            if let Some(method) =
                unsupported_string_mutator(&helper.function.block, original)
            {
                return Err(attach(Error::at(
                    *span,
                    format!(
                        "cannot preserve `&mut` aliasing for the `{original}` argument of `{}`: String::{method} has no faithful rewrite",
                        helper.name
                    ),
                )));
            }
        }
        let mut shadow_names = std::collections::HashSet::new();
        shadow_names.extend(mut_names.iter().cloned());
        shadow_names.extend(dest_names.iter().cloned());
        for name in &shadow_names {
            if let Some(shadow) = shadowed_local(&helper.function.block, name) {
                return Err(attach(Error::at(
                    shadow,
                    format!(
                        "cannot preserve `&mut` aliasing for the `{name}` argument of `{}`: the name is shadowed",
                        helper.name
                    ),
                )));
            }
        }
        let mut block = helper.function.block.clone();
        for (from, to, span) in &renames {
            if implicit_macro_capture(&helper.function.block, from).is_some() {
                return Err(Error::at(
                    *span,
                    format!(
                        "cannot preserve `&mut` aliasing for the `{from}` argument of `{}`: implicit format captures cannot be renamed",
                        helper.name
                    ),
                ));
            }
            rename_paths(&mut block, from, to);
        }
        self.value_helper = mode == Inline::Value;
        let key = format!("{}::{}", helper.module, helper.name);
        let stack_key = if write_backs.is_empty() {
            key
        } else {
            format!("{key}{WRITE_BACK_MARKER}")
        };
        self.inline_stack.push(stack_key);
        self.inline_budget = self.inline_budget.saturating_sub(1);
        let result = match mode {
            Inline::Value => self.block_inner(&block, true).map(|()| {
                (
                    temp.clone().unwrap_or_default(),
                    match returns {
                        Returns::Value(ty) => ty,
                        _ => Ty::VALUE,
                    },
                )
            }),
            Inline::Tail => self
                .block_inner(&block, true)
                .map(|()| (String::new(), Ty::VOID)),
            Inline::Statement => self
                .block_inner(&block, false)
                .map(|()| (String::new(), Ty::VOID)),
            Inline::Ops => {
                self.ops_body(&block).map(|()| (String::new(), Ty::VOID))
            }
        };
        if !write_backs.is_empty() {
            let body = self.out.split_off(body_start);
            let (body, ends_in_return) =
                copy_back_returns(&body, &write_backs, &string_params);
            self.out.push_str(&body);
            if !ends_in_return {
                for (target, name) in &write_backs {
                    self.line(&format!("{target} = {name}"));
                }
            }
        }
        self.inline_stack.pop();
        self.value_helper = saved_value_helper;
        self.self_class = saved_class;
        self.tail_target = saved_target;
        self.constant_scopes.pop();
        self.module = saved_module;
        self.pop_scope();
        self.local_statics.pop();
        self.indent = self.indent.saturating_sub(1);
        if iife {
            self.line("end)()");
            if temp.is_none() {
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
            }
        } else {
            self.line("end");
        }
        if mode == Inline::Tail
            && iife
            && let Some(value) = &temp
        {
            if let Some(target) = self.tail_target.clone() {
                self.line(&format!("{target} = {value}"));
            } else {
                self.line(&format!("return {value}"));
            }
        }
        let foreign = helper.module != self.home;
        result.map_err(|error| {
            if foreign {
                error.with_file(&helper.module)
            } else {
                error
            }
        })
    }

    pub(super) fn inline_block(
        &mut self,
        block: &syn::Block,
        tail: bool,
    ) -> Result<(), Error> {
        if tail {
            return self.block_inner(block, true);
        }
        self.line("do");
        self.indent = self.indent.saturating_add(1);
        self.block_inner(block, false)?;
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        Ok(())
    }

    pub(super) fn inline_arm(
        &mut self,
        body: &Expr,
        tail: bool,
    ) -> Result<(), Error> {
        if tail {
            return self.arm_body(body, true);
        }
        self.line("do");
        self.indent = self.indent.saturating_add(1);
        self.arm_body(body, false)?;
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        Ok(())
    }

    pub(super) fn arm_body(
        &mut self,
        body: &Expr,
        tail: bool,
    ) -> Result<(), Error> {
        match body {
            Expr::Block(block) => self.block_inner(&block.block, tail),
            other if tail => self.tail_expr(other),
            other => self.expr_stmt(other),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Inline {
    Tail,
    Statement,
    Value,
    Ops,
}

pub(crate) struct Helper {
    pub(crate) module: String,
    pub(crate) function: ItemFn,
    pub(crate) name: String,
    pub(crate) class: Option<String>,
}

#[derive(Clone)]
pub(crate) enum Binding {
    Context(String),
    Ops(String),
    Local {
        name: String,
        text: String,
        ty: Ty,
        container: StaticKind,
    },
    Mut {
        name: String,
        text: String,
        ty: Ty,
        target: Option<String>,
        scalar: bool,
        operands: Vec<String>,
        span: proc_macro2::Span,
    },
}

fn is_mut_scalar(name: &str) -> bool {
    matches!(
        name,
        "String"
            | "str"
            | "bool"
            | "char"
            | "Path"
            | "PathBuf"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
    )
}

fn unsupported_lvalue(expr: &Expr) -> Option<&'static str> {
    fn walk(expr: &Expr) -> Option<&'static str> {
        match strip_reference(expr) {
            Expr::Field(field) => walk(&field.base),
            Expr::Unary(unary) if matches!(unary.op, UnOp::Deref(_)) => {
                walk(&unary.expr)
            }
            Expr::Index(index) => {
                match strip_reference(&index.index) {
                    Expr::Range(_) => {
                        return Some("slice destinations are not assignable");
                    }
                    Expr::Path(_) => {}
                    _ => {
                        return Some(
                            "computed index operands cannot be snapshotted",
                        );
                    }
                }
                walk(&index.expr)
            }
            Expr::Group(group) => walk(&group.expr),
            Expr::Paren(paren) => walk(&paren.expr),
            _ => None,
        }
    }

    walk(expr)
}

fn mutable_lvalue(expr: &Expr) -> bool {
    match strip_reference(expr) {
        Expr::Path(_) => true,
        Expr::Field(field) => mutable_lvalue(&field.base),
        Expr::Index(index) => mutable_lvalue(&index.expr),
        Expr::Unary(unary) if matches!(unary.op, UnOp::Deref(_)) => {
            mutable_lvalue(&unary.expr)
        }
        Expr::Group(group) => mutable_lvalue(&group.expr),
        Expr::Paren(paren) => mutable_lvalue(&paren.expr),
        _ => false,
    }
}

fn rename_tokens(
    tokens: proc_macro2::TokenStream,
    from: &str,
    to: &str,
) -> proc_macro2::TokenStream {
    use proc_macro2::TokenTree;
    let items: Vec<TokenTree> = tokens.into_iter().collect();
    let mut out = proc_macro2::TokenStream::new();
    let mut after_separator = false;
    for (index, token) in items.iter().enumerate() {
        let is_label = matches!(
            items.get(index + 1),
            Some(TokenTree::Punct(punct))
                if punct.as_char() == '='
                    && punct.spacing() == proc_macro2::Spacing::Alone
        );
        let renamed = match token {
            TokenTree::Ident(ident)
                if ident == from && !after_separator && !is_label =>
            {
                TokenTree::Ident(proc_macro2::Ident::new(to, ident.span()))
            }
            TokenTree::Ident(ident) => TokenTree::Ident(ident.clone()),
            TokenTree::Group(group) => {
                let mut renamed = proc_macro2::Group::new(
                    group.delimiter(),
                    rename_tokens(group.stream(), from, to),
                );
                renamed.set_span(group.span());
                TokenTree::Group(renamed)
            }
            other => other.clone(),
        };
        after_separator = matches!(
            &renamed,
            TokenTree::Punct(punct)
                if punct.as_char() == '.' || punct.as_char() == ':'
        );
        out.extend(std::iter::once(renamed));
    }
    out
}

fn rename_paths(block: &mut syn::Block, from: &str, to: &str) {
    fn visit(expr: &mut Expr, from: &str, to: &str) {
        match expr {
            Expr::Path(path) => {
                if path.path.get_ident().is_some_and(|id| id == from) {
                    path.path = syn::parse_str(to).expect("fresh local name");
                }
            }
            Expr::Block(item) => visit_block(&mut item.block, from, to),
            Expr::If(item) => {
                visit(&mut item.cond, from, to);
                visit_block(&mut item.then_branch, from, to);
                if let Some((_, branch)) = &mut item.else_branch {
                    visit(branch, from, to);
                }
            }
            Expr::Match(item) => {
                visit(&mut item.expr, from, to);
                for arm in &mut item.arms {
                    visit(&mut arm.body, from, to);
                }
            }
            Expr::ForLoop(item) => {
                visit(&mut item.expr, from, to);
                visit_block(&mut item.body, from, to);
            }
            Expr::While(item) => {
                visit(&mut item.cond, from, to);
                visit_block(&mut item.body, from, to);
            }
            Expr::Loop(item) => visit_block(&mut item.body, from, to),
            Expr::Closure(item) => visit(&mut item.body, from, to),
            Expr::MethodCall(item) => {
                visit(&mut item.receiver, from, to);
                for argument in &mut item.args {
                    visit(argument, from, to);
                }
            }
            Expr::Call(item) => {
                visit(&mut item.func, from, to);
                for argument in &mut item.args {
                    visit(argument, from, to);
                }
            }
            Expr::Assign(item) => {
                visit(&mut item.left, from, to);
                visit(&mut item.right, from, to);
            }
            Expr::Unary(item) => visit(&mut item.expr, from, to),
            Expr::Binary(item) => {
                visit(&mut item.left, from, to);
                visit(&mut item.right, from, to);
            }
            Expr::Paren(item) => visit(&mut item.expr, from, to),
            Expr::Group(item) => visit(&mut item.expr, from, to),
            Expr::Reference(item) => visit(&mut item.expr, from, to),
            Expr::Try(item) => visit(&mut item.expr, from, to),
            Expr::Return(item) => {
                if let Some(value) = &mut item.expr {
                    visit(value, from, to);
                }
            }
            Expr::Field(item) => visit(&mut item.base, from, to),
            Expr::Index(item) => {
                visit(&mut item.expr, from, to);
                visit(&mut item.index, from, to);
            }
            Expr::Array(item) => {
                for element in &mut item.elems {
                    visit(element, from, to);
                }
            }
            Expr::Tuple(item) => {
                for element in &mut item.elems {
                    visit(element, from, to);
                }
            }
            Expr::Struct(item) => {
                for field in &mut item.fields {
                    visit(&mut field.expr, from, to);
                }
            }
            Expr::Cast(item) => visit(&mut item.expr, from, to),
            Expr::Range(item) => {
                if let Some(start) = &mut item.start {
                    visit(start, from, to);
                }
                if let Some(end) = &mut item.end {
                    visit(end, from, to);
                }
            }
            Expr::Macro(item) => {
                item.mac.tokens = rename_tokens(
                    std::mem::take(&mut item.mac.tokens),
                    from,
                    to,
                );
            }
            _ => {}
        }
    }

    fn visit_block(block: &mut syn::Block, from: &str, to: &str) {
        for stmt in &mut block.stmts {
            match stmt {
                Stmt::Expr(item, _) => visit(item, from, to),
                Stmt::Local(local) => {
                    if let Some(init) = &mut local.init {
                        visit(&mut init.expr, from, to);
                    }
                }
                _ => {}
            }
        }
    }

    visit_block(block, from, to);
}

fn lvalue_operands(expr: &Expr) -> Vec<String> {
    fn walk(expr: &Expr, out: &mut Vec<String>) {
        match strip_reference(expr) {
            Expr::Path(path) => {
                if let Some(name) = path.path.get_ident() {
                    out.push(name.to_string());
                }
            }
            Expr::Field(field) => walk(&field.base, out),
            Expr::Unary(unary) if matches!(unary.op, UnOp::Deref(_)) => {
                walk(&unary.expr, out);
            }
            Expr::Index(index) => {
                walk(&index.expr, out);
                walk(&index.index, out);
            }
            Expr::Group(group) => walk(&group.expr, out),
            Expr::Paren(paren) => walk(&paren.expr, out),
            _ => {}
        }
    }

    let mut out = Vec::new();
    walk(expr, &mut out);
    out
}

fn pattern_ident(pattern: &Pat) -> Option<&syn::Ident> {
    match pattern {
        Pat::Ident(item) => Some(&item.ident),
        Pat::Type(item) => pattern_ident(&item.pat),
        Pat::Paren(item) => pattern_ident(&item.pat),
        _ => None,
    }
}

fn shadowed_local(block: &syn::Block, name: &str) -> Option<proc_macro2::Span> {
    fn statements(block: &syn::Block, name: &str) -> Option<proc_macro2::Span> {
        for stmt in &block.stmts {
            if let Stmt::Local(local) = stmt
                && pattern_ident(&local.pat).is_some_and(|id| id == name)
            {
                return Some(local.pat.span());
            }
            if let Some(span) = expressions(stmt, name) {
                return Some(span);
            }
        }
        None
    }

    fn expressions(stmt: &Stmt, name: &str) -> Option<proc_macro2::Span> {
        match stmt {
            Stmt::Expr(expr, _) => exprs(expr, name),
            Stmt::Local(local) => {
                local.init.as_ref().and_then(|init| exprs(&init.expr, name))
            }
            _ => None,
        }
    }

    fn exprs(expr: &Expr, name: &str) -> Option<proc_macro2::Span> {
        match expr {
            Expr::Block(item) => statements(&item.block, name),
            Expr::If(item) => {
                statements(&item.then_branch, name).or_else(|| {
                    item.else_branch.as_ref().and_then(|(_, branch)| {
                        match &**branch {
                            Expr::Block(block) => {
                                statements(&block.block, name)
                            }
                            other => exprs(other, name),
                        }
                    })
                })
            }
            Expr::Match(item) => {
                item.arms.iter().find_map(|arm| match &*arm.body {
                    Expr::Block(block) => statements(&block.block, name),
                    other => exprs(other, name),
                })
            }
            Expr::ForLoop(item) => statements(&item.body, name),
            Expr::While(item) => statements(&item.body, name),
            Expr::Loop(item) => statements(&item.body, name),
            _ => None,
        }
    }

    statements(block, name)
}

fn mut_reborrow(block: &syn::Block, name: &str) -> Option<proc_macro2::Span> {
    fn statements(block: &syn::Block, name: &str) -> Option<proc_macro2::Span> {
        for stmt in &block.stmts {
            if let Stmt::Local(local) = stmt
                && let Some(init) = &local.init
                && reborrow(&init.expr, name)
            {
                return Some(init.expr.span());
            }
            if let Some(span) = match stmt {
                Stmt::Expr(expr, _) => exprs(expr, name),
                Stmt::Local(local) => {
                    local.init.as_ref().and_then(|init| exprs(&init.expr, name))
                }
                _ => None,
            } {
                return Some(span);
            }
        }
        None
    }

    fn reborrow(expr: &Expr, name: &str) -> bool {
        let Expr::Reference(reference) = expr else {
            return false;
        };
        if reference.mutability.is_none() {
            return false;
        }
        match strip_reference(&reference.expr) {
            Expr::Path(path) => {
                path.path.get_ident().is_some_and(|id| id == name)
            }
            Expr::Unary(unary) if matches!(unary.op, UnOp::Deref(_)) => {
                let Expr::Path(path) = strip_reference(&unary.expr) else {
                    return false;
                };
                path.path.get_ident().is_some_and(|id| id == name)
            }
            _ => false,
        }
    }

    fn exprs(expr: &Expr, name: &str) -> Option<proc_macro2::Span> {
        match expr {
            Expr::Block(item) => statements(&item.block, name),
            Expr::If(item) => {
                statements(&item.then_branch, name).or_else(|| {
                    item.else_branch.as_ref().and_then(|(_, branch)| {
                        match &**branch {
                            Expr::Block(block) => {
                                statements(&block.block, name)
                            }
                            other => exprs(other, name),
                        }
                    })
                })
            }
            Expr::Match(item) => {
                item.arms.iter().find_map(|arm| match &*arm.body {
                    Expr::Block(block) => statements(&block.block, name),
                    other => exprs(other, name),
                })
            }
            Expr::ForLoop(item) => statements(&item.body, name),
            Expr::While(item) => statements(&item.body, name),
            Expr::Loop(item) => statements(&item.body, name),
            _ => None,
        }
    }

    statements(block, name)
}

fn implicit_macro_capture(
    block: &syn::Block,
    name: &str,
) -> Option<proc_macro2::Span> {
    use proc_macro2::TokenTree;

    fn tokens(stream: proc_macro2::TokenStream, name: &str) -> bool {
        let items: Vec<TokenTree> = stream.into_iter().collect();
        let explicit = items.iter().enumerate().any(|(index, token)| {
            matches!(token, TokenTree::Ident(ident) if ident == name)
                && matches!(
                    items.get(index + 1),
                    Some(TokenTree::Punct(punct)) if punct.as_char() == '='
                )
        });
        if explicit {
            return false;
        }
        items.iter().any(|token| match token {
            TokenTree::Literal(literal) => {
                literal.to_string().contains(&format!("{{{name}}}"))
            }
            TokenTree::Group(group) => tokens(group.stream(), name),
            _ => false,
        })
    }

    fn walk(expr: &Expr, name: &str) -> bool {
        match expr {
            Expr::Macro(item) => tokens(item.mac.tokens.clone(), name),
            Expr::Block(item) => body(&item.block, name),
            Expr::If(item) => {
                walk(&item.cond, name)
                    || body(&item.then_branch, name)
                    || item
                        .else_branch
                        .as_ref()
                        .is_some_and(|(_, branch)| walk(branch, name))
            }
            Expr::Match(item) => {
                walk(&item.expr, name)
                    || item.arms.iter().any(|arm| walk(&arm.body, name))
            }
            Expr::ForLoop(item) => {
                walk(&item.expr, name) || body(&item.body, name)
            }
            Expr::While(item) => {
                walk(&item.cond, name) || body(&item.body, name)
            }
            Expr::Loop(item) => body(&item.body, name),
            Expr::Call(item) => {
                walk(&item.func, name)
                    || item.args.iter().any(|arg| walk(arg, name))
            }
            Expr::MethodCall(item) => {
                walk(&item.receiver, name)
                    || item.args.iter().any(|arg| walk(arg, name))
            }
            Expr::Assign(item) => {
                walk(&item.left, name) || walk(&item.right, name)
            }
            Expr::Unary(item) => walk(&item.expr, name),
            Expr::Binary(item) => {
                walk(&item.left, name) || walk(&item.right, name)
            }
            Expr::Paren(item) => walk(&item.expr, name),
            Expr::Group(item) => walk(&item.expr, name),
            Expr::Reference(item) => walk(&item.expr, name),
            Expr::Try(item) => walk(&item.expr, name),
            Expr::Return(item) => {
                item.expr.as_ref().is_some_and(|value| walk(value, name))
            }
            Expr::Field(item) => walk(&item.base, name),
            Expr::Index(item) => {
                walk(&item.expr, name) || walk(&item.index, name)
            }
            Expr::Array(item) => item.elems.iter().any(|item| walk(item, name)),
            Expr::Tuple(item) => item.elems.iter().any(|item| walk(item, name)),
            Expr::Struct(item) => {
                item.fields.iter().any(|field| walk(&field.expr, name))
            }
            _ => false,
        }
    }

    fn body(block: &syn::Block, name: &str) -> bool {
        block.stmts.iter().any(|stmt| match stmt {
            Stmt::Expr(expr, _) => walk(expr, name),
            Stmt::Local(local) => local
                .init
                .as_ref()
                .is_some_and(|init| walk(&init.expr, name)),
            _ => false,
        })
    }

    body(block, name).then_some(proc_macro2::Span::call_site())
}

fn call_args_mut(block: &syn::Block, name: &str, is_mut: MutPredicate) -> bool {
    fn argument(expr: &Expr, name: &str) -> bool {
        match strip_reference(expr) {
            Expr::Path(path) => {
                path.path.get_ident().is_some_and(|id| id == name)
            }
            Expr::Unary(unary) if matches!(unary.op, UnOp::Deref(_)) => {
                argument(&unary.expr, name)
            }
            Expr::Paren(paren) => argument(&paren.expr, name),
            Expr::Group(group) => argument(&group.expr, name),
            _ => false,
        }
    }

    fn walk(expr: &Expr, name: &str, is_mut: MutPredicate) -> bool {
        match expr {
            Expr::Call(item) => {
                item.args.iter().enumerate().any(|(index, arg)| {
                    is_mut(item, index) && argument(arg, name)
                }) || walk(&item.func, name, is_mut)
                    || item.args.iter().any(|arg| walk(arg, name, is_mut))
            }
            Expr::MethodCall(item) => {
                item.args.iter().enumerate().any(|(index, arg)| {
                    is_mut_param(index) && argument(arg, name)
                }) || walk(&item.receiver, name, is_mut)
                    || item.args.iter().any(|arg| walk(arg, name, is_mut))
            }
            Expr::Block(item) => body(&item.block, name, is_mut),
            Expr::If(item) => {
                walk(&item.cond, name, is_mut)
                    || body(&item.then_branch, name, is_mut)
                    || item
                        .else_branch
                        .as_ref()
                        .is_some_and(|(_, branch)| walk(branch, name, is_mut))
            }
            Expr::Match(item) => {
                walk(&item.expr, name, is_mut)
                    || item.arms.iter().any(|arm| walk(&arm.body, name, is_mut))
            }
            Expr::ForLoop(item) => body(&item.body, name, is_mut),
            Expr::While(item) => {
                walk(&item.cond, name, is_mut) || body(&item.body, name, is_mut)
            }
            Expr::Loop(item) => body(&item.body, name, is_mut),
            Expr::Closure(item) => walk(&item.body, name, is_mut),
            Expr::Assign(item) => {
                walk(&item.left, name, is_mut)
                    || walk(&item.right, name, is_mut)
            }
            Expr::Unary(item) => walk(&item.expr, name, is_mut),
            Expr::Binary(item) => {
                walk(&item.left, name, is_mut)
                    || walk(&item.right, name, is_mut)
            }
            Expr::Paren(item) => walk(&item.expr, name, is_mut),
            Expr::Group(item) => walk(&item.expr, name, is_mut),
            Expr::Reference(item) => walk(&item.expr, name, is_mut),
            Expr::Try(item) => walk(&item.expr, name, is_mut),
            Expr::Return(item) => item
                .expr
                .as_ref()
                .is_some_and(|value| walk(value, name, is_mut)),
            Expr::Field(item) => walk(&item.base, name, is_mut),
            Expr::Index(item) => {
                walk(&item.expr, name, is_mut)
                    || walk(&item.index, name, is_mut)
            }
            Expr::Array(item) => {
                item.elems.iter().any(|item| walk(item, name, is_mut))
            }
            Expr::Tuple(item) => {
                item.elems.iter().any(|item| walk(item, name, is_mut))
            }
            Expr::Struct(item) => item
                .fields
                .iter()
                .any(|field| walk(&field.expr, name, is_mut)),
            _ => false,
        }
    }

    fn is_mut_param(index: usize) -> bool {
        index == 0
    }

    fn body(block: &syn::Block, name: &str, is_mut: MutPredicate) -> bool {
        block.stmts.iter().any(|stmt| match stmt {
            Stmt::Expr(expr, _) => walk(expr, name, is_mut),
            Stmt::Local(local) => local
                .init
                .as_ref()
                .is_some_and(|init| walk(&init.expr, name, is_mut)),
            _ => false,
        })
    }

    body(block, name, is_mut)
}

fn unsupported_string_mutator(
    block: &syn::Block,
    name: &str,
) -> Option<String> {
    fn walk(expr: &Expr, name: &str, out: &mut Option<String>) {
        if out.is_some() {
            return;
        }
        match expr {
            Expr::MethodCall(call) => {
                if let Expr::Path(path) = strip_reference(&call.receiver)
                    && path.path.get_ident().is_some_and(|id| id == name)
                {
                    let method = call.method.to_string();
                    if matches!(
                        method.as_str(),
                        "insert"
                            | "insert_str"
                            | "remove"
                            | "truncate"
                            | "drain"
                            | "replace_range"
                    ) {
                        *out = Some(method);
                        return;
                    }
                }
                walk(&call.receiver, name, out);
                for argument in &call.args {
                    walk(argument, name, out);
                }
            }
            Expr::Block(item) => body(&item.block, name, out),
            Expr::If(item) => {
                walk(&item.cond, name, out);
                body(&item.then_branch, name, out);
                if let Some((_, branch)) = &item.else_branch {
                    walk(branch, name, out);
                }
            }
            Expr::Match(item) => {
                walk(&item.expr, name, out);
                for arm in &item.arms {
                    walk(&arm.body, name, out);
                }
            }
            Expr::ForLoop(item) => body(&item.body, name, out),
            Expr::While(item) => {
                walk(&item.cond, name, out);
                body(&item.body, name, out);
            }
            Expr::Loop(item) => body(&item.body, name, out),
            Expr::Closure(item) => walk(&item.body, name, out),
            Expr::Call(item) => {
                walk(&item.func, name, out);
                for argument in &item.args {
                    walk(argument, name, out);
                }
            }
            Expr::Assign(item) => {
                walk(&item.left, name, out);
                walk(&item.right, name, out);
            }
            Expr::Unary(item) => walk(&item.expr, name, out),
            Expr::Binary(item) => {
                walk(&item.left, name, out);
                walk(&item.right, name, out);
            }
            Expr::Paren(item) => walk(&item.expr, name, out),
            Expr::Group(item) => walk(&item.expr, name, out),
            Expr::Reference(item) => walk(&item.expr, name, out),
            Expr::Try(item) => walk(&item.expr, name, out),
            Expr::Return(item) => {
                if let Some(value) = &item.expr {
                    walk(value, name, out);
                }
            }
            Expr::Field(item) => walk(&item.base, name, out),
            Expr::Index(item) => {
                walk(&item.expr, name, out);
                walk(&item.index, name, out);
            }
            _ => {}
        }
    }

    fn body(block: &syn::Block, name: &str, out: &mut Option<String>) {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Expr(expr, _) => walk(expr, name, out),
                Stmt::Local(local) => {
                    if let Some(init) = &local.init {
                        walk(&init.expr, name, out);
                    }
                }
                _ => {}
            }
        }
    }

    let mut out = None;
    body(block, name, &mut out);
    out
}

fn assigned_params(block: &syn::Block) -> std::collections::HashSet<String> {
    fn collect(expr: &Expr, out: &mut std::collections::HashSet<String>) {
        match expr {
            Expr::Assign(item) => {
                target(&item.left, out);
                collect(&item.right, out);
            }
            Expr::Closure(item) => collect(&item.body, out),
            Expr::Block(item) => body(&item.block, out),
            Expr::If(item) => {
                collect(&item.cond, out);
                body(&item.then_branch, out);
                if let Some((_, branch)) = &item.else_branch {
                    collect(branch, out);
                }
            }
            Expr::Match(item) => {
                collect(&item.expr, out);
                for arm in &item.arms {
                    collect(&arm.body, out);
                }
            }
            Expr::ForLoop(item) => body(&item.body, out),
            Expr::While(item) => body(&item.body, out),
            Expr::Loop(item) => body(&item.body, out),
            Expr::MethodCall(item) => {
                collect(&item.receiver, out);
                for argument in &item.args {
                    collect(argument, out);
                }
            }
            Expr::Call(item) => {
                collect(&item.func, out);
                for argument in &item.args {
                    collect(argument, out);
                }
            }
            Expr::Unary(item) => collect(&item.expr, out),
            Expr::Binary(item) => {
                collect(&item.left, out);
                collect(&item.right, out);
            }
            Expr::Paren(item) => collect(&item.expr, out),
            Expr::Group(item) => collect(&item.expr, out),
            Expr::Reference(item) => collect(&item.expr, out),
            Expr::Try(item) => collect(&item.expr, out),
            Expr::Return(item) => {
                if let Some(value) = &item.expr {
                    collect(value, out);
                }
            }
            Expr::Field(item) => collect(&item.base, out),
            Expr::Index(item) => {
                collect(&item.expr, out);
                collect(&item.index, out);
            }
            Expr::Array(item) => {
                for element in &item.elems {
                    collect(element, out);
                }
            }
            Expr::Tuple(item) => {
                for element in &item.elems {
                    collect(element, out);
                }
            }
            Expr::Struct(item) => {
                for field in &item.fields {
                    collect(&field.expr, out);
                }
            }
            _ => {}
        }
    }

    fn target(expr: &Expr, out: &mut std::collections::HashSet<String>) {
        match strip_reference(expr) {
            Expr::Unary(unary) if matches!(unary.op, UnOp::Deref(_)) => {
                target(&unary.expr, out);
            }
            Expr::Path(path) => {
                if let Some(name) = path.path.get_ident() {
                    out.insert(name.to_string());
                }
            }
            Expr::Tuple(tuple) => {
                for element in &tuple.elems {
                    target(element, out);
                }
            }
            Expr::Paren(paren) => target(&paren.expr, out),
            Expr::Group(group) => target(&group.expr, out),
            Expr::Array(array) => {
                for element in &array.elems {
                    target(element, out);
                }
            }
            _ => {}
        }
    }

    fn body(block: &syn::Block, out: &mut std::collections::HashSet<String>) {
        for stmt in &block.stmts {
            match stmt {
                Stmt::Expr(expr, _) => collect(expr, out),
                Stmt::Local(local) => {
                    if let Some(init) = &local.init {
                        collect(&init.expr, out);
                    }
                }
                _ => {}
            }
        }
    }

    let mut out = std::collections::HashSet::new();
    body(block, &mut out);
    out
}

fn strip_strings(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut quoted = false;
    let mut escaped = false;
    for character in text.chars() {
        if quoted {
            out.push(' ');
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
        } else if character == '"' {
            quoted = true;
            out.push(' ');
        } else {
            out.push(character);
        }
    }
    out
}

fn copy_back_returns(
    body: &str,
    write_backs: &[(String, String)],
    string_params: &[(String, String, proc_macro2::Span)],
) -> (String, bool) {
    let mut out = String::with_capacity(body.len());
    let mut blocks: Vec<bool> = Vec::new();
    let mut ends_in_return = false;
    for line in body.split_inclusive('\n') {
        let trimmed = line.trim_start_matches(' ');
        let indent = &line[..line.len() - trimmed.len()];
        let newline = trimmed.ends_with('\n');
        let content = trimmed.strip_suffix('\n').unwrap_or(trimmed);
        let scan = strip_strings(content);
        let one_line = scan.contains(" end") || scan.contains("end)");
        if (scan.contains("function(") || scan.contains("(function()"))
            && !one_line
        {
            blocks.push(true);
        } else if !one_line
            && !scan.trim_start().starts_with("elseif")
            && (scan == "do"
                || scan.ends_with(" then")
                || scan.ends_with(" do"))
        {
            blocks.push(false);
        }
        let mut line = content.to_owned();
        for (name, _, _) in string_params {
            if let Some(value) = line
                .strip_prefix(&format!("table.insert({name}, "))
                .and_then(|rest| rest.strip_suffix(')'))
            {
                line = format!("{name} = {name} .. ({value})");
            } else if line == format!("table.clear({name})") {
                line = format!("{name} = \"\"");
            }
        }
        let outside_function = !blocks.iter().any(|function| *function);
        let early_return = outside_function && scan.contains("then return end");
        ends_in_return = outside_function
            && scan.trim_start().starts_with("return")
            && !scan
                .trim_start()
                .strip_prefix("return")
                .is_some_and(|rest| {
                    rest.starts_with(|c: char| {
                        c.is_ascii_alphanumeric() || c == '_'
                    })
                });
        if ends_in_return || early_return {
            for (target, name) in write_backs {
                out.push_str(indent);
                out.push_str(target);
                out.push_str(" = ");
                out.push_str(name);
                out.push('\n');
            }
        }
        out.push_str(indent);
        out.push_str(&line);
        if newline {
            out.push('\n');
        }
        if content.starts_with("end") {
            blocks.pop();
        }
    }
    (out, ends_in_return)
}

#[derive(Default)]
pub(crate) struct Scan {
    pub(crate) return_span: Option<proc_macro2::Span>,
    pub(crate) try_span: Option<proc_macro2::Span>,
    pub(crate) closure_span: Option<proc_macro2::Span>,
}

impl Scan {
    pub(super) fn block(&mut self, block: &syn::Block) {
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
    }

    pub(super) fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Local(local) => {
                if let Some(init) = &local.init {
                    self.expr(&init.expr);
                    if let Some((_, diverge)) = &init.diverge {
                        self.expr(diverge);
                    }
                }
            }
            Stmt::Expr(expr, _) => self.expr(expr),
            _ => {}
        }
    }

    pub(super) fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Return(item) => {
                if self.return_span.is_none() {
                    self.return_span = Some(item.span());
                }
                if let Some(value) = &item.expr {
                    self.expr(value);
                }
            }
            Expr::Try(item) => {
                if self.try_span.is_none() {
                    self.try_span = Some(item.span());
                }
                self.expr(&item.expr);
            }
            Expr::Closure(item) => {
                if self.closure_span.is_none() {
                    self.closure_span = Some(item.span());
                }
                self.expr(&item.body);
            }
            Expr::Block(item) => self.block(&item.block),
            Expr::If(item) => {
                self.expr(&item.cond);
                self.block(&item.then_branch);
                if let Some((_, branch)) = &item.else_branch {
                    self.expr(branch);
                }
            }
            Expr::Match(item) => {
                self.expr(&item.expr);
                for arm in &item.arms {
                    if let Pat::Guard(guard) = &arm.pat {
                        self.expr(&guard.guard);
                    }
                    self.expr(&arm.body);
                }
            }
            Expr::ForLoop(item) => {
                self.expr(&item.expr);
                self.block(&item.body);
            }
            Expr::While(item) => {
                self.expr(&item.cond);
                self.block(&item.body);
            }
            Expr::Loop(item) => self.block(&item.body),
            Expr::Let(item) => self.expr(&item.expr),
            Expr::Binary(item) => {
                self.expr(&item.left);
                self.expr(&item.right);
            }
            Expr::Unary(item) => self.expr(&item.expr),
            Expr::Paren(item) => self.expr(&item.expr),
            Expr::Group(item) => self.expr(&item.expr),
            Expr::Reference(item) => self.expr(&item.expr),
            Expr::Assign(item) => {
                self.expr(&item.left);
                self.expr(&item.right);
            }
            Expr::Cast(item) => self.expr(&item.expr),
            Expr::Field(item) => self.expr(&item.base),
            Expr::Index(item) => {
                self.expr(&item.expr);
                self.expr(&item.index);
            }
            Expr::MethodCall(item) => {
                self.expr(&item.receiver);
                for argument in &item.args {
                    self.expr(argument);
                }
            }
            Expr::Call(item) => {
                self.expr(&item.func);
                for argument in &item.args {
                    self.expr(argument);
                }
            }
            Expr::Array(item) => {
                for element in &item.elems {
                    self.expr(element);
                }
            }
            Expr::Tuple(item) => {
                for element in &item.elems {
                    self.expr(element);
                }
            }
            Expr::Struct(item) => {
                for field in &item.fields {
                    self.expr(&field.expr);
                }
            }
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BodyCheck {
    pub(crate) needs_iife: bool,
}

fn check_body(function: &ItemFn, mode: Inline) -> Result<BodyCheck, Error> {
    let mut scan = Scan::default();
    scan.block(&function.block);
    let tail_return = matches!(
        function.block.stmts.last(),
        Some(Stmt::Expr(Expr::Return(_), _))
    );
    let needs_iife = (scan.return_span.is_some()
        && !(tail_return && !matches!(mode, Inline::Statement | Inline::Ops)))
        || scan.try_span.is_some();
    if mode == Inline::Value
        && !matches!(function.block.stmts.last(), Some(Stmt::Expr(_, _)))
    {
        return Err(Error::at(
            function.sig.ident.span(),
            "a value helper must end in an expression",
        ));
    }
    Ok(BodyCheck { needs_iife })
}
