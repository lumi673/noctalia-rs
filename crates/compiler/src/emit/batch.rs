//! Batching of adjacent native calls in a straight-line block.

use super::*;

struct BatchCall<'a> {
    local: &'a syn::Local,
    call: &'a ExprCall,
    item: NativeFn,
    ty: Ty,
}

impl<'a> Emitter<'a> {
    /// Emits a maximal run of adjacent native calls as one helper request.
    ///
    /// Returns the number of statements consumed. A call qualifies only when
    /// the binding is a plain local and every argument is a literal or an
    /// existing local, so argument evaluation stays ahead of the request in
    /// exactly the same order as the unbatched calls. Calls run in order on
    /// the helper, so an argument may be a result from an earlier call in the
    /// same batch.
    pub(super) fn native_batch(
        &mut self,
        stmts: &[Stmt],
    ) -> Result<usize, Error> {
        if self.native_mode == NativeMode::Deny || self.deferred > 0 {
            return Ok(0);
        }
        let mut calls = Vec::new();
        for stmt in stmts {
            let Some(call) = self.batch_call(stmt) else {
                break;
            };
            if call.call.args.iter().any(|arg| self.batch_arg_null(arg)) {
                break;
            }
            calls.push(call);
        }
        if calls.len() < 2 {
            return Ok(0);
        }
        let mut bound = BTreeSet::new();
        for entry in &calls {
            if entry.call.args.iter().any(|arg| arg_uses(arg, &bound)) {
                return Ok(0);
            }
            let pattern = match &entry.local.pat {
                Pat::Type(typed) => &*typed.pat,
                other => other,
            };
            if let Pat::Ident(pattern) = pattern {
                bound.insert(pattern.ident.to_string());
            }
        }
        let count = calls.len();
        let table = self.temp();
        let mut descriptors = Vec::new();
        let mut needs_context = false;
        for (index, entry) in calls.iter().enumerate() {
            if entry.item.params.len() != entry.call.args.len() {
                return Ok(0);
            }
            let mut fields = Vec::new();
            for (param, expr) in entry.item.params.iter().zip(&entry.call.args)
            {
                if param.kind == NativeParamKind::Context {
                    continue;
                }
                let (text, _) = self.expr(expr)?;
                let name = self.temp();
                self.line(&format!("local {name} = {text}"));
                let key = if is_identifier(&param.name)
                    && !is_luau_keyword(&param.name)
                {
                    param.name.clone()
                } else {
                    format!("[{}]", quote(&param.name))
                };
                fields.push(format!("{key} = {name}"));
            }
            let context = entry
                .item
                .params
                .iter()
                .any(|param| param.kind == NativeParamKind::Context);
            needs_context |= context;
            let args = if fields.is_empty() {
                "{}".to_owned()
            } else {
                format!("{{ {} }}", fields.join(", "))
            };
            descriptors.push(format!(
                "{{ id = {}, name = {}, args = {args}, cache_ms = {}, needs_context = {context}, result = false }}",
                quote(&index.to_string()),
                quote(&entry.item.name),
                entry.item.cache_ms,
            ));
        }
        self.line(&format!(
            "local {table} = __native.batch({{ {} }})",
            descriptors.join(", ")
        ));
        for (index, entry) in calls.iter().enumerate() {
            let (pattern, hint) = match &entry.local.pat {
                Pat::Type(typed) => (&*typed.pat, Some(&*typed.ty)),
                other => (other, None),
            };
            let Pat::Ident(pattern) = pattern else {
                return Ok(0);
            };
            let name = pattern.ident.to_string();
            let lua = self.bind_name(&name);
            let ty = hint.map_or(entry.ty, type_hint);
            self.push(&name, ty);
            self.line(&format!("local {lua} = {table}[{}].value", index + 1));
        }
        for entry in calls {
            self.native_found
                .insert(entry.item.name.clone(), entry.item);
        }
        self.uses_native = true;
        self.uses_native_context |= needs_context;
        Ok(count)
    }

    fn batch_arg_null(&self, expr: &Expr) -> bool {
        match strip_reference(expr) {
            Expr::Path(path) => path
                .path
                .get_ident()
                .and_then(|name| self.lookup(&name.to_string()))
                .is_some_and(|ty| ty.null),
            _ => false,
        }
    }

    fn batch_call<'b>(&self, stmt: &'b Stmt) -> Option<BatchCall<'b>> {
        let Stmt::Local(local) = stmt else {
            return None;
        };
        let pattern = match &local.pat {
            Pat::Type(typed) => &*typed.pat,
            other => other,
        };
        let Pat::Ident(pattern) = pattern else {
            return None;
        };
        if pattern.by_ref.is_some() || pattern.mutability.is_some() {
            return None;
        }
        let init = local.init.as_ref()?;
        if init.diverge.is_some() {
            return None;
        }
        let Expr::Call(call) = &*init.expr else {
            return None;
        };
        if !call.args.iter().all(is_batch_arg) {
            return None;
        }
        let helper = self.helper(call)?;
        let item = self.known_native.get(&native_key(&helper))?.clone();
        if item.result {
            return None;
        }
        let returns = return_kind(&helper.function).ok()?;
        if !matches!(returns, Returns::Value(_)) {
            return None;
        }
        Some(BatchCall {
            local,
            call,
            item,
            ty: value_type(returns),
        })
    }
}

fn is_batch_arg(expr: &Expr) -> bool {
    matches!(strip_reference(expr), Expr::Lit(_) | Expr::Path(_))
}

fn arg_uses(expr: &Expr, bound: &BTreeSet<String>) -> bool {
    match strip_reference(expr) {
        Expr::Path(path) => path
            .path
            .get_ident()
            .is_some_and(|name| bound.contains(&name.to_string())),
        _ => false,
    }
}
