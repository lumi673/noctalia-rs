use super::*;

impl<'a> Emitter<'a> {
    pub(super) fn stmt(&mut self, stmt: &Stmt) -> Result<(), Error> {
        match stmt {
            Stmt::Local(local) => self.local(local),
            Stmt::Item(syn::Item::Static(item)) => self.local_static(item),
            Stmt::Item(syn::Item::Const(item)) => {
                let value = constant(&item.expr).ok_or_else(|| {
                    Error::at(item.span(), "unsupported local constant")
                })?;
                self.constants.insert(item.ident.to_string(), value);
                Ok(())
            }
            Stmt::Item(syn::Item::Use(_)) => Ok(()),
            Stmt::Expr(expr, _) if diverges(expr) => {
                self.line("do");
                self.indent = self.indent.saturating_add(1);
                self.expr_stmt(expr)?;
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok(())
            }
            Stmt::Expr(expr, _) => self.expr_stmt(expr),
            other => Err(Error::at(
                other.span(),
                "unsupported statement in a compilable handler",
            )),
        }
    }

    pub(super) fn tail_stmt(&mut self, stmt: &Stmt) -> Result<(), Error> {
        match stmt {
            Stmt::Expr(expr, _) => self.tail_expr(expr),
            other => self.stmt(other),
        }
    }

    pub(super) fn tail_expr(&mut self, expr: &Expr) -> Result<(), Error> {
        if let Expr::Call(call) = expr
            && !is_ok_call(call)
            && self.inline_call(call, Inline::Tail)?.is_some()
        {
            return Ok(());
        }
        if let Expr::Macro(mac) = expr
            && macro_name(mac) == "vec"
        {
            let items = parse_vec(mac)?;
            if items.iter().any(is_op_expr) {
                for item in &items {
                    self.op_expr(item)?;
                }
                return Ok(());
            }
        }
        match expr {
            Expr::Call(call) if is_ok_call(call) => {
                if self.value_helper {
                    let value = call.args.first().ok_or_else(|| {
                        Error::at(call.span(), "Ok() needs a value")
                    })?;
                    let (text, _) = self.expr(value)?;
                    if let Some(target) = self.tail_target.clone() {
                        self.line(&format!("{target} = {text}"));
                    } else {
                        self.line(&format!("return {text}"));
                    }
                    Ok(())
                } else {
                    self.ok_tail(&call.args)
                }
            }
            Expr::If(item) => self.if_stmt(item, true),
            Expr::Match(item) => self.match_stmt(item, true),
            Expr::Return(item) => self.ret(item.expr.as_deref()),
            Expr::Block(block) => self.block(&block.block),
            Expr::Paren(paren) => self.tail_expr(&paren.expr),
            Expr::Group(group) => self.tail_expr(&group.expr),
            Expr::Tuple(tuple) if tuple.elems.is_empty() => Ok(()),
            _ => {
                let saved_statement = self.in_statement;
                self.in_statement = true;
                let result = self.expr(expr);
                self.in_statement = saved_statement;
                let (text, ty) = result?;
                if ty.base != Base::Void {
                    if let Some(target) = self.tail_target.clone() {
                        self.line(&format!("{target} = {text}"));
                    } else {
                        self.line(&format!("return {text}"));
                    }
                }
                Ok(())
            }
        }
    }

    pub(super) fn local(&mut self, local: &syn::Local) -> Result<(), Error> {
        let (pattern, hint) = match &local.pat {
            Pat::Type(typed) => (&*typed.pat, Some(&*typed.ty)),
            other => (other, None),
        };
        if let (Pat::Wild(_), Some(init)) = (pattern, &local.init)
            && self.is_ctx_expr(&init.expr)
        {
            return Ok(());
        }
        if let Some(init) = &local.init
            && let Some((_, diverge)) = &init.diverge
        {
            return self.let_else(pattern, &init.expr, diverge);
        }
        if let Pat::Tuple(tuple) = pattern {
            let initializer =
                local.init.as_ref().map(|init| &*init.expr).ok_or_else(
                    || {
                        Error::at(
                            local.span(),
                            "tuple binding needs an initializer",
                        )
                    },
                )?;
            let multi = multi_return(initializer);
            let (text, _) = self.expr(initializer)?;
            let mut names = Vec::new();
            let mut lua_names = Vec::new();
            for element in &tuple.elems {
                match element {
                    Pat::Ident(pattern) => {
                        let name = self.binding_name(pattern)?;
                        self.push(&name, Ty::VALUE);
                        names.push(name.clone());
                        lua_names.push(self.bind_name(&name));
                    }
                    Pat::Wild(_) => {
                        names.push("_".to_owned());
                        lua_names.push("_".to_owned());
                    }
                    other => {
                        return Err(Error::at(
                            other.span(),
                            "tuple bindings must be plain identifiers",
                        ));
                    }
                }
            }
            if multi {
                for name in &names {
                    if name != "_" {
                        self.push(name, Ty::OPT_VALUE);
                    }
                }
                self.line(&format!("local {} = {text}", lua_names.join(", ")));
                return Ok(());
            }
            let temp = self.temp();
            self.line(&format!("local {temp} = {text}"));
            let values = names
                .iter()
                .enumerate()
                .map(|(index, _)| format!("{temp}[{}]", index + 1))
                .collect::<Vec<_>>();
            self.line(&format!(
                "local {} = {}",
                lua_names.join(", "),
                values.join(", ")
            ));
            return Ok(());
        }
        let name = match pattern {
            Pat::Ident(pattern) => pattern.ident.to_string(),
            Pat::Wild(_) => "_".to_owned(),
            _ => {
                return Err(Error::at(
                    local.pat.span(),
                    "only plain `let` bindings are supported",
                ));
            }
        };
        let lua = self.bind_name(&name);
        let Some(init) = &local.init else {
            let ty = hint.map_or(Ty::VALUE, type_hint);
            if let Some(kind) = hint.map(static_kind)
                && kind != StaticKind::Value
            {
                self.containers.insert(name.clone(), kind);
            }
            self.push(&name, ty);
            self.line(&format!("local {lua} = nil"));
            return Ok(());
        };
        if let Expr::Call(call) = &*init.expr
            && is_vec_new(call)
        {
            self.line(&format!("local {lua} = {{}}"));
            self.push(&name, Ty::OPS);
            return Ok(());
        }
        if let Expr::Macro(mac) = &*init.expr
            && macro_name(mac) == "vec"
        {
            let items = parse_vec(mac)?;
            if items.iter().any(is_op_expr) {
                for item in &items {
                    self.op_expr(item)?;
                }
                self.line(&format!("local {lua} = nil"));
                self.push(&name, Ty::OPS);
                return Ok(());
            }
            let (text, ty) = self.list(&items)?;
            self.line(&format!("local {lua} = {text}"));
            self.push(&name, ty);
            return Ok(());
        }
        if is_default_call(&init.expr) {
            let hint =
                hint.ok_or_else(|| bare_default_error(init.expr.span()))?;
            let text = default_value(self, hint, init.expr.span())?;
            self.line(&format!("local {lua} = {text}"));
            self.push(&name, type_hint(hint));
            return Ok(());
        }
        let (text, mut ty) = self.expr(&init.expr)?;
        if ty.result
            && let Some(hint) = hint
        {
            ty.base = json_base_for_type(hint)?;
        }
        self.line(&format!("local {lua} = {text}"));
        if let Some(kind) = hint.map(static_kind)
            && kind != StaticKind::Value
        {
            self.containers.insert(name.clone(), kind);
        }
        if let Some(class) = self.pending_class.take() {
            self.classes.insert(name.clone(), class);
        }
        self.push(&name, ty);
        Ok(())
    }

    fn let_else(
        &mut self,
        pattern: &Pat,
        init: &Expr,
        diverge: &Expr,
    ) -> Result<(), Error> {
        let (text, ty) = self.expr(init)?;
        let value = self.temp();
        let error = if ty.result {
            let error = self.temp();
            self.line(&format!("local {value}, {error} = {text}"));
            Some(error)
        } else {
            self.line(&format!("local {value} = {text}"));
            None
        };
        let (condition, bindings) =
            self.pattern_match(&value, pattern, error.as_deref())?;
        for (name, text) in bindings {
            let lua = self.bind_name(&name);
            self.line(&format!("local {lua} = {text}"));
            self.push(&name, Ty::VALUE);
        }
        self.line(&format!("if not ({condition}) then"));
        self.indent = self.indent.saturating_add(1);
        self.expr_stmt(diverge)?;
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        Ok(())
    }

    pub(super) fn expr_stmt(&mut self, expr: &Expr) -> Result<(), Error> {
        if let Expr::Call(call) = expr
            && !is_ok_call(call)
            && !is_ops_push(self, call)
            && self.inline_call(call, Inline::Statement)?.is_some()
        {
            return Ok(());
        }
        match expr {
            Expr::Call(call) if is_ok_call(call) => self.ok_tail(&call.args),
            Expr::Call(call) if is_ops_push(self, call) => {
                let value = call.args.first().ok_or_else(|| {
                    Error::at(call.span(), "push() needs a value")
                })?;
                self.op_expr(value)
            }
            Expr::If(item) => self.if_stmt(item, false),
            Expr::Match(item) => self.match_stmt(item, false),
            Expr::ForLoop(item) => self.for_stmt(item),
            Expr::While(item) => {
                let condition = match strip_reference(&item.cond) {
                    Expr::Let(condition) => self.let_condition(condition)?,
                    other => self.expr(other)?.0,
                };
                self.line(&format!("while {condition} do"));
                self.indent = self.indent.saturating_add(1);
                self.statement_block(&item.body)?;
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok(())
            }
            Expr::Loop(item) => {
                self.line("while true do");
                self.indent = self.indent.saturating_add(1);
                self.statement_block(&item.body)?;
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok(())
            }
            Expr::Break(_) => {
                self.line("break");
                Ok(())
            }
            Expr::Continue(_) => {
                self.line("continue");
                Ok(())
            }
            Expr::Assign(item) => self.assign(&item.left, &item.right, "="),
            Expr::Return(item) => self.ret(item.expr.as_deref()),
            Expr::Block(block) => {
                self.line("do");
                self.indent = self.indent.saturating_add(1);
                self.scopes.push(HashMap::new());
                self.local_statics.push(HashMap::new());
                let result = self.block_inner(&block.block, false);
                self.local_statics.pop();
                self.scopes.pop();
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                result
            }
            Expr::Struct(_) => self.op_expr(expr),
            Expr::Macro(mac) if macro_name(mac) == "vec" => {
                for item in parse_vec(mac)? {
                    self.op_expr(&item)?;
                }
                Ok(())
            }
            Expr::Paren(paren) => self.expr_stmt(&paren.expr),
            Expr::Group(group) => self.expr_stmt(&group.expr),
            Expr::Tuple(tuple) if tuple.elems.is_empty() => Ok(()),
            _ => {
                let saved_statement = self.in_statement;
                self.in_statement = true;
                let result = self.expr(expr);
                self.in_statement = saved_statement;
                let (text, ty) = result?;
                if ty.base == Base::Void {
                    if !text.is_empty() && !is_identifier(&text) {
                        if text.starts_with('(') || text.starts_with('{') {
                            self.line("do");
                            self.indent = self.indent.saturating_add(1);
                            self.line(&text);
                            self.indent = self.indent.saturating_sub(1);
                            self.line("end");
                        } else {
                            self.line(&text);
                        }
                    }
                    self.emitted = true;
                    return Ok(());
                }
                if matches!(expr, Expr::MethodCall(_)) {
                    self.emitted = true;
                    return Ok(());
                }
                Err(Error::at(
                    expr.span(),
                    "this expression has no effect and is not a supported op",
                ))
            }
        }
    }

    pub(super) fn ret(&mut self, expr: Option<&Expr>) -> Result<(), Error> {
        let Some(expr) = expr else {
            if let Some(target) = self.tail_target.clone() {
                self.line(&format!("{target} = nil"));
            } else {
                self.line("return");
            }
            return Ok(());
        };
        if let Expr::Call(call) = expr {
            if is_ok_call(call) {
                return self.ok(&call.args);
            }
            if is_err_call(call) {
                let (text, _) =
                    self.expr(call.args.first().ok_or_else(|| {
                        Error::at(call.span(), "Err() needs a value")
                    })?)?;
                self.line(&format!("error({text})"));
                return Ok(());
            }
        }
        if let Expr::Macro(mac) = expr
            && macro_name(mac) == "vec"
        {
            let items = parse_vec(mac)?;
            if items.iter().any(is_op_expr) {
                for item in &items {
                    self.op_expr(item)?;
                }
                if self.tail_target.is_none() {
                    self.line("return");
                }
                return Ok(());
            }
        }
        let (text, _) = self.expr(expr)?;
        if let Some(target) = self.tail_target.clone() {
            self.line(&format!("{target} = {text}"));
        } else {
            self.line(&format!("return {text}"));
        }
        Ok(())
    }

    pub(super) fn ok(
        &mut self,
        args: &Punctuated<Expr, Token![,]>,
    ) -> Result<(), Error> {
        self.ok_impl(args, true)
    }

    fn ok_tail(
        &mut self,
        args: &Punctuated<Expr, Token![,]>,
    ) -> Result<(), Error> {
        self.ok_impl(args, false)
    }

    fn ok_impl(
        &mut self,
        args: &Punctuated<Expr, Token![,]>,
        emit_return: bool,
    ) -> Result<(), Error> {
        let value = args.first().ok_or_else(|| {
            Error::at(proc_macro2::Span::call_site(), "Ok() needs a value")
        })?;
        if matches!(value, Expr::Tuple(tuple) if tuple.elems.is_empty()) {
            if emit_return {
                self.line("return");
            }
            return Ok(());
        }
        if let Expr::Call(call) = value
            && let Some(helper) = self.helper(call)
            && matches!(return_kind(&helper.function)?, Returns::VecOps)
        {
            let check = self.check_inline(
                &helper,
                Inline::Ops,
                Returns::VecOps,
                &call.span(),
            )?;
            let bindings = self.bindings(&helper, call)?;
            self.inline(
                &helper,
                bindings,
                Inline::Ops,
                Returns::VecOps,
                check,
            )?;
            if emit_return {
                self.line("return");
            }
            return Ok(());
        }
        if let Expr::Call(call) = value
            && is_vec_new(call)
        {
            if emit_return {
                self.line("return");
            }
            return Ok(());
        }
        if let Expr::Macro(mac) = value
            && macro_name(mac) == "vec"
        {
            for item in parse_vec(mac)? {
                self.op_expr(&item)?;
            }
            if emit_return {
                self.line("return");
            }
            return Ok(());
        }
        if let Expr::Path(path) = value
            && let Some(segment) = path.path.get_ident()
            && self
                .lookup(&segment.to_string())
                .is_some_and(|ty| matches!(ty.base, Base::Ops | Base::List))
        {
            if emit_return {
                self.line("return");
            }
            return Ok(());
        }
        Err(Error::at(
            value.span(),
            "Ok(...) must wrap Vec::new(), vec![...] or a local built from them",
        ))
    }

    fn if_stmt(&mut self, item: &syn::ExprIf, tail: bool) -> Result<(), Error> {
        let (chain, else_branch) = if_chain(item);
        if chain
            .iter()
            .all(|(condition, _)| self.const_bool(condition).is_some())
        {
            for (condition, block) in &chain {
                if self.const_bool(condition) == Some(true) {
                    return self.inline_block(block, tail);
                }
            }
            return self.emit_else(else_branch, tail);
        }
        self.write_if_chain(&chain, else_branch, tail, false)
    }

    fn emit_else(
        &mut self,
        else_branch: Option<&Expr>,
        tail: bool,
    ) -> Result<(), Error> {
        match else_branch {
            None => Ok(()),
            Some(Expr::Block(block)) => self.inline_block(&block.block, tail),
            Some(other) => Err(Error::at(
                other.span(),
                "only `else` and `else if` are supported",
            )),
        }
    }

    pub(super) fn let_condition(
        &mut self,
        item: &syn::ExprLet,
    ) -> Result<String, Error> {
        let (text, ty) = self.expr(&item.expr)?;
        let value = self.temp();
        let error = if ty.result {
            let error = self.temp();
            self.line(&format!("local {value}, {error} = {text}"));
            Some(error)
        } else {
            self.line(&format!("local {value} = {text}"));
            None
        };
        let (condition, bindings) =
            self.pattern_match(&value, &item.pat, error.as_deref())?;
        for (name, text) in bindings {
            let lua = self.bind_name(&name);
            self.line(&format!("local {lua} = {text}"));
            self.push(&name, Ty::VALUE);
        }
        Ok(condition)
    }

    fn write_if_chain(
        &mut self,
        chain: &[(&Expr, &syn::Block)],
        else_branch: Option<&Expr>,
        tail: bool,
        elseif: bool,
    ) -> Result<(), Error> {
        let Some((condition, block)) = chain.first().copied() else {
            return self.emit_else(else_branch, tail);
        };
        let (condition, _) = match condition {
            Expr::Let(item) => (self.let_condition(item)?, Ty::BOOL),
            _ => self.expr(condition)?,
        };
        let keyword = if elseif { "elseif" } else { "if" };
        self.line(&format!("{keyword} {condition} then"));
        self.indent = self.indent.saturating_add(1);
        self.block_inner(block, tail)?;
        self.indent = self.indent.saturating_sub(1);
        if chain.len() > 1 {
            if !simple_condition(chain[1].0) {
                self.line("else");
                self.indent = self.indent.saturating_add(1);
                self.write_if_chain(&chain[1..], else_branch, tail, false)?;
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(());
            }
            return self.write_if_chain(&chain[1..], else_branch, tail, true);
        }
        match else_branch {
            Some(Expr::Block(block)) => {
                self.line("else");
                self.indent = self.indent.saturating_add(1);
                self.block_inner(&block.block, tail)?;
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok(())
            }
            None => {
                self.line("end");
                Ok(())
            }
            Some(other) => Err(Error::at(
                other.span(),
                "only `else` and `else if` are supported",
            )),
        }
    }

    pub(super) fn const_bool(&self, expr: &Expr) -> Option<bool> {
        match expr {
            Expr::Lit(ExprLit {
                lit: Lit::Bool(value),
                ..
            }) => Some(value.value),
            Expr::Paren(paren) => self.const_bool(&paren.expr),
            Expr::Group(group) => self.const_bool(&group.expr),
            Expr::Reference(reference) => self.const_bool(&reference.expr),
            Expr::Unary(unary) if matches!(unary.op, UnOp::Not(_)) => {
                self.const_bool(&unary.expr).map(|value| !value)
            }
            Expr::Binary(binary) => {
                if matches!(binary.op, BinOp::And(_)) {
                    return Some(
                        self.const_bool(&binary.left)?
                            && self.const_bool(&binary.right)?,
                    );
                }
                if matches!(binary.op, BinOp::Or(_)) {
                    return Some(
                        self.const_bool(&binary.left)?
                            || self.const_bool(&binary.right)?,
                    );
                }
                let negate = matches!(binary.op, BinOp::Ne(_));
                if !negate && !matches!(binary.op, BinOp::Eq(_)) {
                    return None;
                }
                let left = self.const_literal(&binary.left);
                let right = self.const_literal(&binary.right);
                let value = match (
                    self.is_event_expr(&binary.left),
                    self.is_event_expr(&binary.right),
                ) {
                    (true, false) => {
                        self.bound_event.as_deref()? == right?.as_str()
                    }
                    (false, true) => {
                        self.bound_event.as_deref()? == left?.as_str()
                    }
                    (false, false) => left? == right?,
                    (true, true) => return None,
                };
                Some(if negate { !value } else { value })
            }
            Expr::MethodCall(call) if call.method == "is_load_event" => {
                match self.bound_event.as_deref() {
                    Some("load") => Some(true),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn const_literal(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Lit(ExprLit {
                lit: Lit::Str(value),
                ..
            }) => Some(value.value()),
            Expr::Lit(ExprLit {
                lit: Lit::Int(value),
                ..
            }) => Some(value.base10_digits().to_owned()),
            Expr::Lit(ExprLit {
                lit: Lit::Bool(value),
                ..
            }) => Some(if value.value { "true" } else { "false" }.to_owned()),
            Expr::Paren(paren) => self.const_literal(&paren.expr),
            Expr::Group(group) => self.const_literal(&group.expr),
            _ => None,
        }
    }

    pub(super) fn match_stmt(
        &mut self,
        item: &syn::ExprMatch,
        tail: bool,
    ) -> Result<(), Error> {
        let guarded =
            item.arms.iter().any(|arm| matches!(arm.pat, Pat::Guard(_)));
        if !guarded
            && let Some(bound) = self.bound_event.clone()
            && self.is_event_expr(&item.expr)
        {
            for arm in &item.arms {
                match pattern_strings(&arm.pat)? {
                    Some(patterns)
                        if patterns.iter().any(|pattern| pattern == &bound) =>
                    {
                        return self.inline_arm(&arm.body, tail);
                    }
                    Some(_) => {}
                    None if is_wild(&arm.pat) => {
                        return self.inline_arm(&arm.body, tail);
                    }
                    None => {
                        return Err(Error::at(
                            arm.pat.span(),
                            "only string literal, wildcard and guarded match patterns are supported by the Luau compiler",
                        ));
                    }
                }
            }
            return Ok(());
        }
        let (mut scrutinee, scrutinee_ty) = self.expr(&item.expr)?;
        let result_error = if scrutinee_ty.result {
            let value = self.temp();
            let error = self.temp();
            self.line(&format!("local {value}, {error} = {scrutinee}"));
            scrutinee = value;
            Some(error)
        } else {
            None
        };
        let event_match = self.is_event_expr(&item.expr);
        let mut opened = false;
        if event_match {
            for arm in &item.arms {
                if let Some(patterns) = pattern_strings(&arm.pat)? {
                    for pattern in patterns {
                        self.events.insert(pattern);
                    }
                }
            }
        }
        for arm in &item.arms {
            let (pattern, guard) = split_guard(&arm.pat);
            let (condition, bindings) = self.pattern_match(
                &scrutinee,
                pattern,
                result_error.as_deref(),
            )?;
            let keyword = if opened { "elseif" } else { "if" };
            self.line(&format!("{keyword} {condition} then"));
            self.indent = self.indent.saturating_add(1);
            self.scopes.push(HashMap::new());
            for (name, text) in bindings {
                let lua = self.bind_name(&name);
                self.line(&format!("local {lua} = {text}"));
                self.push(&name, Ty::VALUE);
            }
            if let Some(guard) = guard {
                let (text, _) = self.expr(guard)?;
                self.line(&format!("if {text} then"));
                self.indent = self.indent.saturating_add(1);
            }
            let result = self.arm_body(&arm.body, tail);
            if guard.is_some() {
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
            }
            self.scopes.pop();
            result?;
            self.indent = self.indent.saturating_sub(1);
            opened = true;
        }
        if opened {
            self.line("end");
        }
        Ok(())
    }

    fn binding_name(&self, pattern: &syn::PatIdent) -> Result<String, Error> {
        let name = pattern.ident.to_string();
        Ok(name)
    }

    pub(super) fn pattern_match(
        &self,
        value: &str,
        pattern: &Pat,
        error: Option<&str>,
    ) -> Result<(String, PatternBindings), Error> {
        match pattern {
            Pat::Wild(_) => Ok(("true".to_owned(), Vec::new())),
            Pat::Paren(paren) => self.pattern_match(value, &paren.pat, error),
            Pat::Lit(lit) => {
                let literal = Expr::Lit(lit.clone());
                let (text, _) = constant(&literal).ok_or_else(|| {
                    Error::at(lit.span(), "unsupported match literal")
                })?;
                Ok((format!("({value} == {text})"), Vec::new()))
            }
            Pat::Or(or) => {
                let mut conditions = Vec::new();
                let mut bindings = Vec::new();
                for case in &or.cases {
                    let (condition, case_bindings) =
                        self.pattern_match(value, case, error)?;
                    conditions.push(condition);
                    if bindings.is_empty() {
                        bindings = case_bindings;
                    }
                }
                Ok((format!("({})", conditions.join(" or ")), bindings))
            }
            Pat::Ident(pattern) => {
                let name = self.binding_name(pattern)?;
                if let Some((_, subpat)) = &pattern.subpat {
                    let (condition, mut bindings) =
                        self.pattern_match(value, subpat, error)?;
                    bindings.insert(0, (name.clone(), value.to_owned()));
                    return Ok((condition, bindings));
                }
                match name.as_str() {
                    "None" | "Null" => {
                        Ok((format!("({value} == nil)"), Vec::new()))
                    }
                    _ => {
                        Ok(("true".to_owned(), vec![(name, value.to_owned())]))
                    }
                }
            }
            Pat::Path(path) => {
                let name = path
                    .path
                    .segments
                    .last()
                    .map(|segment| segment.ident.to_string())
                    .unwrap_or_default();
                match name.as_str() {
                    "None" | "Null" => {
                        Ok((format!("({value} == nil)"), Vec::new()))
                    }
                    "Some" => Ok((format!("({value} ~= nil)"), Vec::new())),
                    _ => {
                        let segments = path
                            .path
                            .segments
                            .iter()
                            .map(|segment| segment.ident.to_string())
                            .collect::<Vec<_>>();
                        if let Some(variant) =
                            self.program.enum_variant(&self.module, &segments)
                        {
                            return Ok((
                                format!("({value} == {})", quote(&variant)),
                                Vec::new(),
                            ));
                        }
                        Err(Error::at(
                            path.span(),
                            format!(
                                "match pattern `{name}` is not supported by the Luau compiler"
                            ),
                        ))
                    }
                }
            }
            Pat::TupleStruct(item)
                if error.is_some()
                    && item
                        .path
                        .segments
                        .last()
                        .is_some_and(|segment| segment.ident == "Ok") =>
            {
                let error = error.unwrap_or_default();
                let mut conditions = vec![format!("({error} == nil)")];
                let mut bindings = Vec::new();
                for element in &item.elems {
                    self.nested_pattern(
                        value,
                        element,
                        &mut conditions,
                        &mut bindings,
                    )?;
                }
                Ok((format!("({})", conditions.join(" and ")), bindings))
            }
            Pat::TupleStruct(item)
                if error.is_some()
                    && item
                        .path
                        .segments
                        .last()
                        .is_some_and(|segment| segment.ident == "Err") =>
            {
                let error = error.unwrap_or_default();
                let mut conditions = vec![format!("({error} ~= nil)")];
                let mut bindings = Vec::new();
                for element in &item.elems {
                    let text = format!("tostring({error})");
                    self.nested_pattern(
                        &text,
                        element,
                        &mut conditions,
                        &mut bindings,
                    )?;
                }
                Ok((format!("({})", conditions.join(" and ")), bindings))
            }
            Pat::TupleStruct(item) => {
                let name = item
                    .path
                    .segments
                    .last()
                    .map(|segment| segment.ident.to_string())
                    .unwrap_or_default();
                let mut conditions = Vec::new();
                let mut bindings = Vec::new();
                match name.as_str() {
                    "Some" => conditions.push(format!("({value} ~= nil)")),
                    "Null" | "None" => {
                        conditions.push(format!("({value} == nil)"))
                    }
                    "Bool" | "String" | "Number" | "Object" | "Array" => {
                        let lua_type = match name.as_str() {
                            "Bool" => "boolean",
                            "String" => "string",
                            "Number" => "number",
                            _ => "table",
                        };
                        conditions
                            .push(format!("(type({value}) == \"{lua_type}\")"));
                    }
                    "Ok" => conditions.push(format!("({value} ~= nil)")),
                    "Err" => conditions.push(format!("({value} == nil)")),
                    _ => {
                        let segments = item
                            .path
                            .segments
                            .iter()
                            .map(|segment| segment.ident.to_string())
                            .collect::<Vec<_>>();
                        if let Some(variant) =
                            self.program.enum_variant(&self.module, &segments)
                        {
                            conditions.push(format!(
                                "({value} == {})",
                                quote(&variant)
                            ));
                        } else {
                            return Err(Error::at(
                                item.span(),
                                format!(
                                    "match pattern `{name}` is not supported by the Luau compiler"
                                ),
                            ));
                        }
                    }
                }
                for element in &item.elems {
                    self.nested_pattern(
                        value,
                        element,
                        &mut conditions,
                        &mut bindings,
                    )?;
                }
                Ok((format!("({})", conditions.join(" and ")), bindings))
            }
            Pat::Slice(slice) => {
                let mut conditions = Vec::new();
                let mut bindings = Vec::new();
                for (index, element) in slice.elems.iter().enumerate() {
                    let item = format!("({value})[{}]", index + 1);
                    conditions.push(format!("({item} ~= nil)"));
                    self.nested_pattern(
                        &item,
                        element,
                        &mut conditions,
                        &mut bindings,
                    )?;
                }
                Ok((format!("({})", conditions.join(" and ")), bindings))
            }
            Pat::Tuple(tuple) => {
                let mut conditions = Vec::new();
                let mut bindings = Vec::new();
                for (index, element) in tuple.elems.iter().enumerate() {
                    let item = format!("({value})[{}]", index + 1);
                    self.nested_pattern(
                        &item,
                        element,
                        &mut conditions,
                        &mut bindings,
                    )?;
                }
                Ok((format!("({})", conditions.join(" and ")), bindings))
            }
            Pat::Range(range) => {
                let mut conditions = Vec::new();
                if let Some(start) = range.start.as_deref()
                    && let Some((text, _)) = constant(start)
                {
                    let operator = if matches!(
                        range.limits,
                        syn::RangeLimits::Closed(_)
                    ) {
                        ">="
                    } else {
                        ">"
                    };
                    conditions.push(format!("({value} {operator} {text})"));
                }
                if let Some(end) = range.end.as_deref()
                    && let Some((text, _)) = constant(end)
                {
                    let operator = if matches!(
                        range.limits,
                        syn::RangeLimits::Closed(_)
                    ) {
                        "<="
                    } else {
                        "<"
                    };
                    conditions.push(format!("({value} {operator} {text})"));
                }
                Ok((format!("({})", conditions.join(" and ")), Vec::new()))
            }
            Pat::Struct(item) => {
                let mut conditions =
                    vec![format!("(type({value}) == \"table\")")];
                let mut bindings = Vec::new();
                for field in &item.fields {
                    let Member::Named(name) = &field.member else {
                        continue;
                    };
                    let member =
                        format!("{value}.{}", lua_key(&name.to_string()));
                    self.nested_pattern(
                        &member,
                        &field.pat,
                        &mut conditions,
                        &mut bindings,
                    )?;
                }
                Ok((format!("({})", conditions.join(" and ")), bindings))
            }
            _ => Err(Error::at(
                pattern.span(),
                "match pattern is not supported by the Luau compiler",
            )),
        }
    }

    /// The Luau literal for a constant used as a match pattern.
    pub(super) fn pattern_constant(&self, name: &str) -> Option<(String, Ty)> {
        if name.chars().any(char::is_lowercase) {
            return None;
        }
        if let Some(value) = self.constant(name) {
            return Some(value);
        }
        let value = self.program.imported_constant(&self.module, name)?;
        constant(value)
    }

    pub(super) fn nested_pattern(
        &self,
        value: &str,
        pattern: &Pat,
        conditions: &mut Vec<String>,
        bindings: &mut PatternBindings,
    ) -> Result<(), Error> {
        match pattern {
            Pat::Ident(pattern) => {
                let name = self.binding_name(pattern)?;
                match name.as_str() {
                    "None" | "Null" => {
                        conditions.push(format!("({value} == nil)"));
                        return Ok(());
                    }
                    "Some" => {
                        conditions.push(format!("({value} ~= nil)"));
                        return Ok(());
                    }
                    _ => {}
                }
                if let Some((_, subpat)) = &pattern.subpat
                    && self.pattern_constant(&name).is_none()
                {
                    bindings.push((name.clone(), value.to_owned()));
                    self.nested_pattern(value, subpat, conditions, bindings)?;
                    return Ok(());
                }
                match self.pattern_constant(&name) {
                    Some((text, _)) => {
                        conditions.push(format!("({value} == {text})"));
                    }
                    None => bindings.push((name, value.to_owned())),
                }
            }
            Pat::Wild(_) => {}
            Pat::Or(or) => {
                let mut alternatives = Vec::new();
                let mut open = false;
                for case in &or.cases {
                    let mut case_conditions = Vec::new();
                    let mut case_bindings = Vec::new();
                    self.nested_pattern(
                        value,
                        case,
                        &mut case_conditions,
                        &mut case_bindings,
                    )?;
                    if case_conditions.is_empty() {
                        open = true;
                        continue;
                    }
                    alternatives
                        .push(format!("({})", case_conditions.join(" and ")));
                }
                if !open {
                    conditions.push(format!("({})", alternatives.join(" or ")));
                }
            }
            Pat::Paren(paren) => {
                self.nested_pattern(value, &paren.pat, conditions, bindings)?;
            }
            Pat::Reference(reference) => {
                self.nested_pattern(
                    value,
                    &reference.pat,
                    conditions,
                    bindings,
                )?;
            }
            Pat::Lit(lit) => {
                let literal = Expr::Lit(lit.clone());
                let (text, _) = constant(&literal).ok_or_else(|| {
                    Error::at(lit.span(), "unsupported match literal")
                })?;
                conditions.push(format!("({value} == {text})"));
            }
            Pat::Path(path) => {
                let name = path
                    .path
                    .segments
                    .last()
                    .map(|segment| segment.ident.to_string())
                    .unwrap_or_default();
                match name.as_str() {
                    "None" | "Null" => {
                        conditions.push(format!("({value} == nil)"))
                    }
                    "Some" => conditions.push(format!("({value} ~= nil)")),
                    _ => {
                        let segments = path
                            .path
                            .segments
                            .iter()
                            .map(|segment| segment.ident.to_string())
                            .collect::<Vec<_>>();
                        if let Some(variant) =
                            self.program.enum_variant(&self.module, &segments)
                        {
                            conditions.push(format!(
                                "({value} == {})",
                                quote(&variant)
                            ));
                        } else {
                            return Err(Error::at(
                                path.span(),
                                format!(
                                    "match pattern `{name}` is not supported by the Luau compiler"
                                ),
                            ));
                        }
                    }
                }
            }
            Pat::TupleStruct(item) => {
                let name = item
                    .path
                    .segments
                    .last()
                    .map(|segment| segment.ident.to_string())
                    .unwrap_or_default();
                match name.as_str() {
                    "Some" => conditions.push(format!("({value} ~= nil)")),
                    "None" | "Null" => {
                        conditions.push(format!("({value} == nil)"))
                    }
                    "Bool" | "String" | "Number" | "Object" | "Array" => {
                        let lua_type = match name.as_str() {
                            "Bool" => "boolean",
                            "String" => "string",
                            "Number" => "number",
                            _ => "table",
                        };
                        conditions
                            .push(format!("(type({value}) == \"{lua_type}\")"));
                    }
                    "Ok" => conditions.push(format!("({value} ~= nil)")),
                    "Err" => conditions.push(format!("({value} == nil)")),
                    _ => {
                        let segments = item
                            .path
                            .segments
                            .iter()
                            .map(|segment| segment.ident.to_string())
                            .collect::<Vec<_>>();
                        if let Some(variant) =
                            self.program.enum_variant(&self.module, &segments)
                        {
                            conditions.push(format!(
                                "({value} == {})",
                                quote(&variant)
                            ));
                        } else {
                            return Err(Error::at(
                                item.span(),
                                format!(
                                    "match pattern `{name}` is not supported by the Luau compiler"
                                ),
                            ));
                        }
                    }
                }
                for element in &item.elems {
                    self.nested_pattern(value, element, conditions, bindings)?;
                }
            }
            Pat::Tuple(tuple) => {
                for (index, element) in tuple.elems.iter().enumerate() {
                    let item = format!("({value})[{}]", index + 1);
                    conditions.push(format!("({item} ~= nil)"));
                    self.nested_pattern(&item, element, conditions, bindings)?;
                }
            }
            Pat::Slice(slice) => {
                for (index, element) in slice.elems.iter().enumerate() {
                    let item = format!("({value})[{}]", index + 1);
                    conditions.push(format!("({item} ~= nil)"));
                    self.nested_pattern(&item, element, conditions, bindings)?;
                }
            }
            Pat::Struct(item) => {
                conditions.push(format!("(type({value}) == \"table\")"));
                for field in &item.fields {
                    let Member::Named(name) = &field.member else {
                        continue;
                    };
                    let member =
                        format!("{value}.{}", lua_key(&name.to_string()));
                    self.nested_pattern(
                        &member, &field.pat, conditions, bindings,
                    )?;
                }
            }
            Pat::Range(range) => {
                if let Some(start) = range.start.as_deref()
                    && let Some((text, _)) = constant(start)
                {
                    let operator = if matches!(
                        range.limits,
                        syn::RangeLimits::Closed(_)
                    ) {
                        ">="
                    } else {
                        ">"
                    };
                    conditions.push(format!("({value} {operator} {text})"));
                }
                if let Some(end) = range.end.as_deref()
                    && let Some((text, _)) = constant(end)
                {
                    let operator = if matches!(
                        range.limits,
                        syn::RangeLimits::Closed(_)
                    ) {
                        "<="
                    } else {
                        "<"
                    };
                    conditions.push(format!("({value} {operator} {text})"));
                }
            }
            Pat::Type(typed) => {
                self.nested_pattern(value, &typed.pat, conditions, bindings)?;
            }
            Pat::Rest(_) => {}
            _ => {
                return Err(Error::at(
                    pattern.span(),
                    "nested match patterns are not supported by the Luau compiler",
                ));
            }
        }
        Ok(())
    }

    fn for_stmt(&mut self, item: &syn::ExprForLoop) -> Result<(), Error> {
        let mut bindings: Vec<Option<String>> = Vec::new();
        match &*item.pat {
            Pat::Ident(pattern) => {
                bindings.push(Some(self.binding_name(pattern)?))
            }
            Pat::Tuple(tuple) => {
                for element in &tuple.elems {
                    match element {
                        Pat::Ident(pattern) => {
                            bindings.push(Some(self.binding_name(pattern)?))
                        }
                        Pat::Wild(_) => bindings.push(None),
                        other => {
                            return Err(Error::at(
                                other.span(),
                                "tuple loop bindings must be plain identifiers",
                            ));
                        }
                    }
                }
            }
            other => {
                return Err(Error::at(
                    other.span(),
                    "only plain and tuple loop variables are supported",
                ));
            }
        }
        let lua_bindings = bindings
            .iter()
            .map(|binding| match binding.as_deref() {
                Some(name) => self.bind_name(name),
                None => "_".to_owned(),
            })
            .collect::<Vec<_>>();
        let mut iterable = strip_reference(&item.expr);
        let enumerate = if let Expr::MethodCall(call) = iterable
            && call.method == "enumerate"
        {
            iterable = strip_reference(&call.receiver);
            true
        } else {
            false
        };
        if let Expr::Range(range) = iterable
            && let Some(name) = bindings.first().and_then(Option::as_deref)
            && bindings.len() == 1
        {
            let start = match range.start.as_deref() {
                Some(start) => self.expr(start)?.0,
                None => "0".to_owned(),
            };
            let inclusive = matches!(range.limits, syn::RangeLimits::Closed(_));
            let Some(end) = range.end.as_deref() else {
                return Err(Error::at(
                    range.span(),
                    "open-ended ranges are not supported",
                ));
            };
            let end = self.expr(end)?.0;
            let end = if inclusive {
                end
            } else {
                format!("({end}) - 1")
            };
            self.push(name, Ty::INT);
            self.line(&format!("for {} = {start}, {end} do", lua_bindings[0]));
            self.indent = self.indent.saturating_add(1);
            self.statement_block(&item.body)?;
            self.indent = self.indent.saturating_sub(1);
            self.line("end");
            return Ok(());
        }
        if enumerate && bindings.len() != 2 {
            return Err(Error::at(
                item.pat.span(),
                "`enumerate()` needs a two-element tuple binding",
            ));
        }
        let (text, ty) = self.expr(iterable)?;
        if ty.base != Base::List
            && ty.base != Base::Value
            && ty.base != Base::Table
        {
            return Err(Error::at(
                item.expr.span(),
                "only ranges, list and map values can be looped over",
            ));
        }
        let source = self.temp();
        self.line(&format!("local {source} = {text}"));
        if ty.base == Base::Table {
            if enumerate {
                return Err(Error::at(
                    item.pat.span(),
                    "`enumerate()` needs a list, not a map",
                ));
            }
            let names = lua_bindings.clone();
            let head = match names.as_slice() {
                [name] => format!("_, {name}"),
                [first, second] => format!("{first}, {second}"),
                _ => {
                    return Err(Error::at(
                        item.pat.span(),
                        "map loops take one or two bindings",
                    ));
                }
            };
            for name in bindings.iter().flatten() {
                self.push(name, Ty::VALUE);
            }
            self.line(&format!("for {head} in pairs({source}) do"));
            self.indent = self.indent.saturating_add(1);
            self.statement_block(&item.body)?;
            self.indent = self.indent.saturating_sub(1);
            self.line("end");
            return Ok(());
        }
        if bindings.len() > 1 && ty.base == Base::Value && !enumerate {
            if bindings.len() != 2 {
                return Err(Error::at(
                    item.pat.span(),
                    "tuple loop bindings over a JSON object need two names",
                ));
            }
            self.line(&format!(
                "if type({source}) == \"table\" and type({source}[1]) == \"table\" then"
            ));
            self.indent = self.indent.saturating_add(1);
            let index = self.temp();
            self.line(&format!("local {index} = {source}.__next or 1"));
            self.line(&format!("for {index} = {index}, #{source} do"));
            self.indent = self.indent.saturating_add(1);
            for (position, name) in bindings.iter().enumerate() {
                let Some(name) = name else { continue };
                self.push(name, Ty::VALUE);
                self.line(&format!(
                    "local {} = {source}[{index}][{}]",
                    lua_bindings[position],
                    position + 1
                ));
            }
            self.line(&format!(
                "if {source}.__next ~= nil then {source}.__next = {index} + 1 end"
            ));
            self.statement_block(&item.body)?;
            self.indent = self.indent.saturating_sub(1);
            self.line("end");
            self.indent = self.indent.saturating_sub(1);
            self.line("else");
            self.indent = self.indent.saturating_add(1);
            let first = lua_bindings[0].clone();
            let second = lua_bindings[1].clone();
            for name in bindings.iter().flatten() {
                self.push(name, Ty::VALUE);
            }
            self.line(&format!(
                "for {first}, {second} in pairs(type({source}) == \"table\" and {source} or {{}}) do"
            ));
            self.indent = self.indent.saturating_add(1);
            self.statement_block(&item.body)?;
            self.indent = self.indent.saturating_sub(1);
            self.line("end");
            self.indent = self.indent.saturating_sub(1);
            self.line("end");
            return Ok(());
        }
        let index = self.temp();
        self.line(&format!("local {index} = {source}.__next or 1"));
        self.line(&format!("for {index} = {index}, #{source} do"));
        self.indent = self.indent.saturating_add(1);
        for (position, name) in bindings.iter().enumerate() {
            let Some(name) = name else {
                continue;
            };
            let value = if enumerate && position == 0 {
                format!("({index} - 1)")
            } else {
                format!(
                    "{source}[{index}]{}",
                    if enumerate {
                        String::new()
                    } else {
                        format!("[{}]", position + 1)
                    }
                )
            };
            let binding_ty = if enumerate && position == 0 {
                Ty::INT
            } else {
                Ty::VALUE
            };
            self.push(name, binding_ty);
            self.line(&format!("local {} = {value}", lua_bindings[position]));
        }
        self.line(&format!(
            "if {source}.__next ~= nil then {source}.__next = {index} + 1 end"
        ));
        self.statement_block(&item.body)?;
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        Ok(())
    }

    pub(super) fn assign(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: &str,
    ) -> Result<(), Error> {
        self.emitted = true;
        if op == "="
            && let Expr::Tuple(tuple) = strip_reference(left)
        {
            let (text, _) = self.expr(right)?;
            let source = self.temp();
            self.line(&format!("local {source} = {text}"));
            for (index, element) in tuple.elems.iter().enumerate() {
                let mut local = false;
                let target =
                    self.assign_target(strip_reference(element), &mut local)?;
                self.line(&format!("{target} = {source}[{}]", index + 1));
                if local
                    && let Some(name) = path_ident(strip_reference(element))
                {
                    self.push(&name, Ty::VALUE);
                }
            }
            return Ok(());
        }
        let mut is_local = false;
        let target =
            self.assign_target(strip_reference(left), &mut is_local)?;
        let (text, ty) = self.expr(right)?;
        if text.is_empty() || ty.base == Base::Void {
            if !text.is_empty() {
                self.line(&text);
            }
            return Ok(());
        }
        self.line(&format!("{target} {op} {text}"));
        if is_local && let Some(name) = path_ident(strip_reference(left)) {
            self.push(&name, ty);
        }
        Ok(())
    }

    fn assign_target(
        &mut self,
        expr: &Expr,
        is_local: &mut bool,
    ) -> Result<String, Error> {
        match expr {
            Expr::Path(path) => {
                let Some(name) = path.path.get_ident() else {
                    return Err(Error::at(
                        expr.span(),
                        "only assignments to local variables and table fields are supported",
                    ));
                };
                let name = name.to_string();
                if self.lookup(&name).is_none()
                    && self
                        .program
                        .resolve_static(
                            &self.module,
                            std::slice::from_ref(&name),
                        )
                        .is_none()
                    && !self.containers.contains_key(&name)
                {
                    return Err(Error::at(
                        expr.span(),
                        format!("assignment to undeclared local `{name}`"),
                    ));
                }
                *is_local = true;
                Ok(self.local_name(&name))
            }
            Expr::Field(field) => {
                let base =
                    self.assign_target(strip_reference(&field.base), is_local)?;
                match &field.member {
                    Member::Named(name) => {
                        Ok(format!("{}.{}", base, lua_key(&name.to_string())))
                    }
                    Member::Unnamed(index) => {
                        Ok(format!("{base}[{}]", index.index + 1))
                    }
                }
            }
            Expr::Index(index) => {
                let base =
                    self.assign_target(strip_reference(&index.expr), is_local)?;
                let (key, key_ty) = self.expr(&index.index)?;
                if key_ty.base == Base::Int {
                    Ok(format!("{base}[({key}) + 1]"))
                } else {
                    Ok(format!("{base}[{key}]"))
                }
            }
            Expr::Unary(unary) if matches!(unary.op, UnOp::Deref(_)) => {
                self.assign_target(&unary.expr, is_local)
            }
            _ => {
                let (text, _) = self.expr(expr)?;
                Ok(text)
            }
        }
    }
}

pub(crate) type IfChain<'a> =
    (Vec<(&'a Expr, &'a syn::Block)>, Option<&'a Expr>);

fn diverges(expr: &Expr) -> bool {
    match expr {
        Expr::Return(_) => true,
        Expr::Call(call) => is_ok_call(call),
        Expr::Paren(paren) => diverges(&paren.expr),
        Expr::Group(group) => diverges(&group.expr),
        _ => false,
    }
}

/// A condition that never emits statements, so `else if` can become `elseif`.
fn simple_condition(expr: &Expr) -> bool {
    match strip_reference(expr) {
        Expr::Lit(_) | Expr::Path(_) => true,
        Expr::Paren(paren) => simple_condition(&paren.expr),
        Expr::Group(group) => simple_condition(&group.expr),
        Expr::Unary(unary) => simple_condition(&unary.expr),
        Expr::Binary(binary) => {
            simple_condition(&binary.left) && simple_condition(&binary.right)
        }
        Expr::Field(field) => simple_condition(&field.base),
        _ => false,
    }
}

pub(super) fn if_chain(item: &syn::ExprIf) -> IfChain<'_> {
    let mut chain = Vec::new();
    let mut current = item;
    loop {
        chain.push((&*current.cond, &current.then_branch));
        let next = current.else_branch.as_ref().map(|(_, expr)| &**expr);
        match next {
            Some(Expr::If(nested)) => current = nested,
            other => return (chain, other),
        }
    }
}

fn split_guard(pattern: &Pat) -> (&Pat, Option<&Expr>) {
    match pattern {
        Pat::Guard(guard) => (&guard.pat, Some(&guard.guard)),
        other => (other, None),
    }
}

fn is_wild(pattern: &Pat) -> bool {
    matches!(pattern, Pat::Wild(_))
}

fn pattern_strings(pattern: &Pat) -> Result<Option<Vec<String>>, Error> {
    let pattern = match pattern {
        Pat::Guard(guard) => &*guard.pat,
        other => other,
    };
    match pattern {
        Pat::Lit(literal) => {
            let Lit::Str(value) = &literal.lit else {
                return Ok(None);
            };
            Ok(Some(vec![value.value()]))
        }
        Pat::Or(or) => {
            let mut values = Vec::new();
            for case in &or.cases {
                let Some(mut item) = pattern_strings(case)? else {
                    return Ok(None);
                };
                values.append(&mut item);
            }
            Ok(Some(values))
        }
        Pat::Paren(paren) => pattern_strings(&paren.pat),
        _ => Ok(None),
    }
}
