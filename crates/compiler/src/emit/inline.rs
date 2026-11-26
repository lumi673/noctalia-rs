use super::*;

const MAX_INLINE_DEPTH: usize = 8;
const WRITE_BACK_MARKER: &str = "\u{1}wb";
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
        let temp = (mode == Inline::Value).then(|| self.temp());
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
        self.scopes.push(HashMap::new());
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
                    self.line(&format!("local {name} = {text}"));
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
                        renames.push((name.clone(), fresh.clone()));
                        fresh
                    } else {
                        name.clone()
                    };
                    self.line(&format!("local {local_name} = {text}"));
                    if scalar && ty.base == Base::Str {
                        string_params.push((
                            local_name.clone(),
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
                        write_backs.push((target, local_name.clone()));
                    }
                    self.push(&local_name, ty);
                }
            }
        }
        for (_, original, span) in &string_params {
            if let Some(method) =
                unsupported_string_mutator(&helper.function.block, original)
            {
                return Err(Error::at(
                    *span,
                    format!(
                        "cannot preserve `&mut` aliasing for the `{original}` argument of `{}`: String::{method} has no faithful rewrite",
                        helper.name
                    ),
                ));
            }
        }
        let mut block = helper.function.block.clone();
        for (from, to) in &renames {
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
        self.scopes.pop();
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
            Expr::ForLoop(item) => visit_block(&mut item.body, from, to),
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
                    if !matches!(method.as_str(), "push" | "push_str" | "clear")
                    {
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
        let one_line = content.contains(" end") || content.contains("end)");
        if (content.contains("function(") || content.contains("(function()"))
            && !one_line
        {
            blocks.push(true);
        } else if !one_line
            && (content == "do"
                || content.ends_with(" then")
                || content.ends_with(" do"))
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
        ends_in_return = !blocks.iter().any(|function| *function)
            && trimmed.starts_with("return");
        if ends_in_return
            && !trimmed.strip_prefix("return").is_some_and(|rest| {
                rest.starts_with(|c: char| {
                    c.is_ascii_alphanumeric() || c == '_'
                })
            })
        {
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
