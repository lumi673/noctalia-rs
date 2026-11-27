use super::*;

const MAX_INLINE_DEPTH: usize = 8;
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
        if self.inline_stack.contains(&key) {
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
        let width = super::methods::shape_from_type(ty);
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
            let base = type_text(ty);
            if !matches!(
                base.as_str(),
                "String"
                    | "str"
                    | "Vec"
                    | "HashMap"
                    | "BTreeMap"
                    | "Value"
                    | "Op"
                    | "Context"
                    | "Path"
                    | "PathBuf"
            ) {
                let (text, value) = self.expr(argument)?;
                return Ok(Binding::Local {
                    width,
                    name,
                    text,
                    ty: value,
                    container: StaticKind::Value,
                });
            }
            return Err(Error::at(
                ty.span(),
                "`&mut` helper parameters other than `&mut Vec<Op>` cannot be inlined",
            ));
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
            width,
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
        let iife = check.needs_iife;
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
        self.push_scope();
        self.local_statics.push(HashMap::new());
        super::methods::push_param_width_scope();
        let saved_module =
            std::mem::replace(&mut self.module, helper.module.clone());
        let scope = self.module_constants(&helper.module);
        self.constant_scopes.push(scope);
        let saved_target = self.tail_target.clone();
        self.tail_target = if iife { None } else { temp.clone() };
        for binding in bindings {
            match binding {
                Binding::Context(name) => self.push(&name, Ty::CONTEXT),
                Binding::Ops(name) => self.push(&name, Ty::OPS),
                Binding::Local {
                    width,
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
                    if let Some(width) = width {
                        super::methods::add_param_width(&name, width);
                    }
                }
            }
        }
        let saved_value_helper = self.value_helper;
        self.value_helper = mode == Inline::Value;
        let key = format!("{}::{}", helper.module, helper.name);
        self.inline_stack.push(key);
        self.inline_budget = self.inline_budget.saturating_sub(1);
        let result = match mode {
            Inline::Value => {
                self.block_inner(&helper.function.block, true).map(|()| {
                    (
                        temp.clone().unwrap_or_default(),
                        match returns {
                            Returns::Value(ty) => ty,
                            _ => Ty::VALUE,
                        },
                    )
                })
            }
            Inline::Tail => self
                .block_inner(&helper.function.block, true)
                .map(|()| (String::new(), Ty::VOID)),
            Inline::Statement => self
                .block_inner(&helper.function.block, false)
                .map(|()| (String::new(), Ty::VOID)),
            Inline::Ops => self
                .ops_body(&helper.function.block)
                .map(|()| (String::new(), Ty::VOID)),
        };
        self.inline_stack.pop();
        super::methods::pop_param_width_scope();
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
        width: Option<(usize, bool)>,
        name: String,
        text: String,
        ty: Ty,
        container: StaticKind,
    },
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
