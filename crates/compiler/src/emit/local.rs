use super::*;

/// Builds the mangled Luau name for one strict helper.
fn helper_name(key: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in key.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("__noctalia_helper_{}_{hash:08x}", sanitize(key))
}

fn has_result_return(helper: &Helper) -> bool {
    matches!(
        &helper.function.sig.output,
        syn::ReturnType::Type(_, ty) if is_result(ty)
    )
}
fn helper_key(helper: &Helper) -> String {
    format!("{}::{}", helper.module, helper.name)
}

fn is_mut_ops_vec(ty: &Type) -> bool {
    matches!(ty, Type::Reference(reference) if reference.mutability.is_some() && is_ops_vec(&reference.elem))
}

/// Helpers that can cross the local-function boundary without changing
/// semantics. Mutable aliases other than `&mut Vec<Op>` still use the inline
/// path, which preserves their writebacks.
fn emittable(helper: &Helper) -> bool {
    if helper.class.is_some() {
        return false;
    }
    typed_args(&helper.function).iter().all(|(pattern, ty)| {
        matches!(pattern, Pat::Ident(_)) && !is_mut_other(ty)
    })
}

fn is_mut_other(ty: &Type) -> bool {
    matches!(ty, Type::Reference(reference) if reference.mutability.is_some() && !is_ops_vec(&reference.elem))
}

fn note_call(
    program: &Program,
    module: &str,
    call: &ExprCall,
    out: &mut BTreeMap<String, Helper>,
) {
    let Expr::Path(path) = &*call.func else {
        return;
    };
    let Some(segments) = path_segments(&path.path) else {
        return;
    };
    let Some(resolved) = program.resolve(module, &segments) else {
        return;
    };
    let key = format!("{}::{}", resolved.module, segments.join("::"));
    if out.contains_key(&key) {
        return;
    }
    out.insert(
        key,
        Helper {
            module: resolved.module.clone(),
            function: resolved.function.clone(),
            name: segments.join("::"),
            class: None,
        },
    );
}

fn walk_expr(
    program: &Program,
    module: &str,
    expr: &Expr,
    out: &mut BTreeMap<String, Helper>,
) {
    match expr {
        Expr::Array(item) => {
            for element in &item.elems {
                walk_expr(program, module, element, out);
            }
        }
        Expr::Assign(item) => {
            walk_expr(program, module, &item.left, out);
            walk_expr(program, module, &item.right, out);
        }
        Expr::Binary(item) => {
            walk_expr(program, module, &item.left, out);
            walk_expr(program, module, &item.right, out);
        }
        Expr::Block(item) => walk_block(program, module, &item.block, out),
        Expr::Call(item) => {
            walk_expr(program, module, &item.func, out);
            for argument in &item.args {
                walk_expr(program, module, argument, out);
            }
            note_call(program, module, item, out);
        }
        Expr::Cast(item) => walk_expr(program, module, &item.expr, out),
        Expr::Closure(item) => walk_expr(program, module, &item.body, out),
        Expr::Field(item) => walk_expr(program, module, &item.base, out),
        Expr::ForLoop(item) => {
            walk_expr(program, module, &item.expr, out);
            walk_block(program, module, &item.body, out);
        }
        Expr::Group(item) => walk_expr(program, module, &item.expr, out),
        Expr::If(item) => {
            walk_expr(program, module, &item.cond, out);
            walk_block(program, module, &item.then_branch, out);
            if let Some((_, branch)) = &item.else_branch {
                walk_expr(program, module, branch, out);
            }
        }
        Expr::Index(item) => {
            walk_expr(program, module, &item.expr, out);
            walk_expr(program, module, &item.index, out);
        }
        Expr::Let(item) => walk_expr(program, module, &item.expr, out),
        Expr::Loop(item) => walk_block(program, module, &item.body, out),
        Expr::Match(item) => {
            walk_expr(program, module, &item.expr, out);
            for arm in &item.arms {
                walk_expr(program, module, &arm.body, out);
            }
        }
        Expr::MethodCall(item) => {
            walk_expr(program, module, &item.receiver, out);
            for argument in &item.args {
                walk_expr(program, module, argument, out);
            }
        }
        Expr::Paren(item) => walk_expr(program, module, &item.expr, out),
        Expr::Range(item) => {
            if let Some(start) = &item.start {
                walk_expr(program, module, start, out);
            }
            if let Some(end) = &item.end {
                walk_expr(program, module, end, out);
            }
        }
        Expr::Reference(item) => walk_expr(program, module, &item.expr, out),
        Expr::Repeat(item) => {
            walk_expr(program, module, &item.expr, out);
            walk_expr(program, module, &item.len, out);
        }
        Expr::Return(item) => {
            if let Some(value) = &item.expr {
                walk_expr(program, module, value, out);
            }
        }
        Expr::Struct(item) => {
            for field in &item.fields {
                walk_expr(program, module, &field.expr, out);
            }
            if let Some(rest) = &item.rest {
                walk_expr(program, module, rest, out);
            }
        }
        Expr::Try(item) => walk_expr(program, module, &item.expr, out),
        Expr::Tuple(item) => {
            for element in &item.elems {
                walk_expr(program, module, element, out);
            }
        }
        Expr::Unary(item) => walk_expr(program, module, &item.expr, out),
        Expr::While(item) => {
            walk_expr(program, module, &item.cond, out);
            walk_block(program, module, &item.body, out);
        }
        other => {
            let _ = other;
        }
    }
}

fn walk_block(
    program: &Program,
    module: &str,
    block: &syn::Block,
    out: &mut BTreeMap<String, Helper>,
) {
    for statement in &block.stmts {
        match statement {
            Stmt::Local(local) => {
                if let Some(init) = &local.init {
                    walk_expr(program, module, &init.expr, out);
                    if let Some((_, diverge)) = &init.diverge {
                        walk_expr(program, module, diverge, out);
                    }
                }
            }
            Stmt::Expr(expr, _) => walk_expr(program, module, expr, out),
            Stmt::Item(syn::Item::Static(item)) => {
                walk_expr(program, module, &item.expr, out);
            }
            Stmt::Item(syn::Item::Const(item)) => {
                walk_expr(program, module, &item.expr, out);
            }
            other => {
                let _ = other;
            }
        }
    }
}

fn collect(
    block: &syn::Block,
    emitter: &Emitter<'_>,
) -> BTreeMap<String, Helper> {
    let mut helpers = BTreeMap::new();
    walk_block(emitter.program, &emitter.module, block, &mut helpers);
    let mut index = 0;
    loop {
        let keys: Vec<String> = helpers.keys().cloned().collect();
        let Some(key) = keys.get(index) else {
            break;
        };
        let Some(helper) = helpers.get(key).cloned() else {
            break;
        };
        walk_block(
            emitter.program,
            &helper.module,
            &helper.function.block,
            &mut helpers,
        );
        index += 1;
    }
    helpers
}

fn context_name(helper: &Helper, fallback: &str) -> String {
    typed_args(&helper.function)
        .iter()
        .find_map(|(pattern, ty)| {
            if is_context(ty)
                && let Pat::Ident(pattern) = pattern
            {
                Some(pattern.ident.to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| fallback.to_owned())
}

impl<'a> Emitter<'a> {
    /// Emits one local function per strict helper before the handler body.
    pub(crate) fn emit_local_helpers(
        &mut self,
        block: &syn::Block,
    ) -> Result<(), Error> {
        if !self.strict() {
            return Ok(());
        }
        let helpers = collect(block, self)
            .into_iter()
            .filter(|(_, helper)| emittable(helper))
            .collect::<BTreeMap<_, _>>();
        if helpers.is_empty() {
            return Ok(());
        }
        let names = helpers
            .keys()
            .map(|key| (key.clone(), helper_name(key)))
            .collect::<BTreeMap<_, _>>();
        self.local_helpers = names.clone();
        self.emitted = true;
        let owned_strings = self.strings.clone();
        let strings =
            owned_strings.iter().map(String::as_str).collect::<Vec<_>>();
        let mut defs = String::new();
        for name in names.values() {
            let indent = " ".repeat(self.indent.saturating_mul(2));
            defs.push_str(&format!("{indent}local {name}\n"));
        }
        for (key, helper) in helpers {
            let returns = return_kind(&helper.function)?;
            let ctx = context_name(&helper, &self.ctx);
            let ident = syn::Ident::new(&ctx, proc_macro2::Span::call_site());
            let mut emitter = Emitter::new(
                &ident,
                None,
                &strings,
                &[],
                self.program,
                &helper.module,
                self.native_mode,
                self.known_native,
                self.strict,
                self.has_action,
                self.has_host_callback,
            );
            emitter.local_helpers = names.clone();
            emitter.helper_result =
                matches!(returns, Returns::Ops) || has_result_return(&helper);
            emitter.helper_pair =
                has_result_return(&helper) && !matches!(returns, Returns::Ops);
            emitter.value_helper = matches!(returns, Returns::Value(_));
            let name = names.get(&key).ok_or_else(|| {
                Error::at(helper.function.span(), "missing helper name")
            })?;
            emitter.emit_helper_definition(&helper, name)?;
            let body = emitter.finish();
            self.merge_helper_body(&body);
            defs.push_str(&body.source);
        }
        self.local_defs = defs;
        Ok(())
    }

    fn emit_helper_definition(
        &mut self,
        helper: &Helper,
        name: &str,
    ) -> Result<(), Error> {
        let returns = return_kind(&helper.function)?;
        let params = typed_args(&helper.function);
        let args = (0..params.len())
            .map(|index| format!("__arg{index}"))
            .collect::<Vec<_>>();
        self.line(&format!("{name} = function({})", args.join(", ")));
        self.indent = self.indent.saturating_add(1);
        self.push_scope();
        self.local_statics.push(HashMap::new());
        super::methods::push_param_width_scope();
        let saved_module =
            std::mem::replace(&mut self.module, helper.module.clone());
        self.constant_scopes
            .push(self.module_constants(&helper.module));
        let helper_ctx = context_name(helper, &self.ctx);
        let saved_ctx = std::mem::replace(&mut self.ctx, helper_ctx);
        let saved_target = self.tail_target.clone();
        let saved_value_helper = self.value_helper;
        let saved_result = self.helper_result;
        let saved_pair = self.helper_pair;
        self.tail_target = None;
        self.value_helper = matches!(returns, Returns::Value(_));
        self.helper_result =
            matches!(returns, Returns::Ops) || has_result_return(helper);
        self.helper_pair =
            has_result_return(helper) && !matches!(returns, Returns::Ops);
        for ((pattern, ty), arg) in params.iter().zip(args.iter()) {
            let Pat::Ident(pattern) = pattern else {
                return Err(Error::at(
                    ty.span(),
                    "helper parameters must be plain identifiers",
                ));
            };
            let param = pattern.ident.to_string();
            let lua = self.bind_name(&param);
            self.line(&format!("local {lua} = {arg}"));
            if is_context(ty) {
                self.ctx = lua.clone();
                self.push(&param, Ty::CONTEXT);
            } else if is_mut_ops_vec(ty) || is_ops_vec(ty) {
                self.push(&param, Ty::OPS);
            } else {
                self.push(&param, type_hint(ty));
            }
            let kind = static_kind(ty);
            if kind != StaticKind::Value {
                self.containers.insert(param.clone(), kind);
            }
            if let Some(width) = shape_from_type(ty) {
                super::methods::add_param_width(&param, width);
            }
        }
        let result = match returns {
            Returns::Unit => self.statement_block(&helper.function.block),
            Returns::VecOps | Returns::Ops => {
                self.ops_body(&helper.function.block)
            }
            Returns::Value(_) => self.block_inner(&helper.function.block, true),
        };
        super::methods::pop_param_width_scope();
        self.helper_pair = saved_pair;
        self.helper_result = saved_result;
        self.value_helper = saved_value_helper;
        self.tail_target = saved_target;
        self.ctx = saved_ctx;
        self.constant_scopes.pop();
        self.module = saved_module;
        self.local_statics.pop();
        self.pop_scope();
        self.indent = self.indent.saturating_sub(1);
        result?;
        self.line("end");
        Ok(())
    }

    fn merge_helper_body(&mut self, body: &Body) {
        self.events.extend(body.events.iter().cloned());
        self.negative_events
            .extend(body.negative_events.iter().cloned());
        self.uses_event |= body.uses_event;
        self.uses_state |= body.uses_state;
        self.uses_entry |= body.uses_entry;
        self.uses_payload |= body.uses_payload;
        self.payload_escapes |= body.payload_escapes;
        self.uses_irem |= body.uses_irem;
        self.uses_unwrap |= body.uses_unwrap;
        self.uses_host_call |= body.uses_host_call;
        self.uses_query |= body.uses_query;
        self.uses_join |= body.uses_join;
        self.uses_bool_or |= body.uses_bool_or;
        self.uses_replace |= body.uses_replace;
        self.uses_source |= body.uses_source;
        self.uses_native |= body.uses_native;
        self.uses_native_context |= body.uses_native_context;
        self.uses_config_table |= body.uses_config_table;
        self.uses_translations_table |= body.uses_translations_table;
        self.uses_unpack |= body.uses_unpack;
        self.uses_pcall |= body.uses_pcall;
        self.uses_action_dispatch |= body.uses_action_dispatch;
        self.actions.extend(body.actions.iter().cloned());
        self.notify_actions
            .extend(body.notify_actions.iter().cloned());
        for (name, init) in &body.statics {
            self.statics.insert(
                name.clone(),
                StaticDef {
                    name: name.clone(),
                    kind: StaticKind::Value,
                    init: init.clone(),
                },
            );
        }
        for native in &body.native {
            self.native_found
                .entry(native.name.clone())
                .or_insert_with(|| native.clone());
        }
        self.emitted |= !body.is_empty;
    }

    pub(super) fn local_call(
        &mut self,
        helper: &Helper,
        mode: Inline,
        span: proc_macro2::Span,
        bindings: Vec<Binding>,
    ) -> Result<Inlined, Error> {
        let key = helper_key(helper);
        let Some(name) = self.local_helpers.get(&key).cloned() else {
            return Ok(None);
        };
        let returns = return_kind(&helper.function)?;
        let mut args = Vec::new();
        for binding in bindings {
            match binding {
                Binding::Context(_) | Binding::Ops(_) => {
                    args.push("nil".to_owned())
                }
                Binding::Local { text, .. } => {
                    let arg = self.temp();
                    self.line(&format!("local {arg} = {text}"));
                    args.push(arg);
                }
                Binding::Mut { span, .. } => {
                    return Err(Error::at(
                        span,
                        format!(
                            "helper `{}` has a mutable parameter that strict local functions cannot preserve",
                            helper.name
                        ),
                    ));
                }
            }
        }
        let call = format!("{name}({})", args.join(", "));
        match mode {
            Inline::Value => match returns {
                Returns::Unit => Err(Error::at(
                    span,
                    format!("helper `{}` returns no value", helper.name),
                )),
                Returns::Value(mut ty) => {
                    if has_result_return(helper) {
                        ty.result = true;
                    }
                    Ok(Some((call, ty)))
                }
                Returns::VecOps => Ok(Some((call, Ty::OPS))),
                Returns::Ops => Ok(Some((call, Ty::result(Base::Ops)))),
            },
            Inline::Tail => {
                match returns {
                    Returns::Ops => {
                        let value = self.temp();
                        let error = self.temp();
                        self.line(&format!("local {value}, {error} = {call}"));
                        self.line(&format!(
                            "if {error} ~= nil then error(tostring({error})) end"
                        ));
                        self.line("return");
                    }
                    Returns::Value(ty) if ty.result => {
                        let value = self.temp();
                        let error = self.temp();
                        self.line(&format!("local {value}, {error} = {call}"));
                        self.line(&format!(
                            "if {error} ~= nil then error(tostring({error})) end"
                        ));
                        self.line(&format!("return {value}"));
                    }
                    Returns::Value(_) => self.line(&format!("return {call}")),
                    Returns::VecOps | Returns::Unit => {
                        self.line(&call);
                        self.line("return");
                    }
                }
                Ok(Some((String::new(), Ty::VOID)))
            }
            Inline::Statement | Inline::Ops => {
                if matches!(returns, Returns::Ops) {
                    let value = self.temp();
                    let error = self.temp();
                    self.line(&format!("local {value}, {error} = {call}"));
                    self.line(&format!(
                        "if {error} ~= nil then error(tostring({error})) end"
                    ));
                } else {
                    self.line(&call);
                }
                Ok(Some((String::new(), Ty::VOID)))
            }
        }
    }
    pub(super) fn local_helper_name(&self, helper: &Helper) -> Option<String> {
        self.local_helpers.get(&helper_key(helper)).cloned()
    }
}
