//! Typed UI props and handlers.
//!
//! A strict entry builds node props with `Props::new()` and its typed methods.
//! The emitter writes a plain Luau table for those props and passes it to the
//! API 57 `ui.<kind>` constructor, so no generic `json!` map and no bridge
//! marker reaches the host. Auto and off keep their existing marker tables.

use super::*;

type PropValue = Option<(String, Ty)>;

/// Which constructor a typed handler value comes from.
#[derive(Clone, Copy)]
enum HandlerCtor {
    /// `Action::new(name)` or `Action::dynamic(name)`.
    Action,
    /// `CallbackId::new(id)`.
    Callback,
    /// A plain string passed to `Props::call`.
    None,
}

impl HandlerCtor {
    fn matches(self, name: &str) -> bool {
        match self {
            Self::Action => {
                name.ends_with("Action::new")
                    || name.ends_with("Action::dynamic")
            }
            Self::Callback => name.ends_with("CallbackId::new"),
            Self::None => false,
        }
    }
}

impl<'a> Emitter<'a> {
    /// Starts a typed property list. Each `Props` value owns its table, so a
    /// binding keeps its identity when a node reuses it.
    pub(super) fn props_new(&mut self) -> Result<(String, Ty), Error> {
        let props = self.temp();
        self.line(&format!("local {props} = {{}}"));
        Ok((props, Ty::PROPS))
    }

    /// Lowers one `Props` builder method, if the receiver is a typed prop list.
    pub(super) fn props_method(
        &mut self,
        call: &ExprMethodCall,
    ) -> Result<PropValue, Error> {
        if !self.is_props_expr(&call.receiver) {
            return Ok(None);
        }

        if call.method == "clone" {
            let (props, _) = self.expr(&call.receiver)?;
            let copy = self.temp();
            self.line(&format!("local {copy} = {{}}"));
            self.line(&format!(
                "for __key, __value in pairs({props}) do {copy}[__key] = __value end"
            ));
            return Ok(Some((copy, Ty::PROPS)));
        }
        let method = call.method.to_string();
        let kind = match method.as_str() {
            "text" => PropKind::Text,
            "int" | "num" => PropKind::Number,
            "bool" => PropKind::Bool,
            "call" => PropKind::Handler(HandlerCtor::None),
            "action" => PropKind::Handler(HandlerCtor::Action),
            "callback" => PropKind::Handler(HandlerCtor::Callback),
            "json" => PropKind::Json,
            _ => {
                return Err(Error::at(
                    call.method.span(),
                    format!("method `{method}` is not supported on Props"),
                ));
            }
        };

        let args = call.args.iter().collect::<Vec<_>>();
        let key_expr = arg(&args, 0, call)?;
        let Some(key) = literal_string(key_expr) else {
            return Err(Error::at(
                key_expr.span(),
                "a Props key must be a string literal",
            ));
        };
        let value_expr = arg(&args, 1, call)?;

        let (props, ty) = self.expr(&call.receiver)?;
        if ty.base != Base::Props {
            return Err(Error::at(
                call.receiver.span(),
                "a Props method needs a Props receiver",
            ));
        }

        let value = match kind {
            PropKind::Text => self.prop_value(
                value_expr,
                &[Base::Str, Base::Path],
                "a text prop",
            )?,
            PropKind::Number => self.prop_value(
                value_expr,
                &[Base::Int, Base::Float],
                "a numeric prop",
            )?,
            PropKind::Bool => {
                self.prop_value(value_expr, &[Base::Bool], "a boolean prop")?
            }
            PropKind::Json if self.strict() => {
                return Err(Error::at(
                    value_expr.span(),
                    "json values in props are not supported in strict mode: use text, int, num, bool or a typed handler",
                ));
            }
            PropKind::Json => self.expr(value_expr)?.0,
            PropKind::Handler(ctor) => {
                self.handler_value(value_expr, ctor, call.span())?
            }
        };

        self.line(&format!("{props}.{} = {value}", props_key(&key)));
        Ok(Some((props, Ty::PROPS)))
    }

    /// The strict value for a typed action. A literal name gets its own
    /// generated global; a dynamic name uses the action dispatcher closure.
    pub(super) fn strict_action_value(
        &mut self,
        expr: &Expr,
        span: proc_macro2::Span,
    ) -> Result<String, Error> {
        if !self.has_action {
            return Err(Error::at(
                span,
                "an action handler needs an action branch in strict mode",
            ));
        }
        let inner = handler_arg(expr, HandlerCtor::Action);
        if let Some(name) = literal_string(inner) {
            self.actions.insert(name.clone());
            return Ok(quote(&strict::action_global(&name)));
        }
        let (name, ty) = self.expr(inner)?;
        reject_null(
            &name,
            ty,
            inner.span(),
            "an action handler name",
            self.strict(),
        )?;
        self.uses_action_dispatch = true;
        Ok(strict::closure(strict::ACTION_DISPATCH, &name))
    }

    /// The strict value for a typed host callback handle.
    pub(super) fn strict_callback_value(
        &mut self,
        expr: &Expr,
        span: proc_macro2::Span,
    ) -> Result<String, Error> {
        if !self.has_host_callback {
            return Err(Error::at(
                span,
                "a callback handler needs a host_callback branch in strict mode",
            ));
        }
        let inner = handler_arg(expr, HandlerCtor::Callback);
        let (id, ty) = self.expr(inner)?;
        reject_null(
            &id,
            ty,
            inner.span(),
            "a callback handler id",
            self.strict(),
        )?;
        self.uses_callback_dispatch = true;
        Ok(strict::closure(strict::CALLBACK_DISPATCH, &id))
    }

    /// Emits one node. Strict calls the API 57 constructor directly with the
    /// typed prop table; other modes keep the internal `{ kind, props, ... }`
    /// table the bridge and runtime converter already read.
    pub(super) fn ui_node_call(
        &mut self,
        kind: &str,
        props: (String, Ty),
        children: String,
        span: proc_macro2::Span,
    ) -> Result<(String, Ty), Error> {
        let (props, ty) = props;
        if !self.strict() {
            return Ok((
                format!(
                    "{{ kind = {}, props = {props}, children = {children} }}",
                    quote(kind)
                ),
                Ty::NODE,
            ));
        }
        if strict::node_member(kind).is_none() {
            return Err(Error::at(
                span,
                format!("unknown strict UI node kind `{kind}`"),
            ));
        }
        if ty.base != Base::Props {
            return Err(Error::at(
                span,
                "strict UI props must come from Props::new(); a generic json! prop map has no typed node shape",
            ));
        }
        if !ui_children(kind) && children != "{}" && children != empty_array() {
            return Err(Error::at(
                span,
                format!("strict UI node `{kind}` cannot take children"),
            ));
        }
        let arguments = if ui_children(kind) {
            format!("{props}, {children}")
        } else {
            props
        };
        Ok((format!("ui.{kind}({arguments})"), Ty::NODE))
    }

    /// Emits a node whose kind is computed, which strict cannot prove against the
    /// API 57 constructor table.
    pub(super) fn dynamic_ui_node(
        &mut self,
        kind_expr: &Expr,
        kind: String,
        props: (String, Ty),
        children: String,
        span: proc_macro2::Span,
    ) -> Result<(String, Ty), Error> {
        if let Some(literal) = literal_string(kind_expr) {
            return self.ui_node_call(&literal, props, children, span);
        }
        if !self.strict() {
            let (props, _) = props;
            return Ok((
                format!(
                    "{{ kind = {kind}, props = {props}, children = {children} }}"
                ),
                Ty::NODE,
            ));
        }
        Err(Error::at(
            kind_expr.span(),
            "strict UI node kinds must be a literal API 57 constructor name",
        ))
    }

    fn is_props_expr(&self, expr: &Expr) -> bool {
        match strip_reference(expr) {
            Expr::Call(call) => path_last_name(&call.func)
                .is_some_and(|name| name.ends_with("Props::new")),
            Expr::MethodCall(call) => {
                matches!(
                    call.method.to_string().as_str(),
                    "text"
                        | "int"
                        | "num"
                        | "bool"
                        | "call"
                        | "action"
                        | "callback"
                        | "json"
                        | "clone"
                ) && self.is_props_expr(&call.receiver)
            }
            Expr::Path(path) => path
                .path
                .get_ident()
                .and_then(|name| self.lookup(&name.to_string()))
                .is_some_and(|ty| ty.base == Base::Props),
            _ => false,
        }
    }

    fn prop_value(
        &mut self,
        expr: &Expr,
        allowed: &[Base],
        role: &str,
    ) -> Result<String, Error> {
        let (value, ty) = self.expr(expr)?;
        if ty.base != Base::Value && !allowed.contains(&ty.base) {
            return Err(Error::at(
                expr.span(),
                format!("{role} has the wrong value type"),
            ));
        }
        reject_null(&value, ty, expr.span(), role, self.strict())?;
        Ok(value)
    }

    fn handler_value(
        &mut self,
        expr: &Expr,
        ctor: HandlerCtor,
        span: proc_macro2::Span,
    ) -> Result<String, Error> {
        if self.strict() {
            return match ctor {
                HandlerCtor::Action | HandlerCtor::None => {
                    self.strict_action_value(expr, span)
                }
                HandlerCtor::Callback => self.strict_callback_value(expr, span),
            };
        }
        let inner = handler_arg(expr, ctor);
        let (value, ty) = self.expr(inner)?;
        reject_null(&value, ty, expr.span(), "a prop handler", self.strict())?;
        let marker = match ctor {
            HandlerCtor::Callback => "__host_callback",
            HandlerCtor::Action | HandlerCtor::None => "__call",
        };
        Ok(format!("{{ {marker} = {value} }}"))
    }
}

#[derive(Clone, Copy)]
enum PropKind {
    Text,
    Number,
    Bool,
    Json,
    Handler(HandlerCtor),
}

/// The newtype constructor around a typed handler name, when there is one.
fn handler_arg(expr: &Expr, ctor: HandlerCtor) -> &Expr {
    let expr = strip_reference(expr);
    if let Expr::Call(call) = expr
        && path_last_name(&call.func).is_some_and(|name| ctor.matches(&name))
        && let Some(first) = call.args.first()
    {
        strip_reference(first)
    } else {
        expr
    }
}

/// Only these API 57 constructors take a child list.
fn ui_children(kind: &str) -> bool {
    matches!(
        kind,
        "column" | "row" | "scroll" | "dragSource" | "dropZone"
    )
}

/// `lua_key` keeps any identifier, including Luau keywords such as `end`.
/// Bracket those so generated prop tables still parse.
fn props_key(key: &str) -> String {
    if is_luau_keyword(key) {
        format!("[{}]", quote(key))
    } else {
        lua_key(key)
    }
}
