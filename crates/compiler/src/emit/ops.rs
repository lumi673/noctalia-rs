use super::*;

/// Marker for a typed callback whose id differs by branch or was overwritten.
pub(super) const UNRESOLVED_CALLBACK_ID: &str = "__unresolved_callback_id";
impl<'a> Emitter<'a> {
    pub(super) fn record_pattern_events(&mut self, expr: &Expr, pattern: &Pat) {
        if let Some(method) = self.typed_event_method(expr) {
            if let Ok(cases) = typed_patterns(pattern) {
                let unknown = cases.iter().any(|case| case.unknown);
                for case in cases {
                    if let Some(label) = case.label() {
                        self.events.insert(label.to_owned());
                    }
                }
                if unknown {
                    for label in events::known_labels(method) {
                        self.events.insert((*label).to_owned());
                    }
                }
            }
            return;
        }
        match pattern {
            Pat::Lit(literal) => {
                self.record_event(expr, &Expr::Lit(literal.clone()), false)
            }
            Pat::Or(or) => {
                for case in &or.cases {
                    self.record_pattern_events(expr, case);
                }
            }
            Pat::Paren(paren) => self.record_pattern_events(expr, &paren.pat),
            _ => {}
        }
    }

    pub(super) fn record_event(
        &mut self,
        event: &Expr,
        literal: &Expr,
        negated: bool,
    ) {
        if !self.is_event_expr(event) {
            return;
        }
        let label = match literal {
            Expr::Lit(ExprLit {
                lit: Lit::Str(value),
                ..
            }) => Some(value.value()),
            other => self.typed_variant_label(other).map(str::to_owned),
        };
        if let Some(label) = label {
            if negated {
                self.negative_events.insert(label);
            } else {
                self.events.insert(label);
            }
        }
    }

    pub(super) fn is_ctx_expr(&self, expr: &Expr) -> bool {
        let Expr::Path(path) = expr else {
            return false;
        };
        path.path.get_ident().is_some_and(|ident| {
            let name = ident.to_string();
            name == self.ctx || self.lookup(&name) == Some(Ty::CONTEXT)
        })
    }

    pub(super) fn is_event_expr(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Field(field) => {
                self.is_ctx_expr(&field.base)
                    && matches!(&field.member, Member::Named(name) if name == "event")
            }
            Expr::MethodCall(call) => {
                (self.is_event_expr(&call.receiver) && call.method == "as_str")
                    || self.typed_event_method(expr).is_some()
            }
            Expr::Paren(paren) => self.is_event_expr(&paren.expr),
            Expr::Group(group) => self.is_event_expr(&group.expr),
            _ => false,
        }
    }

    pub(super) fn typed_event_method(
        &self,
        expr: &Expr,
    ) -> Option<&'static str> {
        let Expr::MethodCall(call) = expr else {
            return None;
        };
        if !self.is_ctx_expr(&call.receiver) || !call.args.is_empty() {
            return None;
        }
        events::TYPED_EVENT_METHODS
            .iter()
            .copied()
            .find(|name| call.method == *name)
    }

    pub(super) fn typed_variant_label(
        &self,
        expr: &Expr,
    ) -> Option<&'static str> {
        let Expr::Path(path) = expr else {
            return None;
        };
        let name = path.path.segments.last()?.ident.to_string();
        events::variant_label(&name)
    }

    pub(super) fn op_expr(&mut self, expr: &Expr) -> Result<(), Error> {
        self.emitted = true;
        match expr {
            Expr::Struct(item) => {
                let segments = item
                    .path
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect::<Vec<_>>();
                if segments.len() < 2
                    || segments.get(segments.len().saturating_sub(2))
                        != Some(&"Op".to_owned())
                {
                    return Err(Error::at(
                        item.span(),
                        "expected an Op constructor",
                    ));
                }
                let name = segments.last().cloned().unwrap_or_default();
                self.op_struct(&name, item)
            }
            Expr::Call(call) => {
                let name = path_last_name(&call.func).ok_or_else(|| {
                    Error::at(call.span(), "unsupported op expression")
                })?;
                let variant =
                    name.rsplit("::").next().unwrap_or(&name).to_owned();
                if op_constructor(&name).is_some() {
                    return self.op_call(call, &variant);
                }
                if variant == "query" && self.strict() {
                    return self.op_query(call);
                }
                if let Some(message) = unsupported_op(&name) {
                    return Err(Error::at(call.span(), message));
                }
                Err(Error::at(call.span(), "expected an Op constructor"))
            }
            Expr::If(item) => self.op_if(item),
            Expr::Paren(paren) => self.op_expr(&paren.expr),
            Expr::Group(group) => self.op_expr(&group.expr),
            Expr::Path(path) => {
                let segments = path
                    .path
                    .segments
                    .iter()
                    .map(|segment| segment.ident.to_string())
                    .collect::<Vec<_>>();
                let name = if segments.len() >= 2
                    && segments.get(segments.len().saturating_sub(2))
                        == Some(&"Op".to_owned())
                {
                    segments.last().cloned().unwrap_or_default()
                } else {
                    String::new()
                };
                if name == "OpenSettings" {
                    self.line("noctalia.openSettings()");
                    Ok(())
                } else {
                    Err(Error::at(
                        expr.span(),
                        format!("op `{name}` is not supported"),
                    ))
                }
            }
            other => Err(Error::at(other.span(), "expected an Op constructor")),
        }
    }

    /// Emits an `if` whose branches build ops, such as an op chosen per value.
    fn op_if(&mut self, item: &syn::ExprIf) -> Result<(), Error> {
        let condition = match *item.cond.clone() {
            Expr::Let(condition) => self.let_condition(&condition)?,
            other => self.expr(&other)?.0,
        };
        self.line(&format!("if {condition} then"));
        self.indent = self.indent.saturating_add(1);
        self.statement_block(&item.then_branch)?;
        self.indent = self.indent.saturating_sub(1);
        if let Some((_, branch)) = &item.else_branch {
            self.line("else");
            self.indent = self.indent.saturating_add(1);
            match &**branch {
                Expr::Block(block) => self.statement_block(&block.block)?,
                Expr::If(inner) => self.op_if(inner)?,
                other => self.op_expr(other)?,
            }
            self.indent = self.indent.saturating_sub(1);
        }
        self.line("end");
        Ok(())
    }

    fn op_struct(
        &mut self,
        name: &str,
        item: &syn::ExprStruct,
    ) -> Result<(), Error> {
        let mut fields = HashMap::new();
        for field in &item.fields {
            let Member::Named(member) = &field.member else {
                return Err(Error::at(
                    field.member.span(),
                    "unsupported op field",
                ));
            };
            fields.insert(member.to_string(), &field.expr);
        }
        let field = |name: &str| -> Result<&Expr, Error> {
            fields.get(name).copied().ok_or_else(|| {
                Error::at(
                    item.span(),
                    format!("op `{name}` is missing its `{name}` field"),
                )
            })
        };
        match name {
            "Log" => {
                let message = self.expr(field("message")?)?.0;
                self.line(&format!("noctalia.log({message})"));
            }
            "Notify" | "NotifyError" => {
                let title = self.expr(field("title")?)?.0;
                let body = self.expr(field("body")?)?.0;
                let options = match fields.get("options") {
                    Some(expr) => self.expr(expr)?.0,
                    None => "nil".to_owned(),
                };
                let function = if name == "Notify" {
                    "noctalia.notify"
                } else {
                    "noctalia.notifyError"
                };
                self.line(&format!("{function}({title}, {body}, {options})"));
            }
            "SetInterval" => {
                let ms = self.expr(field("ms")?)?.0;
                self.line(&format!("noctalia.setUpdateInterval({ms})"));
            }
            "StateSet" => {
                let key = self.expr(field("key")?)?.0;
                let value = self.expr(field("value")?)?.0;
                self.line(&format!("state.set({key}, {value})"));
            }
            "TogglePanel" => {
                let id = self.expr(field("id")?)?.0;
                self.line(&format!("noctalia.togglePanel({id})"));
            }
            "OpenSettings" => self.line("noctalia.openSettings()"),
            "BarText" => {
                let text = self.expr(field("text")?)?.0;
                self.line(&format!("barWidget.setText({text})"));
            }
            "BarColor" => {
                let role = self.expr(field("role")?)?.0;
                self.line(&format!("barWidget.setColor({role})"));
            }
            "BarVisible" => {
                let visible = self.expr(field("visible")?)?.0;
                self.line(&format!("barWidget.setVisible({visible})"));
            }
            "BarTooltip" => {
                let value = self.expr(field("value")?)?.0;
                self.line(&format!("barWidget.setTooltip({value})"));
            }
            "BarImage" => {
                let path = self.expr(field("path")?)?.0;
                let preserve = self.expr(field("preserve")?)?.0;
                let width = self.expr(field("width")?)?.0;
                let height = self.expr(field("height")?)?.0;
                self.line(&format!(
                    "barWidget.setImage({path}, {preserve}, {width}, {height})"
                ));
            }
            "BarRender" => {
                let tree = self.expr(field("tree")?)?.0;
                self.line(&format!("barWidget.render({tree})"));
            }
            "PanelRender" => {
                let tree = self.expr(field("tree")?)?.0;
                self.line(&format!("panel.render({tree})"));
            }
            "DesktopWidgetRender" => {
                let tree = self.expr(field("tree")?)?.0;
                self.line(&format!("desktopWidget.render({tree})"));
            }
            "LayerShow" => {
                let id = self.expr(field("id")?)?.0;
                self.line(&format!("noctalia.layer.show({id})"));
            }
            "LayerHide" => {
                let id = self.expr(field("id")?)?.0;
                self.line(&format!("noctalia.layer.hide({id})"));
            }
            "LayerRender" => {
                let tree = self.expr(field("tree")?)?.0;
                self.line(&format!("layer.render({tree})"));
            }
            "LayerConfigure" => {
                let id = self.expr(field("id")?)?.0;
                let props = self.expr(field("props")?)?.0;
                self.line(&format!("noctalia.layer.configure({id}, {props})"));
            }
            "LauncherResults" => {
                let text = self.expr(field("text")?)?.0;
                let rows = self.expr(field("rows")?)?.0;
                self.line(&format!("launcher.setResults({text}, {rows})"));
            }
            "RemoveFile" => {
                let path = self.expr(field("path")?)?.0;
                self.line(&format!("noctalia.removeFile({path})"));
            }
            "HostCall" => {
                self.uses_host_call = true;
                let root = self.expr(field("root")?)?.0;
                let path = self.expr(field("path")?)?.0;
                let args = self.expr(field("args")?)?.0;
                self.line(&format!("host_call({root}, {path}, {args})"));
            }
            other => {
                return Err(Error::at(
                    item.span(),
                    format!(
                        "op `{other}` is not supported by the Luau compiler"
                    ),
                ));
            }
        }
        Ok(())
    }

    /// The host-callback marker for a callback-id argument, or `None` when the
    /// argument is `None`. Compiled Luau names its callbacks, so the runtime
    /// hands the payload back to the entry's `onHostCallback`.
    pub(super) fn callback_marker(
        &mut self,
        args: &[&Expr],
        index: usize,
    ) -> Result<Option<String>, Error> {
        let Some(argument) = args.get(index) else {
            return Ok(None);
        };
        let argument = strip_reference(argument);
        if let Expr::Path(path) = argument
            && path
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "None")
        {
            return Ok(None);
        }
        let inner = match argument {
            Expr::Call(inner)
                if path_last_name(&inner.func).as_deref() == Some("Some") =>
            {
                arg(&inner.args.iter().collect::<Vec<_>>(), 0, inner)?
            }
            other => other,
        };
        if self.strict() {
            return Ok(Some(self.strict_callback(inner, inner.span())?));
        }
        let text = if let Some(id) =
            self.typed_callback_id_checked(inner, inner.span())?
        {
            crate::quote(&id)
        } else {
            self.expr(inner)?.0
        };
        Ok(Some(format!("{{ __host_callback = {text} }}")))
    }

    /// The raw callback id for a strict handle helper, or `None` when the
    /// argument is `None`. The helper captures the id once and wraps it in a
    /// closure, so the host still sees a real API 57 function.
    pub(super) fn strict_callback_id(
        &mut self,
        args: &[&Expr],
        index: usize,
    ) -> Result<Option<String>, Error> {
        let Some(argument) = args.get(index) else {
            return Ok(None);
        };
        let argument = strip_reference(argument);
        if let Expr::Path(path) = argument
            && path
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "None")
        {
            return Ok(None);
        }
        let inner = match argument {
            Expr::Call(inner)
                if path_last_name(&inner.func).as_deref() == Some("Some") =>
            {
                arg(&inner.args.iter().collect::<Vec<_>>(), 0, inner)?
            }
            other => other,
        };
        if !self.has_host_callback {
            return Err(Error::at(
                inner.span(),
                "a handle callback needs a host_callback branch in strict mode",
            ));
        }
        Ok(Some(self.expr(inner)?.0))
    }
    /// Records a notify action global that the host will call by name.
    fn notify_action(&mut self, options: &Expr) -> Result<(), Error> {
        let Some(name) = json_action_name(options) else {
            return Ok(());
        };
        if !self.has_action {
            return Err(Error::at(
                options.span(),
                "a notify action needs an action branch in strict mode",
            ));
        }
        if !is_identifier(&name) || is_luau_keyword(&name) {
            return Err(Error::at(
                options.span(),
                format!(
                    "notify action `{name}` is not a valid Luau global name"
                ),
            ));
        }
        let reserved = name == "update"
            || name == "onHostCallback"
            || name.starts_with("__strict_")
            || crate::CALLBACKS
                .iter()
                .any(|callback| callback.function == name);
        if reserved {
            return Err(Error::at(
                options.span(),
                format!(
                    "notify action `{name}` collides with a generated handler global"
                ),
            ));
        }
        self.notify_actions.insert(name);
        Ok(())
    }

    /// The strict callback value: a real closure for a callback id.
    pub(super) fn strict_callback(
        &mut self,
        inner: &Expr,
        span: proc_macro2::Span,
    ) -> Result<String, Error> {
        if !self.has_host_callback {
            return Err(Error::at(
                span,
                "a callback needs a host_callback branch in strict mode",
            ));
        }
        if is_ui_marker_call(inner, "callback") {
            return Ok(self.expr(inner)?.0);
        }
        let id = if let Some(id) =
            self.typed_callback_id_checked(inner, inner.span())?
        {
            crate::quote(&id)
        } else {
            self.expr(inner)?.0
        };
        self.uses_callback_dispatch = true;
        Ok(strict::closure(strict::CALLBACK_DISPATCH, &id))
    }

    /// The stable id of a typed callback, at the call site or in a local.
    pub(super) fn typed_callback_id(&self, expr: &Expr) -> Option<String> {
        let expr = strip_reference(expr);
        if let Some(id) = typed_handle_expr(
            expr,
            TypedHandle::Callback,
            self.program,
            &self.module,
        ) {
            return Some(id);
        }
        match expr {
            Expr::Path(path) => {
                let name = path.path.get_ident()?.to_string();
                self.callback_ids
                    .iter()
                    .rev()
                    .find_map(|scope| scope.get(&name).cloned())
            }
            Expr::Block(block) => {
                block
                    .block
                    .stmts
                    .last()
                    .and_then(|statement| match statement {
                        Stmt::Expr(expr, _) => self.typed_callback_id(expr),
                        _ => None,
                    })
            }
            Expr::If(item) => {
                let then =
                    item.then_branch.stmts.last().and_then(|statement| {
                        match statement {
                            Stmt::Expr(expr, _) => self.typed_callback_id(expr),
                            _ => None,
                        }
                    });
                let otherwise =
                    item.else_branch.as_ref().and_then(|(_, branch)| {
                        match &**branch {
                            Expr::Block(block) => {
                                block.block.stmts.last().and_then(|statement| {
                                    match statement {
                                        Stmt::Expr(expr, _) => {
                                            self.typed_callback_id(expr)
                                        }
                                        _ => None,
                                    }
                                })
                            }
                            other => self.typed_callback_id(other),
                        }
                    });
                match (then, otherwise) {
                    (Some(left), Some(right)) if left == right => Some(left),
                    _ => Some(UNRESOLVED_CALLBACK_ID.to_owned()),
                }
            }
            _ => None,
        }
    }

    /// Updates the scope that owns a callback binding.
    ///
    /// A reassignment cannot keep a compile-time id, because a later branch
    /// may take a different path through the assignment.
    pub(super) fn set_callback_id(&mut self, name: &str, id: Option<String>) {
        for scope in self.callback_ids.iter_mut().rev() {
            if scope.contains_key(name) {
                match id {
                    Some(id) => {
                        scope.insert(name.to_owned(), id);
                    }
                    None => {
                        scope.remove(name);
                    }
                }
                return;
            }
        }
        if let (Some(id), Some(scope)) = (id, self.callback_ids.last_mut()) {
            scope.insert(name.to_owned(), id);
        }
    }

    /// Reads a typed callback id, failing closed when one branch or a later
    /// assignment left it unknown.
    pub(super) fn typed_callback_id_checked(
        &self,
        expr: &Expr,
        span: proc_macro2::Span,
    ) -> Result<Option<String>, Error> {
        let Some(id) = self.typed_callback_id(expr) else {
            return Ok(None);
        };
        if id == UNRESOLVED_CALLBACK_ID {
            return Err(Error::at(
                span,
                "a typed callback id must resolve to one id at compile time",
            ));
        }
        Ok(Some(id))
    }

    /// A strict `Op::query`: the direct member call plus the response envelope.
    pub(super) fn op_query(&mut self, call: &ExprCall) -> Result<(), Error> {
        let args = call.args.iter().collect::<Vec<_>>();
        let root_expr = arg(&args, 1, call)?;
        let Some(root) = literal_string(root_expr) else {
            return Err(Error::at(
                root_expr.span(),
                "strict host resolution needs a literal root",
            ));
        };
        let path_expr = arg(&args, 2, call)?;
        let path = literal_path(path_expr)?;
        let values = self.expr(arg(&args, 3, call)?)?.0;
        let response = self.expr(arg(&args, 4, call)?)?.0;
        let full = strict::member_path(&root, &path);
        let Some(kind) = strict::member(&root, &path) else {
            return Err(Error::at(
                path_expr.span(),
                format!("unknown host member `{full}`"),
            ));
        };
        if matches!(
            kind,
            strict::MemberKind::Namespace
                | strict::MemberKind::Node
                | strict::MemberKind::Void
        ) {
            return Err(Error::at(
                path_expr.span(),
                format!("`{full}` cannot answer a query"),
            ));
        }
        let ok = self.temp();
        let value = self.temp();
        let error = self.temp();
        let envelope = self.temp();
        self.uses_unpack = true;
        self.uses_pcall = true;
        self.line(&format!(
            "local {ok}, {value}, {error} = __pcall(function() return {full}(__unpack({values})) end)"
        ));
        self.line(&format!("local {envelope}"));
        self.line(&format!(
            "if {ok} then {envelope} = {{ ok = true, value = {value}, error = {error} }} else {envelope} = {{ ok = false, error = tostring({value}) }} end"
        ));
        self.line(&format!(
            "noctalia.writeFile({response}, noctalia.json.encode({envelope}))"
        ));
        Ok(())
    }

    /// The value inside a `Some(...)` argument, or `nil` for `None`.
    pub(super) fn option_value(
        &mut self,
        argument: &Expr,
    ) -> Result<String, Error> {
        let argument = strip_reference(argument);
        if let Expr::Path(path) = argument
            && path
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "None")
        {
            return Ok("nil".to_owned());
        }
        let inner = match argument {
            Expr::Call(inner)
                if path_last_name(&inner.func).as_deref() == Some("Some") =>
            {
                arg(&inner.args.iter().collect::<Vec<_>>(), 0, inner)?
            }
            other => other,
        };
        Ok(self.expr(inner)?.0)
    }

    pub(super) fn op_call(
        &mut self,
        call: &ExprCall,
        name: &str,
    ) -> Result<(), Error> {
        let args = call.args.iter().collect::<Vec<_>>();
        match name {
            "log" | "log_static" => {
                let message = self.expr(arg(&args, 0, call)?)?.0;
                self.line(&format!("noctalia.log({message})"));
            }
            "state" => {
                let key = self.expr(arg(&args, 0, call)?)?.0;
                let value = self.expr(arg(&args, 1, call)?)?.0;
                self.line(&format!("state.set({key}, {value})"));
            }
            "state_watch" | "state_watch_callback" => {
                let key = self.expr(arg(&args, 0, call)?)?.0;
                let id_expr = arg(&args, 1, call)?;
                if self.strict() {
                    let callback =
                        self.strict_callback(id_expr, id_expr.span())?;
                    self.line(&format!("state.watch({key}, {callback})"));
                } else {
                    let id = if let Some(id) =
                        self.typed_callback_id_checked(id_expr, id_expr.span())?
                    {
                        crate::quote(&id)
                    } else {
                        self.expr(id_expr)?.0
                    };
                    self.line(&format!(
                        "state.watch({key}, {{ __host_callback = {id} }})"
                    ));
                }
            }
            "run" | "run_callback" => {
                let command = self.expr(arg(&args, 0, call)?)?.0;
                let callback = self.callback_marker(&args, 1)?;
                let line = match callback {
                    Some(marker) => {
                        format!("noctalia.runAsync({command}, {marker})")
                    }
                    None => format!("noctalia.runAsync({command})"),
                };
                self.line(&line);
            }
            "run_with_timeout" | "run_callback_with_timeout" => {
                let command = self.expr(arg(&args, 0, call)?)?.0;
                let callback = self
                    .callback_marker(&args, 1)?
                    .unwrap_or_else(|| "nil".to_owned());
                let timeout = match args.get(2) {
                    Some(timeout) => self.expr(strip_reference(timeout))?.0,
                    None => "nil".to_owned(),
                };
                self.line(&format!(
                    "noctalia.runAsync({command}, {callback}, {timeout})"
                ));
            }
            "run_stream" | "run_stream_callback" => {
                let command = self.expr(arg(&args, 0, call)?)?.0;
                let callback =
                    self.callback_marker(&args, 1)?.ok_or_else(|| {
                        Error::at(
                            call.span(),
                            "op `run_stream` needs a callback id",
                        )
                    })?;
                self.line(&format!(
                    "noctalia.runStream({command}, {callback})"
                ));
            }
            "http" | "http_callback" => {
                let request = self.expr(arg(&args, 0, call)?)?.0;
                let callback = self.callback_marker(&args, 1)?;
                let line = match callback {
                    Some(marker) => {
                        format!("noctalia.http({request}, {marker})")
                    }
                    None => format!("noctalia.http({request})"),
                };
                self.line(&line);
            }
            "set_interval" => {
                let ms = self.expr(arg(&args, 0, call)?)?.0;
                self.line(&format!("noctalia.setUpdateInterval({ms})"));
            }
            "host_call" => {
                self.uses_host_call = true;
                let root = self.expr(arg(&args, 0, call)?)?.0;
                let path = self.expr(arg(&args, 1, call)?)?.0;
                let value = self.expr(arg(&args, 2, call)?)?.0;
                self.line(&format!("host_call({root}, {path}, {value})"));
            }
            "notify" | "notify_with_options" => {
                let title = self.expr(arg(&args, 0, call)?)?.0;
                let body = self.expr(arg(&args, 1, call)?)?.0;
                let options = if name == "notify_with_options" {
                    let option_expr = arg(&args, 2, call)?;
                    if self.strict() {
                        self.notify_action(option_expr)?;
                    }
                    self.expr(option_expr)?.0
                } else {
                    "nil".to_owned()
                };
                self.line(&format!(
                    "noctalia.notify({title}, {body}, {options})"
                ));
            }
            "notify_error" | "notify_error_with_options" => {
                let title = self.expr(arg(&args, 0, call)?)?.0;
                let body = self.expr(arg(&args, 1, call)?)?.0;
                let options = if name == "notify_error_with_options" {
                    let option_expr = arg(&args, 2, call)?;
                    if self.strict() {
                        self.notify_action(option_expr)?;
                    }
                    self.expr(option_expr)?.0
                } else {
                    "nil".to_owned()
                };
                self.line(&format!(
                    "noctalia.notifyError({title}, {body}, {options})"
                ));
            }
            "copy" => {
                let text = self.expr(arg(&args, 0, call)?)?.0;
                self.line(&format!("noctalia.copyToClipboard({text})"));
            }
            "copy_with_mime" => {
                let text = self.expr(arg(&args, 0, call)?)?.0;
                let mime = self.expr(arg(&args, 1, call)?)?.0;
                self.line(&format!("noctalia.copyToClipboard({text}, {mime})"));
            }
            "open_path" => {
                let path = self.expr(arg(&args, 0, call)?)?.0;
                self.line(&format!("noctalia.openPath({path})"));
            }
            "query" => {
                if self.strict() {
                    self.op_query(call)?;
                } else {
                    return Err(Error::at(
                        call.span(),
                        "op `query` needs a response path, which compiled Luau cannot allocate",
                    ));
                }
            }
            "copy_bytes" if self.strict() => {
                let bytes = self.expr(arg(&args, 0, call)?)?.0;
                let mime = self.expr(arg(&args, 1, call)?)?.0;
                self.line(&format!(
                    "noctalia.copyToClipboard({bytes}, {mime})"
                ));
            }
            "write_file_bytes" if self.strict() => {
                let path = self.expr(arg(&args, 0, call)?)?.0;
                let bytes = self.expr(arg(&args, 1, call)?)?.0;
                self.line(&format!("noctalia.writeFile({path}, {bytes})"));
            }
            "http_bytes" if self.strict() => {
                let request = self.expr(arg(&args, 0, call)?)?.0;
                let bytes = self.expr(arg(&args, 1, call)?)?.0;
                let callback = match args.get(2) {
                    Some(callback) => {
                        self.strict_callback(callback, callback.span())?
                    }
                    None => "nil".to_owned(),
                };
                self.line(&format!(
                    "noctalia.http((function(__r) __r.body = {bytes}; return __r end)({request}), {callback})"
                ));
            }
            other => {
                return Err(Error::at(
                    call.span(),
                    format!("op constructor `{other}` is not supported"),
                ));
            }
        }
        Ok(())
    }
}

pub(super) fn qualified_variant<'a>(
    name: &'a str,
    root: &str,
) -> Option<&'a str> {
    let mut parts = name.rsplit("::");
    let variant = parts.next()?;
    (parts.next()? == root).then_some(variant)
}

pub(super) fn unsupported_op(name: &str) -> Option<String> {
    let variant = qualified_variant(name, "Op")?;
    let message = match variant {
        "http_bytes" => {
            "op `http_bytes` sends a raw byte body, which compiled Luau cannot build"
        }
        "query" => {
            "op `query` needs a response path, which compiled Luau cannot allocate"
        }
        _ => return None,
    };
    Some(message.to_owned())
}

pub(super) fn op_constructor(name: &str) -> Option<String> {
    let variant = qualified_variant(name, "Op")?;
    let supported = [
        "log",
        "log_static",
        "state",
        "state_watch",
        "set_interval",
        "host_call",
        "notify",
        "notify_with_options",
        "notify_error",
        "notify_error_with_options",
        "copy",
        "copy_with_mime",
        "open_path",
        "run",
        "run_with_timeout",
        "run_stream",
        "http",
        "copy_bytes",
        "write_file_bytes",
        "http_bytes",
        "query",
        "state_watch_callback",
        "run_callback",
        "run_callback_with_timeout",
        "run_stream_callback",
        "http_callback",
    ];
    supported.contains(&variant).then(|| variant.to_owned())
}

pub(super) fn is_ops_push(emitter: &Emitter<'_>, call: &ExprCall) -> bool {
    let Expr::MethodCall(method) = &*call.func else {
        return false;
    };
    if method.method != "push" || call.args.len() != 1 {
        return false;
    }
    call.args.first().is_some_and(is_op_expr)
        && path_ident(&method.receiver)
            .and_then(|name| emitter.lookup(&name))
            .is_some_and(|ty| ty.base == Base::Ops)
}

pub(super) fn is_op_expr(expr: &Expr) -> bool {
    match expr {
        Expr::Struct(item) => {
            let segments = item.path.segments.iter().collect::<Vec<_>>();
            segments.len() >= 2
                && segments
                    .get(segments.len().saturating_sub(2))
                    .is_some_and(|segment| segment.ident == "Op")
        }
        Expr::Call(call) => path_last_name(&call.func)
            .is_some_and(|name| op_constructor(&name).is_some()),
        Expr::Path(path) => path_last_name(&Expr::Path(path.clone()))
            .is_some_and(|name| name == "Op::OpenSettings"),
        Expr::If(item) => {
            block_tail_is_op(&item.then_branch)
                || item.else_branch.as_ref().is_some_and(|(_, branch)| {
                    match &**branch {
                        Expr::Block(block) => block_tail_is_op(&block.block),
                        other => is_op_expr(other),
                    }
                })
        }
        _ => false,
    }
}

fn block_tail_is_op(block: &syn::Block) -> bool {
    matches!(block.stmts.last(), Some(Stmt::Expr(expr, _)) if is_op_expr(expr))
}

/// Whether `expr` is a `ui::<variant>(...)` call.
pub(super) fn is_ui_marker_call(expr: &Expr, variant: &str) -> bool {
    let Expr::Call(call) = strip_reference(expr) else {
        return false;
    };
    path_last_name(&call.func).is_some_and(|name| {
        qualified_variant(&name, "ui").is_some_and(|found| found == variant)
    })
}

/// A host path made only of string literals.
pub(super) fn literal_path(expr: &Expr) -> Result<Vec<String>, Error> {
    let expr = strip_reference(expr);
    let Expr::Array(array) = expr else {
        return Err(Error::at(
            expr.span(),
            "strict host resolution needs a literal path",
        ));
    };
    let mut result = Vec::new();
    for element in &array.elems {
        let Some(value) = literal_string(element) else {
            return Err(Error::at(
                element.span(),
                "strict host resolution needs a literal path",
            ));
        };
        result.push(value);
    }
    Ok(result)
}

/// Which typed handle a constructor builds.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum TypedHandle {
    Callback,
    Action,
}

/// The id of a typed handle constructor.
///
/// `Callback::<Row>::new()`, `callback::<Row>()`, `Callback::named("row")`
/// and their `Action` counterparts are recognized. The id comes from the
/// outer type name, which is the same spelling the library reads with
/// `type_name::<T>()`.
pub(super) fn typed_handle_call(
    call: &syn::ExprCall,
    kind: TypedHandle,
    program: &Program,
    module: &str,
) -> Option<String> {
    let Expr::Path(path) = &*call.func else {
        return None;
    };
    let segments = path.path.segments.iter().collect::<Vec<_>>();
    let last = segments.last()?.ident.to_string();
    let root = if kind == TypedHandle::Callback {
        "Callback"
    } else {
        "Action"
    };
    if last == "named" {
        if segments.get(segments.len().saturating_sub(2))?.ident != root {
            return None;
        }
        let argument =
            arg(&call.args.iter().collect::<Vec<_>>(), 0, call).ok()?;
        let Expr::Lit(ExprLit {
            lit: Lit::Str(value),
            ..
        }) = strip_reference(argument)
        else {
            return None;
        };
        return Some(value.value());
    }
    let (name, segment) = if last == "new" {
        let name = segments.get(segments.len().saturating_sub(2))?;
        (name, name)
    } else {
        (segments.last()?, segments.last()?)
    };
    let matches = name.ident == root
        || (kind == TypedHandle::Callback && name.ident == "callback")
        || (kind == TypedHandle::Action && name.ident == "action");
    if !matches {
        return None;
    }
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let syn::GenericArgument::Type(ty) = arguments.args.first()? else {
        return None;
    };
    let id = type_ident(ty)?;
    if !is_known_id(&id) && !program.has_type(module, &id) {
        return None;
    }
    Some(id)
}

/// The id of a typed handle expression.
pub(super) fn typed_handle_expr(
    expr: &Expr,
    kind: TypedHandle,
    program: &Program,
    module: &str,
) -> Option<String> {
    let Expr::Call(call) = strip_reference(expr) else {
        return None;
    };
    typed_handle_call(call, kind, program, module)
}

fn type_ident(ty: &syn::Type) -> Option<String> {
    match ty {
        syn::Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        syn::Type::Reference(reference) => {
            type_ident(&reference.elem).map(|id| format!("&{id}"))
        }
        syn::Type::Paren(paren) => type_ident(&paren.elem),
        syn::Type::Group(group) => type_ident(&group.elem),
        syn::Type::Tuple(_) => None,
        _ => None,
    }
}

/// Type names whose `type_name` tail is the same as their source spelling.
fn is_known_id(id: &str) -> bool {
    matches!(
        id,
        "Value"
            | "String"
            | "str"
            | "&str"
            | "bool"
            | "char"
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
            | "Option"
            | "Result"
            | "Vec"
            | "Box"
            | "Cow"
    )
}
