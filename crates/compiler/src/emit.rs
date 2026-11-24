use std::collections::{BTreeMap, BTreeSet, HashMap};

use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    BinOp, Expr, ExprBinary, ExprCall, ExprLit, ExprMacro, ExprMethodCall,
    ItemFn, Lit, Member, Pat, PathArguments, Stmt, Token, Type, UnOp,
};

use crate::program::{Program, typed_args};
use crate::{Error, is_context, is_identifier, quote, sanitize};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Base {
    Str,
    Int,
    Float,
    Bool,
    Value,
    List,
    Node,
    Table,
    Ops,
    Context,
    Void,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Ty {
    base: Base,
    optional: bool,
    result: bool,
}

impl Ty {
    const fn new(base: Base) -> Self {
        Self {
            base,
            optional: false,
            result: false,
        }
    }

    const fn opt(base: Base) -> Self {
        Self {
            base,
            optional: true,
            result: false,
        }
    }

    const fn result(base: Base) -> Self {
        Self {
            base,
            optional: false,
            result: true,
        }
    }

    const STR: Self = Self::new(Base::Str);
    const OPT_STR: Self = Self::opt(Base::Str);
    const INT: Self = Self::new(Base::Int);
    const OPT_INT: Self = Self::opt(Base::Int);
    const FLOAT: Self = Self::new(Base::Float);
    const OPT_FLOAT: Self = Self::opt(Base::Float);
    const BOOL: Self = Self::new(Base::Bool);
    const OPT_BOOL: Self = Self::opt(Base::Bool);
    const VALUE: Self = Self::new(Base::Value);
    const OPT_VALUE: Self = Self::opt(Base::Value);
    const LIST: Self = Self::new(Base::List);
    const OPT_LIST: Self = Self::opt(Base::List);
    const NODE: Self = Self::new(Base::Node);
    const TABLE: Self = Self::new(Base::Table);
    const OPS: Self = Self::new(Base::Ops);
    const CONTEXT: Self = Self::new(Base::Context);
    const VOID: Self = Self::new(Base::Void);

    const fn is_numeric(self) -> bool {
        matches!(self.base, Base::Int | Base::Float)
    }

    fn is_string(self) -> bool {
        self.base == Base::Str
    }

    const fn default_literal(self) -> Option<&'static str> {
        match self.base {
            Base::Str => Some("\"\""),
            Base::Int | Base::Float => Some("0"),
            Base::Bool => Some("false"),
            Base::List | Base::Table => Some("{}"),
            Base::Value
            | Base::Node
            | Base::Ops
            | Base::Context
            | Base::Void => None,
        }
    }
}

pub(crate) type PayloadParam = (&'static str, &'static str, &'static str);

type Constant = (String, Ty);
type Constants = HashMap<String, Constant>;
type LocalStatics = Vec<HashMap<String, (String, StaticKind)>>;
type PatternBindings = Vec<(String, String)>;
type OptionalValue = Option<(String, Ty)>;
type OptionalStatic = Option<(String, StaticKind)>;
type Inlined = Option<(String, Ty)>;

/// Payload keys that map directly to a callback parameter, so the generated
/// body can take the argument instead of building and reading a payload table.
pub(crate) fn payload_params(event: &str) -> &'static [PayloadParam] {
    match event {
        "activate" => &[("id", "arg_id", "id")],
        "search" | "search_change" | "query" => &[("text", "arg_text", "text")],
        "ipc" => &[
            ("event", "arg_event", "event"),
            ("payload", "arg_payload", "payload"),
        ],
        "scroll" => &[("axis", "arg_axis", "axis")],
        "key" => &[("chord", "arg_chord", "chord")],
        "open" => &[("context", "arg_context", "context")],
        "audio_spectrum" => &[
            ("values_csv", "arg_values_csv", "valuesCsv"),
            ("state_csv", "arg_state_csv", "stateCsv"),
        ],
        _ => &[],
    }
}

#[derive(Default)]
pub(crate) struct Body {
    pub source: String,
    pub events: BTreeSet<String>,
    pub negative_events: BTreeSet<String>,
    pub uses_event: bool,
    pub uses_state: bool,
    pub uses_entry: bool,
    pub uses_payload: bool,
    pub payload_escapes: bool,
    pub uses_irem: bool,
    pub uses_unwrap: bool,
    pub uses_host_call: bool,
    pub uses_bool_or: bool,
    pub uses_source: bool,
    pub params: Vec<&'static str>,
    pub is_empty: bool,
    pub statics: Vec<(String, String)>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StaticKind {
    Once,
    OnceLock,
    Lazy,
    Mutex,
    RwLock,
    Atomic,
    Value,
}

#[derive(Clone, Debug)]
struct StaticDef {
    name: String,
    kind: StaticKind,
    init: String,
}

pub(crate) struct Emitter<'a> {
    ctx: String,
    out: String,
    indent: usize,
    scopes: Vec<HashMap<String, Ty>>,
    events: BTreeSet<String>,
    negative_events: BTreeSet<String>,
    uses_event: bool,
    bound_event: Option<String>,
    payload_params: &'static [PayloadParam],
    params: Vec<&'static str>,
    strings: Vec<String>,
    constants: Constants,
    constant_scopes: Vec<Constants>,
    program: &'a Program,
    module: String,
    home: String,
    inline_stack: Vec<String>,
    inline_budget: usize,
    statics: BTreeMap<String, StaticDef>,
    local_statics: LocalStatics,
    classes: HashMap<String, String>,
    self_class: Option<String>,
    pending_class: Option<String>,
    value_helper: bool,
    in_statement: bool,
    containers: HashMap<String, StaticKind>,
    tail_target: Option<String>,
    uses_state: bool,
    uses_payload: bool,
    payload_escapes: bool,
    uses_entry: bool,
    uses_irem: bool,
    uses_unwrap: bool,
    uses_host_call: bool,
    uses_bool_or: bool,
    uses_source: bool,
    emitted: bool,
    temp: usize,
}

const MAX_INLINE_DEPTH: usize = 8;
const MAX_INLINE_CALLS: usize = 200;

impl<'a> Emitter<'a> {
    pub(crate) fn new(
        ctx: &syn::Ident,
        bound_event: Option<String>,
        strings: &[&str],
        payload_params: &'static [PayloadParam],
        program: &'a Program,
        module: &str,
    ) -> Self {
        Self {
            ctx: ctx.to_string(),
            out: String::new(),
            indent: 1,
            scopes: vec![HashMap::new()],
            events: BTreeSet::new(),
            negative_events: BTreeSet::new(),
            uses_event: false,
            bound_event,
            payload_params,
            params: Vec::new(),
            strings: strings.iter().map(|key| (*key).to_owned()).collect(),
            constants: HashMap::new(),
            constant_scopes: Vec::new(),
            program,
            module: module.to_owned(),
            home: module.to_owned(),
            inline_stack: Vec::new(),
            inline_budget: MAX_INLINE_CALLS,
            statics: BTreeMap::new(),
            local_statics: Vec::new(),
            classes: HashMap::new(),
            self_class: None,
            pending_class: None,
            value_helper: false,
            in_statement: false,
            containers: HashMap::new(),
            tail_target: None,
            uses_state: false,
            uses_payload: false,
            payload_escapes: false,
            uses_entry: false,
            uses_irem: false,
            uses_unwrap: false,
            uses_host_call: false,
            uses_bool_or: false,
            uses_source: false,
            emitted: false,
            temp: 0,
        }
    }

    pub(crate) fn constants(&mut self) {
        for (name, value) in self.program.constants(&self.module) {
            if let Some(entry) = constant(value) {
                self.constants.insert(name.to_owned(), entry);
            }
        }
    }

    fn register_static(
        &mut self,
        module: &str,
        name: &str,
    ) -> Result<StaticDef, Error> {
        let key = format!("{module}::{name}");
        if let Some(def) = self.statics.get(&key) {
            return Ok(def.clone());
        }
        let item = self
            .program
            .resolve_static(module, &[name.to_owned()])
            .map(|(_, item)| item.clone())
            .ok_or_else(|| {
                Error::at(
                    proc_macro2::Span::call_site(),
                    format!("unknown static `{name}` in a compilable handler"),
                )
            })?;
        let kind = static_kind(&item.ty);
        let init = self.static_init(&item.expr, kind)?;
        let lua_name = format!("__static_{}", sanitize(&key));
        let def = StaticDef {
            name: lua_name,
            kind,
            init,
        };
        self.statics.insert(key, def.clone());
        Ok(def)
    }

    fn static_init(
        &mut self,
        expr: &Expr,
        kind: StaticKind,
    ) -> Result<String, Error> {
        if let Some((text, _)) = constant(expr) {
            return Ok(text);
        }
        if let Expr::Path(path) = expr
            && path_segments(&path.path).is_some_and(|parts| {
                parts.last().is_some_and(|name| name == "Null")
                    && parts.iter().any(|name| name == "Value")
            })
        {
            return Ok("nil".to_owned());
        }
        if let Expr::Call(call) = expr
            && let Some(name) = path_last_name(&call.func)
        {
            let variant = name.rsplit("::").next().unwrap_or(&name);
            match (variant, kind) {
                ("new", StaticKind::Once) => return Ok("false".to_owned()),
                ("new", StaticKind::OnceLock | StaticKind::Lazy) => {
                    return Ok("nil".to_owned());
                }
                ("new", StaticKind::Mutex | StaticKind::RwLock) => {
                    let inner =
                        arg(&call.args.iter().collect::<Vec<_>>(), 0, call)?;
                    return self.static_value(inner);
                }
                ("new", StaticKind::Atomic) => {
                    let inner =
                        arg(&call.args.iter().collect::<Vec<_>>(), 0, call)?;
                    return self.static_value(inner);
                }
                _ => {}
            }
        }
        if let Expr::Struct(item) = expr {
            let mut fields = Vec::new();
            for field in &item.fields {
                let syn::Member::Named(name) = &field.member else {
                    return Err(Error::at(
                        field.member.span(),
                        "tuple structs are not supported in static initializers",
                    ));
                };
                let value = self.static_value(&field.expr)?;
                fields
                    .push(format!("{} = {value}", lua_key(&name.to_string())));
            }
            return Ok(format!("{{ {} }}", fields.join(", ")));
        }
        self.static_value(expr)
    }

    fn static_value(&mut self, expr: &Expr) -> Result<String, Error> {
        let saved_out = std::mem::take(&mut self.out);
        let saved_indent = self.indent;
        self.indent = 0;
        let result = self.expr(expr);
        let lines = std::mem::replace(&mut self.out, saved_out);
        self.indent = saved_indent;
        let (text, _) = result?;
        if lines.trim().is_empty() {
            Ok(text)
        } else {
            Ok(format!("(function()\n{lines}  return {text}\nend)()"))
        }
    }

    fn constant(&self, name: &str) -> Option<Constant> {
        if let Some(value) = self.constants.get(name) {
            return Some(value.clone());
        }
        for scope in self.constant_scopes.iter().rev() {
            if let Some(value) = scope.get(name) {
                return Some(value.clone());
            }
        }
        None
    }

    fn module_constants(&self, module: &str) -> Constants {
        let mut scope = HashMap::new();
        for (name, value) in self.program.constants(module) {
            if let Some(entry) = constant(value) {
                scope.insert(name.to_owned(), entry);
            }
        }
        scope
    }

    fn helper(&self, call: &ExprCall) -> Option<Helper> {
        let Expr::Path(path) = &*call.func else {
            return None;
        };
        let segments = path_segments(&path.path)?;
        let resolved = self.program.resolve(&self.module, &segments)?;
        Some(Helper {
            module: resolved.module.clone(),
            function: resolved.function.clone(),
            name: segments.join("::"),
        })
    }

    fn check_inline(
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
            | (Inline::Tail, Returns::Ops | Returns::Unit) => {}
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
            (Inline::Value, Returns::VecOps)
            | (Inline::Tail, Returns::VecOps) => {
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
            (Inline::Statement, Returns::Value(_) | Returns::VecOps) => {
                return Err(Error::at(
                    *span,
                    format!(
                        "helper `{}` returns a value and must be used in an expression",
                        helper.name
                    ),
                ));
            }
        }
        check_body(&helper.function, mode)
    }

    fn bindings(
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

    fn binding(
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

    fn inline_call(
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

    fn inline_resolved(
        &mut self,
        helper: Helper,
        mode: Inline,
        span: proc_macro2::Span,
        bindings: Vec<Binding>,
    ) -> Result<Inlined, Error> {
        let returns = return_kind(&helper.function)?;
        let check = self.check_inline(&helper, mode, returns, &span)?;
        let value = self.inline(&helper, bindings, mode, returns, check)?;
        Ok(Some(value))
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
            && self.lookup(&ident.to_string()) == Some(Ty::OPS)
        {
            return Ok(());
        }
        self.stmt(last)
    }

    fn inline(
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
                    self.push(&name, ty);
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

    pub(crate) fn block(&mut self, block: &syn::Block) -> Result<(), Error> {
        self.scopes.push(HashMap::new());
        self.local_statics.push(HashMap::new());
        let result = self.block_inner(block, true);
        self.local_statics.pop();
        self.scopes.pop();
        result
    }

    fn block_inner(
        &mut self,
        block: &syn::Block,
        tail: bool,
    ) -> Result<(), Error> {
        let Some((last, rest)) = block.stmts.split_last() else {
            return Ok(());
        };
        for stmt in rest {
            self.stmt(stmt)?;
        }
        if tail {
            self.tail_stmt(last)
        } else {
            self.stmt(last)
        }
    }

    pub(crate) fn finish(self) -> Body {
        let mut source = String::new();
        if self.uses_state {
            source.push_str(
                "  local plugin_state = state.get(STATE_KEY) or {}\n",
            );
        }
        if self.uses_entry {
            source.push_str("  local entry = ENTRY\n");
        }
        source.push_str(&self.out);
        Body {
            source,
            events: self.events,
            negative_events: self.negative_events,
            uses_event: self.uses_event,
            uses_state: self.uses_state,
            uses_entry: self.uses_entry,
            uses_payload: self.uses_payload,
            payload_escapes: self.payload_escapes,
            uses_irem: self.uses_irem,
            uses_unwrap: self.uses_unwrap,
            uses_host_call: self.uses_host_call,
            uses_bool_or: self.uses_bool_or,
            uses_source: self.uses_source,
            params: self
                .payload_params
                .iter()
                .filter(|(_, body_param, _)| self.params.contains(body_param))
                .map(|(_, body_param, _)| *body_param)
                .collect(),
            is_empty: !self.emitted,
            statics: self
                .statics
                .values()
                .map(|def| (def.name.clone(), def.init.clone()))
                .collect(),
        }
    }

    fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    fn push(&mut self, name: &str, ty: Ty) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_owned(), ty);
        }
    }

    fn lookup(&self, name: &str) -> Option<Ty> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    fn stmt(&mut self, stmt: &Stmt) -> Result<(), Error> {
        match stmt {
            Stmt::Local(local) => self.local(local),
            Stmt::Item(syn::Item::Static(item)) => self.local_static(item),
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

    fn tail_stmt(&mut self, stmt: &Stmt) -> Result<(), Error> {
        match stmt {
            Stmt::Expr(expr, _) => self.tail_expr(expr),
            other => self.stmt(other),
        }
    }

    fn tail_expr(&mut self, expr: &Expr) -> Result<(), Error> {
        if let Expr::Call(call) = expr
            && !is_ok_call(call)
            && self.inline_call(call, Inline::Tail)?.is_some()
        {
            return Ok(());
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

    fn local(&mut self, local: &syn::Local) -> Result<(), Error> {
        let (pattern, hint) = match &local.pat {
            Pat::Type(typed) => (&*typed.pat, Some(&*typed.ty)),
            other => (other, None),
        };
        if let Some(init) = &local.init
            && let Some((_, diverge)) = &init.diverge
        {
            return self.let_else(pattern, &init.expr, diverge);
        }
        if let Pat::Tuple(tuple) = pattern {
            let (text, _) = self.expr(
                local.init.as_ref().map(|i| &*i.expr).ok_or_else(|| {
                    Error::at(
                        local.span(),
                        "tuple binding needs an initializer",
                    )
                })?,
            )?;
            let mut names = Vec::new();
            for element in &tuple.elems {
                match element {
                    Pat::Ident(pattern) => {
                        let name = pattern.ident.to_string();
                        self.push(&name, Ty::VALUE);
                        names.push(name);
                    }
                    Pat::Wild(_) => names.push("_".to_owned()),
                    other => {
                        return Err(Error::at(
                            other.span(),
                            "tuple bindings must be plain identifiers",
                        ));
                    }
                }
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
                names.join(", "),
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
        let Some(init) = &local.init else {
            let ty = hint.map_or(Ty::VALUE, type_hint);
            if let Some(kind) = hint.map(static_kind)
                && kind != StaticKind::Value
            {
                self.containers.insert(name.clone(), kind);
            }
            self.push(&name, ty);
            self.line(&format!("local {name} = nil"));
            return Ok(());
        };
        if let Expr::Call(call) = &*init.expr
            && is_vec_new(call)
        {
            self.line(&format!("local {name} = {{}}"));
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
                self.line(&format!("local {name} = nil"));
                self.push(&name, Ty::OPS);
                return Ok(());
            }
            let (text, ty) = self.list(&items)?;
            self.line(&format!("local {name} = {text}"));
            self.push(&name, ty);
            return Ok(());
        }
        let (text, ty) = self.expr(&init.expr)?;
        self.line(&format!("local {name} = {text}"));
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
        let (name, inner) = match pattern {
            Pat::Ident(pattern) => (pattern.ident.to_string(), None),
            Pat::TupleStruct(item)
                if item
                    .path
                    .segments
                    .last()
                    .is_some_and(|s| s.ident == "Some") =>
            {
                match item.elems.first() {
                    Some(Pat::Ident(pattern)) => {
                        (pattern.ident.to_string(), Some(true))
                    }
                    _ => {
                        return Err(Error::at(
                            item.span(),
                            "let-else patterns must bind plain identifiers",
                        ));
                    }
                }
            }
            _ => {
                return Err(Error::at(
                    pattern.span(),
                    "let-else patterns must be plain identifiers or `Some(name)`",
                ));
            }
        };
        let (text, ty) = self.expr(init)?;
        self.line(&format!("local {name} = {text}"));
        self.push(
            &name,
            if inner.is_some() {
                Ty::opt(ty.base)
            } else {
                ty
            },
        );
        self.line(&format!("if {name} == nil then"));
        self.indent = self.indent.saturating_add(1);
        self.expr_stmt(diverge)?;
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        Ok(())
    }

    fn local_static(&mut self, item: &syn::ItemStatic) -> Result<(), Error> {
        let kind = static_kind(&item.ty);
        let init = self.static_init(&item.expr, kind)?;
        let name = format!("__local_{}", self.temp().trim_start_matches("__t"));
        self.line(&format!("local {name} = {init}"));
        if let Some(scope) = self.local_statics.last_mut() {
            scope.insert(item.ident.to_string(), (name, kind));
        }
        Ok(())
    }

    fn list(&mut self, items: &[Expr]) -> Result<(String, Ty), Error> {
        if items.is_empty() {
            return Ok(("{}".to_owned(), Ty::LIST));
        }
        let mut values = Vec::new();
        for item in items {
            values.push(self.expr(item)?.0);
        }
        Ok((format!("{{ {} }}", values.join(", ")), Ty::LIST))
    }

    fn expr_stmt(&mut self, expr: &Expr) -> Result<(), Error> {
        if let Expr::Call(call) = expr
            && !is_ok_call(call)
            && !is_ops_push(call)
            && self.inline_call(call, Inline::Statement)?.is_some()
        {
            return Ok(());
        }
        match expr {
            Expr::Call(call) if is_ok_call(call) => self.ok_tail(&call.args),
            Expr::Call(call) if is_ops_push(call) => {
                let value = call.args.first().ok_or_else(|| {
                    Error::at(call.span(), "push() needs a value")
                })?;
                self.op_expr(value)
            }
            Expr::If(item) => self.if_stmt(item, false),
            Expr::Match(item) => self.match_stmt(item, false),
            Expr::ForLoop(item) => self.for_stmt(item),
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
                let (text, ty) = self.expr(expr)?;
                if ty.base == Base::Void
                    || (matches!(expr, Expr::MethodCall(_)) && !text.is_empty())
                {
                    if !text.is_empty() {
                        self.line(&text);
                    }
                    Ok(())
                } else {
                    Err(Error::at(
                        expr.span(),
                        "this expression has no effect and is not a supported op",
                    ))
                }
            }
        }
    }
    fn ret(&mut self, expr: Option<&Expr>) -> Result<(), Error> {
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
        let (text, _) = self.expr(expr)?;
        if let Some(target) = self.tail_target.clone() {
            self.line(&format!("{target} = {text}"));
        } else {
            self.line(&format!("return {text}"));
        }
        Ok(())
    }

    fn ok(&mut self, args: &Punctuated<Expr, Token![,]>) -> Result<(), Error> {
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
            && self.lookup(&segment.to_string()) == Some(Ty::OPS)
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

    fn inline_block(
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

    fn let_condition(&mut self, item: &syn::ExprLet) -> Result<String, Error> {
        let (text, ty) = self.expr(&item.expr)?;
        match &*item.pat {
            Pat::Ident(pattern) => {
                let name = pattern.ident.to_string();
                self.line(&format!("local {name} = {text}"));
                self.push(&name, ty);
                Ok(format!("({name} ~= nil)"))
            }
            Pat::TupleStruct(binding)
                if binding
                    .path
                    .segments
                    .last()
                    .is_some_and(|s| s.ident == "Some") =>
            {
                let Some(Pat::Ident(pattern)) = binding.elems.first() else {
                    return Err(Error::at(
                        binding.span(),
                        "if let patterns must bind plain identifiers",
                    ));
                };
                let name = pattern.ident.to_string();
                self.line(&format!("local {name} = {text}"));
                self.push(&name, Ty::opt(ty.base));
                Ok(format!("({name} ~= nil)"))
            }
            Pat::Wild(_) => Ok(text),
            other => Err(Error::at(
                other.span(),
                "if let patterns must be plain identifiers or `Some(name)`",
            )),
        }
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

    fn const_bool(&self, expr: &Expr) -> Option<bool> {
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

    fn match_stmt(
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
        let (scrutinee, _) = self.expr(&item.expr)?;
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
            let (mut condition, bindings) =
                self.pattern_match(&scrutinee, pattern)?;
            if let Some(guard) = guard {
                let (text, _) = self.expr(guard)?;
                condition = format!("({condition}) and ({text})");
            }
            let keyword = if opened { "elseif" } else { "if" };
            self.line(&format!("{keyword} {condition} then"));
            self.indent = self.indent.saturating_add(1);
            self.scopes.push(HashMap::new());
            for (name, text) in bindings {
                self.line(&format!("local {name} = {text}"));
                self.push(&name, Ty::VALUE);
            }
            let result = self.arm_body(&arm.body, tail);
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

    fn pattern_match(
        &self,
        value: &str,
        pattern: &Pat,
    ) -> Result<(String, PatternBindings), Error> {
        match pattern {
            Pat::Wild(_) => Ok(("true".to_owned(), Vec::new())),
            Pat::Paren(paren) => self.pattern_match(value, &paren.pat),
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
                        self.pattern_match(value, case)?;
                    conditions.push(condition);
                    if bindings.is_empty() {
                        bindings = case_bindings;
                    }
                }
                Ok((format!("({})", conditions.join(" or ")), bindings))
            }
            Pat::Ident(pattern) => {
                let name = pattern.ident.to_string();
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
                    _ => {
                        return Err(Error::at(
                            item.span(),
                            format!(
                                "match pattern `{name}` is not supported by the Luau compiler"
                            ),
                        ));
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
            Pat::Tuple(tuple) => {
                let mut conditions = Vec::new();
                let mut bindings = Vec::new();
                for (index, element) in tuple.elems.iter().enumerate() {
                    let item = format!("{value}[{}]", index + 1);
                    conditions.push(format!("({item} ~= nil)"));
                    if let Pat::Ident(pattern) = element {
                        bindings.push((pattern.ident.to_string(), item));
                    }
                }
                Ok((format!("({})", conditions.join(" and ")), bindings))
            }
            _ => Err(Error::at(
                pattern.span(),
                "match pattern is not supported by the Luau compiler",
            )),
        }
    }

    fn nested_pattern(
        &self,
        value: &str,
        pattern: &Pat,
        conditions: &mut Vec<String>,
        bindings: &mut PatternBindings,
    ) -> Result<(), Error> {
        match pattern {
            Pat::Ident(pattern) => {
                bindings.push((pattern.ident.to_string(), value.to_owned()));
            }
            Pat::Wild(_) => {}
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
                        return Err(Error::at(
                            path.span(),
                            format!(
                                "match pattern `{name}` is not supported by the Luau compiler"
                            ),
                        ));
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
                    _ => {
                        return Err(Error::at(
                            item.span(),
                            format!(
                                "match pattern `{name}` is not supported by the Luau compiler"
                            ),
                        ));
                    }
                }
                for element in &item.elems {
                    self.nested_pattern(value, element, conditions, bindings)?;
                }
            }
            _ => {
                return Err(Error::at(
                    pattern.span(),
                    "nested match patterns are not supported by the Luau compiler",
                ));
            }
        }
        Ok(())
    }
    fn inline_arm(&mut self, body: &Expr, tail: bool) -> Result<(), Error> {
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

    fn arm_body(&mut self, body: &Expr, tail: bool) -> Result<(), Error> {
        match body {
            Expr::Block(block) => self.block_inner(&block.block, tail),
            other if tail => self.tail_expr(other),
            other => self.expr_stmt(other),
        }
    }

    fn for_stmt(&mut self, item: &syn::ExprForLoop) -> Result<(), Error> {
        let Pat::Ident(pattern) = &*item.pat else {
            return Err(Error::at(
                item.pat.span(),
                "only plain loop variables are supported",
            ));
        };
        let name = pattern.ident.to_string();
        let iterable = strip_reference(&item.expr);
        if let Expr::Range(range) = iterable {
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
            self.push(&name, Ty::INT);
            self.line(&format!("for {name} = {start}, {end} do"));
            self.indent = self.indent.saturating_add(1);
            self.block(&item.body)?;
            self.indent = self.indent.saturating_sub(1);
            self.line("end");
            return Ok(());
        }
        let (text, ty) = self.expr(iterable)?;
        if ty.base != Base::List && ty.base != Base::Value {
            return Err(Error::at(
                item.expr.span(),
                "only ranges and list values can be looped over",
            ));
        }
        self.push(&name, Ty::VALUE);
        self.line(&format!("for _, {name} in ipairs({text}) do"));
        self.indent = self.indent.saturating_add(1);
        self.block(&item.body)?;
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
        Ok(())
    }

    fn assign(
        &mut self,
        left: &Expr,
        right: &Expr,
        op: &str,
    ) -> Result<(), Error> {
        let target = match strip_reference(left) {
            Expr::Path(path) => path
                .path
                .get_ident()
                .map(|ident| ident.to_string())
                .ok_or_else(|| {
                    Error::at(
                        left.span(),
                        "only assignments to local variables are supported",
                    )
                })?,
            Expr::Field(field) => {
                let Expr::Path(base) = strip_reference(&field.base) else {
                    return Err(Error::at(
                        left.span(),
                        "only assignments to local variables and struct fields are supported",
                    ));
                };
                let syn::Member::Named(member) = &field.member else {
                    return Err(Error::at(
                        left.span(),
                        "tuple field assignment is not supported",
                    ));
                };
                let base = base
                    .path
                    .get_ident()
                    .ok_or_else(|| {
                        Error::at(
                            left.span(),
                            "only assignments to local variables and struct fields are supported",
                        )
                    })?
                    .to_string();
                format!("{base}.{}", lua_key(&member.to_string()))
            }
            _ => {
                return Err(Error::at(
                    left.span(),
                    "only assignments to local variables are supported",
                ));
            }
        };
        if matches!(strip_reference(left), Expr::Path(_))
            && self.lookup(&target).is_none()
            && self
                .program
                .resolve_static(&self.module, std::slice::from_ref(&target))
                .is_none()
            && !self.containers.contains_key(&target)
        {
            return Err(Error::at(
                left.span(),
                format!("assignment to undeclared local `{target}`"),
            ));
        }
        let (text, ty) = self.expr(right)?;
        self.line(&format!("{target} {op} {text}"));
        if matches!(strip_reference(left), Expr::Path(_)) {
            self.push(&target, ty);
        }
        Ok(())
    }

    fn expr(&mut self, expr: &Expr) -> Result<(String, Ty), Error> {
        match expr {
            Expr::Lit(lit) => self.literal(lit),
            Expr::Path(path) => self.path(expr, path),
            Expr::Paren(paren) => self.expr(&paren.expr),
            Expr::Group(group) => self.expr(&group.expr),
            Expr::Reference(reference) => self.expr(&reference.expr),
            Expr::Unary(unary) => self.unary(unary),
            Expr::Binary(binary) => self.binary(binary),
            Expr::MethodCall(call) => self.method(call),
            Expr::Call(call) => self.call(call),
            Expr::Field(field) => self.field(expr, field),
            Expr::Macro(mac) => self.macro_expr(mac),
            Expr::Index(index) => self.index(index),
            Expr::Cast(cast) => self.cast(cast),
            Expr::Try(item) => self.try_expr(&item.expr),
            Expr::Closure(item) => self.closure(item),
            Expr::Match(item) => {
                let saved_out = std::mem::take(&mut self.out);
                let saved_indent = self.indent;
                let saved_target = self.tail_target.clone();
                self.indent = 0;
                self.tail_target = None;
                let result = self.match_stmt(item, true);
                let body = std::mem::replace(&mut self.out, saved_out);
                self.indent = saved_indent;
                self.tail_target = saved_target;
                result?;
                Ok((format!("(function()\n{body}end)()"), Ty::VALUE))
            }
            Expr::Struct(item) => self.struct_expr(item),
            Expr::If(item) => {
                let (chain, else_branch) = if_chain(item);
                if chain
                    .iter()
                    .all(|(condition, _)| self.const_bool(condition).is_some())
                {
                    for (condition, block) in &chain {
                        if self.const_bool(condition) == Some(true) {
                            return self.block_value(block);
                        }
                    }
                    return match else_branch {
                        Some(Expr::Block(block)) => {
                            self.block_value(&block.block)
                        }
                        _ => Ok(("nil".to_owned(), Ty::VALUE)),
                    };
                }
                let name = self.temp();
                self.line(&format!("local {name}"));
                let (condition, _) = self.expr(&item.cond)?;
                self.line(&format!("if {condition} then"));
                self.indent = self.indent.saturating_add(1);
                let (value, ty) = self.block_value(&item.then_branch)?;
                self.line(&format!("{name} = {value}"));
                self.indent = self.indent.saturating_sub(1);
                if let Some((_, branch)) = &item.else_branch {
                    self.line("else");
                    self.indent = self.indent.saturating_add(1);
                    let (value, _) = match &**branch {
                        Expr::Block(block) => self.block_value(&block.block)?,
                        other => self.expr(other)?,
                    };
                    self.line(&format!("{name} = {value}"));
                    self.indent = self.indent.saturating_sub(1);
                }
                self.line("end");
                Ok((name, ty))
            }
            Expr::Array(array) => {
                let mut values = Vec::new();
                for item in &array.elems {
                    values.push(self.expr(item)?.0);
                }
                Ok((format!("{{ {} }}", values.join(", ")), Ty::LIST))
            }
            Expr::Tuple(tuple) if tuple.elems.is_empty() => {
                Ok(("nil".to_owned(), Ty::VOID))
            }
            Expr::Tuple(tuple) => {
                let mut values = Vec::new();
                for element in &tuple.elems {
                    values.push(self.expr(element)?.0);
                }
                Ok((format!("{{ {} }}", values.join(", ")), Ty::LIST))
            }
            other => Err(Error::at(
                other.span(),
                "unsupported expression in a compilable handler",
            )),
        }
    }

    fn struct_expr(
        &mut self,
        item: &syn::ExprStruct,
    ) -> Result<(String, Ty), Error> {
        let segments = item
            .path
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        if segments
            .get(segments.len().saturating_sub(2))
            .is_some_and(|name| name == "Op")
        {
            self.op_expr(&Expr::Struct(item.clone()))?;
            return Ok((String::new(), Ty::VOID));
        }
        let class = segments.last().cloned().unwrap_or_default();
        let class = if class == "Self" {
            self.self_class.clone().unwrap_or(class)
        } else {
            class
        };
        let mut fields = Vec::new();
        for field in &item.fields {
            let syn::Member::Named(name) = &field.member else {
                return Err(Error::at(
                    field.member.span(),
                    "tuple structs are not supported by the Luau compiler",
                ));
            };
            let value = self.expr(&field.expr)?.0;
            fields.push(format!("{} = {value}", lua_key(&name.to_string())));
        }
        self.pending_class = Some(class);
        Ok((format!("{{ {} }}", fields.join(", ")), Ty::VALUE))
    }

    fn temp(&mut self) -> String {
        self.temp = self.temp.saturating_add(1);
        format!("__t{}", self.temp)
    }

    fn block_value(
        &mut self,
        block: &syn::Block,
    ) -> Result<(String, Ty), Error> {
        match block.stmts.last() {
            Some(Stmt::Expr(value, _)) => self.expr(value),
            _ => Ok(("nil".to_owned(), Ty::VALUE)),
        }
    }

    fn literal(&mut self, lit: &ExprLit) -> Result<(String, Ty), Error> {
        match &lit.lit {
            Lit::Str(value) => Ok((quote(&value.value()), Ty::STR)),
            Lit::Int(value) => Ok((value.base10_digits().to_owned(), Ty::INT)),
            Lit::Float(value) => {
                Ok((value.base10_digits().to_owned(), Ty::FLOAT))
            }
            Lit::Bool(value) => Ok((
                if value.value { "true" } else { "false" }.to_owned(),
                Ty::BOOL,
            )),
            Lit::Char(value) => {
                Ok((quote(&value.value().to_string()), Ty::STR))
            }
            other => Err(Error::at(other.span(), "unsupported literal")),
        }
    }

    fn path(
        &mut self,
        expr: &Expr,
        path: &syn::ExprPath,
    ) -> Result<(String, Ty), Error> {
        if path.qself.is_none()
            && path.path.segments.len() > 1
            && let Some(segments) = path_segments(&path.path)
            && let Some(value) =
                self.program.resolve_constant(&self.module, &segments)
            && let Some((text, ty)) = constant(value)
        {
            return Ok((text, ty));
        }
        if let Some(segments) = path_segments(&path.path)
            && segments.last().is_some_and(|name| name == "from")
            && segments.iter().any(|name| name == "PathBuf")
        {
            return Ok((
                "(function(__value) return __value end)".to_owned(),
                Ty::VALUE,
            ));
        }
        if let Some(segments) = path_segments(&path.path)
            && segments.last().is_some_and(|name| name == "Null")
            && segments.iter().any(|name| name == "Value")
        {
            return Ok(("nil".to_owned(), Ty::VALUE));
        }
        if let Some(segments) = path_segments(&path.path)
            && let Some(variant) =
                self.program.enum_variant(&self.module, &segments)
        {
            return Ok((quote(&variant), Ty::STR));
        }
        if let Some(segments) = path_segments(&path.path)
            && let Some(name) = segments.last()
        {
            if let Some((lua, _)) = self
                .local_statics
                .iter()
                .rev()
                .find_map(|scope| scope.get(name))
            {
                return Ok((lua.clone(), Ty::VALUE));
            }
            let resolved = self
                .program
                .resolve_static(&self.module, &segments)
                .map(|(module, _)| module);
            if let Some(module) = resolved {
                let def = self.register_static(&module, name)?;
                return Ok((def.name, Ty::VALUE));
            }
        }
        if let Some(segments) = path_segments(&path.path)
            && segments.len() == 2
            && segments[0] == "Value"
            && let Some(lua_type) = value_accessor(&segments[1])
        {
            return Ok((
                format!(
                    "(function(__value) if type(__value) == \"{lua_type}\" then return __value end end)"
                ),
                Ty::VALUE,
            ));
        }
        let Some(name) = path.path.get_ident() else {
            return Err(Error::at(expr.span(), "unsupported path expression"));
        };
        let name = name.to_string();
        if name == "true" || name == "false" {
            return Ok((name, Ty::BOOL));
        }
        if name == "None" {
            return Ok(("nil".to_owned(), Ty::VALUE));
        }
        if name == self.ctx {
            return Err(Error::at(
                expr.span(),
                "the context must be accessed through a supported field or method",
            ));
        }
        if self.lookup(&name) == Some(Ty::CONTEXT) {
            return Err(Error::at(
                expr.span(),
                "the context can only be passed to an inlined helper",
            ));
        }
        if let Some(ty) = self.lookup(&name) {
            return Ok((name, ty));
        }
        if let Some((value, ty)) = self.constant(&name) {
            return Ok((value, ty));
        }
        if let Some(value) = self.program.imported_constant(&self.module, &name)
            && let Some((text, ty)) = constant(value)
        {
            return Ok((text, ty));
        }
        Err(Error::at(
            expr.span(),
            format!("unknown variable `{name}` in a compilable handler"),
        ))
    }

    fn unary(&mut self, unary: &syn::ExprUnary) -> Result<(String, Ty), Error> {
        let (text, ty) = self.expr(&unary.expr)?;
        match unary.op {
            UnOp::Not(_) => Ok((format!("(not {text})"), Ty::BOOL)),
            UnOp::Neg(_) => Ok((format!("(-{text})"), ty)),
            UnOp::Deref(_) => Ok((text, ty)),
            other => Err(Error::at(other.span(), "unsupported unary operator")),
        }
    }

    fn binary(&mut self, binary: &ExprBinary) -> Result<(String, Ty), Error> {
        if let Some(operator) = compound_operator(&binary.op) {
            self.assign(&binary.left, &binary.right, operator)?;
            return Ok((String::new(), Ty::VOID));
        }
        if matches!(binary.op, BinOp::Eq(_) | BinOp::Ne(_)) {
            let negated = matches!(binary.op, BinOp::Ne(_));
            self.record_event(&binary.left, &binary.right, negated);
            self.record_event(&binary.right, &binary.left, negated);
        }
        let (left, left_ty) = self.expr(&binary.left)?;
        let (right, right_ty) = self.expr(&binary.right)?;
        if let Some(operator) = compare_operator(&binary.op) {
            return Ok((format!("({left} {operator} {right})"), Ty::BOOL));
        }
        let integer = left_ty.base == Base::Int && right_ty.base == Base::Int;
        match binary.op {
            BinOp::Add(_) => Ok((
                format!("({left} + {right})"),
                numeric(&left_ty, &right_ty),
            )),
            BinOp::Sub(_) => Ok((
                format!("({left} - {right})"),
                numeric(&left_ty, &right_ty),
            )),
            BinOp::Mul(_) => Ok((
                format!("({left} * {right})"),
                numeric(&left_ty, &right_ty),
            )),
            BinOp::Div(_) if integer => {
                Ok((format!("(math.modf({left} / {right}))"), Ty::INT))
            }
            BinOp::Div(_) => Ok((
                format!("({left} / {right})"),
                numeric(&left_ty, &right_ty),
            )),
            BinOp::Rem(_) if integer => {
                self.uses_irem = true;
                Ok((format!("irem({left}, {right})"), Ty::INT))
            }
            BinOp::Rem(_) => Ok((
                format!("({left} % {right})"),
                numeric(&left_ty, &right_ty),
            )),
            other => {
                Err(Error::at(other.span(), "unsupported binary operator"))
            }
        }
    }

    fn static_receiver(
        &mut self,
        expr: &Expr,
    ) -> Result<OptionalStatic, Error> {
        let Expr::Path(path) = strip_reference(expr) else {
            return Ok(None);
        };
        let Some(segments) = path_segments(&path.path) else {
            return Ok(None);
        };
        let Some(name) = segments.last() else {
            return Ok(None);
        };
        if let Some((lua, kind)) = self
            .local_statics
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
        {
            return Ok(Some((lua.clone(), *kind)));
        }
        if let Some(kind) = self.containers.get(name) {
            return Ok(Some((name.clone(), *kind)));
        }
        let Some((module, _)) =
            self.program.resolve_static(&self.module, &segments)
        else {
            return Ok(None);
        };
        let def = self.register_static(&module, name)?;
        Ok(Some((def.name, def.kind)))
    }

    fn static_method(
        &mut self,
        lua: &str,
        kind: StaticKind,
        call: &ExprMethodCall,
        args: &[&Expr],
    ) -> Result<(String, Ty), Error> {
        match call.method.to_string().as_str() {
            "call_once" => {
                let closure = arg(args, 0, call)?;
                self.line(&format!("if not {lua} then"));
                self.indent = self.indent.saturating_add(1);
                self.line(&format!("{lua} = true"));
                let (text, _) = self.invoke_arg(closure, &[])?;
                if !text.is_empty() {
                    self.line(&text);
                }
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok((String::new(), Ty::VOID))
            }
            "get_or_init" | "get_or_try_init" => {
                let closure = arg(args, 0, call)?;
                self.line(&format!("if {lua} == nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, ty) = self.invoke_arg(closure, &[])?;
                self.line(&format!("{lua} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok((
                    lua.to_owned(),
                    Ty {
                        result: false,
                        ..ty
                    },
                ))
            }
            "set" => {
                let value = arg(args, 0, call)?;
                let (text, _) = self.expr(value)?;
                let previous = self.temp();
                self.line(&format!("local {previous} = ({lua} ~= nil)"));
                self.line(&format!(
                    "if not {previous} then {lua} = {text} end"
                ));
                if self.in_statement {
                    Ok((String::new(), Ty::VOID))
                } else {
                    Ok((previous, Ty::BOOL))
                }
            }
            "take" => {
                self.line(&format!("local __taken = {lua}"));
                self.line(&format!("{lua} = nil"));
                Ok(("__taken".to_owned(), Ty::VALUE))
            }
            "lock" | "read" | "write" => {
                Ok((lua.to_owned(), Ty::result(Base::Value)))
            }
            "get" | "get_mut" | "load" | "deref" | "borrow" | "borrow_mut" => {
                Ok((lua.to_owned(), Ty::VALUE))
            }
            "store" | "swap" => {
                let value = arg(args, 0, call)?;
                let (text, _) = self.expr(value)?;
                let previous = self.temp();
                self.line(&format!("local {previous} = {lua}"));
                self.line(&format!("{lua} = {text}"));
                Ok((previous, Ty::VALUE))
            }
            "fetch_add" | "fetch_sub" => {
                let value = arg(args, 0, call)?;
                let (text, _) = self.expr(value)?;
                let previous = self.temp();
                self.line(&format!("local {previous} = {lua}"));
                let operator =
                    if call.method == "fetch_add" { "+" } else { "-" };
                self.line(&format!("{lua} = {previous} {operator} {text}"));
                Ok((previous, Ty::INT))
            }
            "is_completed" | "is_finished" => {
                Ok((format!("({lua} ~= nil)"), Ty::BOOL))
            }
            _ => {
                let _ = kind;
                Err(Error::at(
                    call.method.span(),
                    format!(
                        "method `{}` on a static is not supported by the Luau compiler",
                        call.method
                    ),
                ))
            }
        }
    }
    fn invoke_arg(
        &mut self,
        arg: &Expr,
        values: &[String],
    ) -> Result<(String, Ty), Error> {
        if let Expr::Closure(closure) = arg {
            return self.closure_value(closure, values);
        }
        let (text, ty) = self.expr(arg)?;
        Ok((format!("({text})({})", values.join(", ")), ty))
    }

    fn combinator(
        &mut self,
        call: &ExprMethodCall,
        method: &str,
        args: &[&Expr],
        receiver: &str,
        ty: Ty,
    ) -> Result<OptionalValue, Error> {
        match method {
            "lock" | "read" | "write" => {
                return Ok(Some((receiver.to_owned(), Ty::result(ty.base))));
            }
            "map_err" if ty.result => {
                return Ok(Some((receiver.to_owned(), ty)));
            }
            "unwrap_or_else" if ty.result => {
                return Ok(Some((
                    receiver.to_owned(),
                    Ty {
                        result: false,
                        ..ty
                    },
                )));
            }
            "map" | "and_then" => {
                let closure = arg(args, 0, call)?;
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result}"));
                self.line(&format!("if {value} ~= nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, value_ty) =
                    self.invoke_arg(closure, std::slice::from_ref(&value))?;
                self.line(&format!("{result} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((
                    result,
                    Ty {
                        optional: true,
                        ..value_ty
                    },
                )));
            }
            "filter" => {
                let closure = arg(args, 0, call)?;
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result}"));
                self.line(&format!("if {value} ~= nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, _) =
                    self.invoke_arg(closure, std::slice::from_ref(&value))?;
                self.line(&format!("if {text} then {result} = {value} end"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((result, Ty::opt(ty.base))));
            }
            "is_some_and" => {
                let closure = arg(args, 0, call)?;
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result} = false"));
                self.line(&format!("if {value} ~= nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, _) =
                    self.invoke_arg(closure, std::slice::from_ref(&value))?;
                self.line(&format!("{result} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((result, Ty::BOOL)));
            }
            "map_or" => {
                let default = arg(args, 0, call)?;
                let closure = arg(args, 1, call)?;
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                let (fallback, fallback_ty) = self.expr(default)?;
                self.line(&format!("local {result} = {fallback}"));
                self.line(&format!("if {value} ~= nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, value_ty) =
                    self.invoke_arg(closure, std::slice::from_ref(&value))?;
                self.line(&format!("{result} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((result, wider(fallback_ty, value_ty))));
            }
            "map_or_else" => {
                let default = arg(args, 0, call)?;
                let closure = arg(args, 1, call)?;
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result}"));
                self.line(&format!("if {value} == nil then"));
                self.indent = self.indent.saturating_add(1);
                let (fallback, fallback_ty) = self.invoke_arg(default, &[])?;
                self.line(&format!("{result} = {fallback}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("else");
                self.indent = self.indent.saturating_add(1);
                let (text, value_ty) =
                    self.invoke_arg(closure, std::slice::from_ref(&value))?;
                self.line(&format!("{result} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((result, wider(fallback_ty, value_ty))));
            }
            "unwrap_or_else" | "or_else" => {
                let closure = arg(args, 0, call)?;
                let value = self.temp();
                let result = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("local {result} = {value}"));
                self.line(&format!("if {value} == nil then"));
                self.indent = self.indent.saturating_add(1);
                let (text, fallback_ty) = self.invoke_arg(closure, &[])?;
                self.line(&format!("{result} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((result, wider(ty, fallback_ty))));
            }
            "ok_or" => {
                let message = arg(args, 0, call)?;
                let value = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("if {value} == nil then"));
                self.indent = self.indent.saturating_add(1);
                let (error, _) = self.expr(message)?;
                self.line(&format!("error(tostring({error}))"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((
                    value,
                    Ty {
                        result: false,
                        ..ty
                    },
                )));
            }
            "ok_or_else" => {
                let message = arg(args, 0, call)?;
                let value = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!("if {value} == nil then"));
                self.indent = self.indent.saturating_add(1);
                let (error, _) = self.invoke_arg(message, &[])?;
                self.line(&format!("error(tostring({error}))"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                return Ok(Some((
                    value,
                    Ty {
                        result: false,
                        ..ty
                    },
                )));
            }
            _ => {}
        }
        Ok(None)
    }
    fn class_of_receiver(&self, expr: &Expr) -> Option<String> {
        let Expr::Path(path) = strip_reference(expr) else {
            return None;
        };
        let name = path.path.get_ident()?.to_string();
        self.classes.get(&name).cloned()
    }

    fn method_call(
        &mut self,
        call: &ExprMethodCall,
        receiver: &str,
        class: &str,
    ) -> Result<OptionalValue, Error> {
        let Some((module, function)) = self.program.resolve_method(
            &self.module,
            class,
            &call.method.to_string(),
        ) else {
            return Ok(None);
        };
        let function = function.clone();
        let helper = Helper {
            module,
            name: format!("{class}::{}", call.method),
            function,
        };
        let returns = return_kind(&helper.function)?;
        let mode = match returns {
            Returns::Value(_) => Inline::Value,
            Returns::Unit => Inline::Statement,
            Returns::Ops | Returns::VecOps => {
                return Err(Error::at(
                    call.span(),
                    "methods returning ops are not supported in expression position",
                ));
            }
        };
        let mut bindings = vec![Binding::Local {
            name: "self".to_owned(),
            text: receiver.to_owned(),
            ty: Ty::VALUE,
            container: StaticKind::Value,
        }];
        let args = call.args.iter().collect::<Vec<_>>();
        let params = typed_args(&helper.function);
        if params.len() != args.len() {
            return Err(Error::at(
                call.span(),
                format!(
                    "`{class}::{}` takes {} arguments but {} were passed",
                    call.method,
                    params.len(),
                    args.len()
                ),
            ));
        }
        for ((pattern, ty), argument) in params.iter().zip(args.iter()) {
            bindings.push(self.binding(pattern, ty, argument)?);
        }
        self.self_class = Some(class.to_owned());
        let result = self.inline_resolved(helper, mode, call.span(), bindings);
        self.self_class = None;
        result
    }

    fn method(&mut self, call: &ExprMethodCall) -> Result<(String, Ty), Error> {
        let method = call.method.to_string();
        let args = call.args.iter().collect::<Vec<_>>();
        if self.is_ctx_expr(&call.receiver) {
            return self.context_method(call, &method, &args);
        }
        if is_config_expr(&call.receiver) && method == "get" {
            let (key, _) = self.expr(arg(&args, 0, call)?)?;
            return Ok((
                self.coerced_string(format!("noctalia.getConfig({key})"), true),
                Ty::OPT_VALUE,
            ));
        }
        if let Some((lua, kind)) = self.static_receiver(&call.receiver)? {
            return self.static_method(&lua, kind, call, &args);
        }
        if let Some(class) = self.class_of_receiver(&call.receiver) {
            let receiver = self.expr(&call.receiver)?.0;
            if let Some(value) = self.method_call(call, &receiver, &class)? {
                return Ok(value);
            }
        }
        let (receiver, ty) = self.expr(&call.receiver)?;
        if let Some(value) =
            self.combinator(call, &method, &args, &receiver, ty)?
        {
            return Ok(value);
        }
        match method.as_str() {
            "to_string" => Ok((format!("tostring({receiver})"), Ty::STR)),
            "into_owned" | "to_owned" | "clone" | "as_ref" | "as_deref"
            | "cloned" => Ok((receiver, ty)),
            "to_string_lossy" | "display" => Ok((receiver, Ty::STR)),
            "as_str" => Ok((receiver, Ty::OPT_STR)),
            "as_i64" | "as_u64" => Ok((receiver, Ty::OPT_INT)),
            "as_f64" => Ok((receiver, Ty::OPT_FLOAT)),
            "as_bool" => Ok((receiver, Ty::OPT_BOOL)),
            "as_array" => Ok((receiver, Ty::OPT_LIST)),
            "is_some" => Ok((format!("({receiver} ~= nil)"), Ty::BOOL)),
            "is_none" => Ok((format!("({receiver} == nil)"), Ty::BOOL)),
            "is_ok" => Ok((format!("({receiver} ~= nil)"), Ty::BOOL)),
            "is_err" => Ok((format!("({receiver} == nil)"), Ty::BOOL)),
            "is_null" => Ok((format!("({receiver} == nil)"), Ty::BOOL)),
            "is_object" | "is_array" => {
                Ok((format!("(type({receiver}) == \"table\")"), Ty::BOOL))
            }
            "is_string" => {
                Ok((format!("(type({receiver}) == \"string\")"), Ty::BOOL))
            }
            "is_number" => {
                Ok((format!("(type({receiver}) == \"number\")"), Ty::BOOL))
            }
            "is_boolean" => {
                Ok((format!("(type({receiver}) == \"boolean\")"), Ty::BOOL))
            }
            "expect" => {
                let (message, _) = self.expr(arg(&args, 0, call)?)?;
                let value = self.temp();
                self.line(&format!("local {value} = {receiver}"));
                self.line(&format!(
                    "if {value} == nil then error(tostring({message})) end"
                ));
                Ok((value, Ty::new(ty.base)))
            }
            "parse" => {
                let ty = turbofish_type(call.turbofish.as_ref());
                Ok((receiver, ty))
            }
            "ok" => Ok((
                receiver,
                Ty {
                    optional: true,
                    ..ty
                },
            )),
            "get_or_insert" | "get_or_insert_with" => {
                let value = arg(&args, 0, call)?;
                let (text, value_ty) = if method == "get_or_insert" {
                    self.expr(value)?
                } else {
                    self.invoke_arg(value, &[])?
                };
                self.line(&format!("if {receiver} == nil then"));
                self.indent = self.indent.saturating_add(1);
                self.line(&format!("{receiver} = {text}"));
                self.indent = self.indent.saturating_sub(1);
                self.line("end");
                Ok((receiver, value_ty))
            }
            "unwrap" => {
                self.uses_unwrap = true;
                Ok((format!("unwrap({receiver})"), Ty::new(ty.base)))
            }
            "unwrap_or" => {
                let (default, default_ty) = self.expr(arg(&args, 0, call)?)?;
                if ty.base == Base::Bool || default_ty.base == Base::Bool {
                    self.uses_bool_or = true;
                    return Ok((
                        format!("bool_or({receiver}, {default})"),
                        Ty::BOOL,
                    ));
                }
                Ok((
                    format!("({receiver} or {default})"),
                    Ty::new(wider(ty, default_ty).base),
                ))
            }
            "unwrap_or_default" => {
                let Some(default) = ty.default_literal() else {
                    return Err(Error::at(
                        call.span(),
                        "this type has no default in a compilable handler",
                    ));
                };
                Ok((format!("({receiver} or {default})"), Ty::new(ty.base)))
            }
            "len" => Ok((format!("#{receiver}"), Ty::INT)),
            "is_empty" => Ok((format!("(#{receiver} == 0)"), Ty::BOOL)),
            "get" => {
                let (key, key_ty) = self.expr(arg(&args, 0, call)?)?;
                if key_ty.base == Base::Int {
                    Ok((format!("{receiver}[({key}) + 1]"), Ty::OPT_VALUE))
                } else {
                    Ok((format!("{receiver}[{key}]"), Ty::OPT_VALUE))
                }
            }
            "trim" => Ok((format!("string.trim({receiver})"), Ty::STR)),
            "to_lowercase" => {
                Ok((format!("string.lower({receiver})"), Ty::STR))
            }
            "to_uppercase" => {
                Ok((format!("string.upper({receiver})"), Ty::STR))
            }
            "contains" => {
                let (needle, _) = self.expr(arg(&args, 0, call)?)?;
                Ok((
                    format!(
                        "(string.find({receiver}, {needle}, 1, true) ~= nil)"
                    ),
                    Ty::BOOL,
                ))
            }
            "push" if ty.base == Base::Ops => {
                self.op_expr(arg(&args, 0, call)?)?;
                Ok((String::new(), Ty::VOID))
            }
            "extend" => {
                let (value, _) = self.expr(arg(&args, 0, call)?)?;
                let item = self.temp();
                self.line(&format!(
                    "for _, {item} in ipairs({value}) do {receiver}[#({receiver}) + 1] = {item} end"
                ));
                Ok((String::new(), Ty::VOID))
            }
            "push" => {
                let (value, _) = self.expr(arg(&args, 0, call)?)?;
                Ok((format!("table.insert({receiver}, {value})"), Ty::VOID))
            }
            _ => Err(Error::at(
                call.method.span(),
                format!(
                    "method `{method}` is not supported by the Luau compiler"
                ),
            )),
        }
    }

    fn context_method(
        &mut self,
        call: &ExprMethodCall,
        method: &str,
        args: &[&Expr],
    ) -> Result<(String, Ty), Error> {
        match method {
            "config_str" => {
                let key = self.string_arg(args, 0, call)?;
                let raw = key.trim_matches('"');
                if self.strings.iter().any(|item| item == raw) {
                    return Ok((
                        format!("noctalia.getConfig({key})"),
                        Ty::OPT_STR,
                    ));
                }
                Ok((
                    self.coerced_string(
                        format!("noctalia.getConfig({key})"),
                        true,
                    ),
                    Ty::OPT_STR,
                ))
            }
            "config_bool" => {
                self.uses_bool_or = true;
                let key = self.string_arg(args, 0, call)?;
                let (default, _) = self.expr(arg(args, 1, call)?)?;
                Ok((
                    format!(
                        "bool_or(noctalia.getConfig({key}), {default})"
                    ),
                    Ty::BOOL,
                ))
            }
            "payload_str" => {
                let key = self.string_arg(args, 0, call)?;
                let raw = key.trim_matches('"');
                if let Some((_, body_param, _)) = self
                    .payload_params
                    .iter()
                    .find(|(payload_key, _, _)| *payload_key == raw)
                {
                    if !self.params.contains(body_param) {
                        self.params.push(body_param);
                    }
                    return Ok(((*body_param).to_owned(), Ty::OPT_STR));
                }
                self.uses_payload = true;
                Ok((
                    self.coerced_string(format!("payload[{key}]"), false),
                    Ty::OPT_STR,
                ))
            }
            "payload_arg" => {
                self.uses_payload = true;
                let (index, _) = self.expr(arg(args, 0, call)?)?;
                Ok((
                    format!("payload.args[({index}) + 1]"),
                    Ty::OPT_VALUE,
                ))
            }
            "state_value" => {
                self.uses_state = true;
                let key = self.string_arg(args, 0, call)?;
                let ty = turbofish_type(call.turbofish.as_ref());
                Ok((format!("plugin_state[{key}]"), ty))
            }
            "setting" | "setting_value" => {
                let key = self.string_arg(args, 0, call)?;
                Ok((
                    format!("noctalia.getSetting({key})"),
                    Ty::OPT_VALUE,
                ))
            }
            "color" => {
                let role = self.string_arg(args, 0, call)?;
                Ok((format!("noctalia.getColor({role})"), Ty::OPT_VALUE))
            }
            "host" => {
                let path = self.string_arg(args, 0, call)?;
                Ok((host_path(&unquote(&path))?.to_owned(), Ty::OPT_VALUE))
            }
            "clipboard" => Ok((
                "noctalia.clipboardText()".to_owned(),
                Ty::OPT_VALUE,
            )),
            "calendar" => Ok((
                "noctalia.calendarEvents()".to_owned(),
                Ty::OPT_VALUE,
            )),
            "media" => {
                Ok(("noctalia.mediaPlayer()".to_owned(), Ty::OPT_VALUE))
            }
            "stats" => {
                Ok(("noctalia.systemStats()".to_owned(), Ty::OPT_VALUE))
            }
            "nightlight" => Ok((
                "noctalia.nightlight.state".to_owned(),
                Ty::OPT_VALUE,
            )),
            "compositor" => Ok((
                "{ name = noctalia.compositor.name }".to_owned(),
                Ty::VALUE,
            )),
            "bar" => Ok((
                "{ vertical = barWidget.isVertical(), output_name = barWidget.outputName() }"
                    .to_owned(),
                Ty::VALUE,
            )),
            "bar_vertical" => {
                Ok(("barWidget.isVertical()".to_owned(), Ty::BOOL))
            }
            "bar_output_name" => Ok((
                "barWidget.outputName()".to_owned(),
                Ty::OPT_STR,
            )),
            "dark_mode" => {
                Ok(("noctalia.isDarkMode()".to_owned(), Ty::BOOL))
            }
            "plugin_dir" => {
                Ok(("noctalia.pluginDir()".to_owned(), Ty::STR))
            }
            "plugin_data_dir" => Ok((
                "noctalia.pluginDataDir()".to_owned(),
                Ty::OPT_STR,
            )),
            "time_format" => Ok((
                "noctalia.getSetting(\"shell.time_format\")".to_owned(),
                Ty::OPT_STR,
            )),
            "date_format" => Ok((
                "noctalia.getSetting(\"shell.date_format\")".to_owned(),
                Ty::OPT_STR,
            )),
            "tr" => {
                let key = self.string_arg(args, 0, call)?;
                let params = arg(args, 1, call)?;
                let params = strip_reference(params);
                let table = self.tr_params(params)?;
                Ok((format!("noctalia.tr({key}, {table})"), Ty::STR))
            }
            "file_exists" => {
                let path = self.string_arg(args, 0, call)?;
                Ok((format!("noctalia.fileExists({path})"), Ty::BOOL))
            }
            "command_exists" => {
                let name = self.string_arg(args, 0, call)?;
                Ok((format!("noctalia.commandExists({name})"), Ty::BOOL))
            }
            "read_file" => {
                let path = self.string_arg(args, 0, call)?;
                Ok((format!("noctalia.readFile({path})"), Ty::OPT_STR))
            }
            "write_file" => {
                let path = self.string_arg(args, 0, call)?;
                let contents = self.string_arg(args, 1, call)?;
                Ok((
                    format!("noctalia.writeFile({path}, {contents})"),
                    Ty::VOID,
                ))
            }
            "env" => {
                let key = self.string_arg(args, 0, call)?;
                Ok((format!("noctalia.getenv({key})"), Ty::OPT_STR))
            }
            "expand_path" => {
                let path = self.string_arg(args, 0, call)?;
                Ok((format!("noctalia.expandPath({path})"), Ty::STR))
            }
            "list_dir" => {
                let path = self.string_arg(args, 0, call)?;
                Ok((format!("(noctalia.listDir({path}))"), Ty::LIST))
            }
            "mkdir_all" => {
                let path = self.string_arg(args, 0, call)?;
                Ok((format!("noctalia.mkdirAll({path})"), Ty::VOID))
            }
            "remove_file" => {
                let path = self.string_arg(args, 0, call)?;
                Ok((format!("noctalia.removeFile({path})"), Ty::VOID))
            }
            "format_time" => {
                let format = self.string_arg(args, 0, call)?;
                Ok((format!("noctalia.formatTime({format})"), Ty::STR))
            }
            "format_time_at" => {
                let format = self.string_arg(args, 0, call)?;
                let seconds = match args.get(1) {
                    Some(argument) => {
                        let (text, _) = self.expr(argument)?;
                        format!("({text})")
                    }
                    None => "nil".to_owned(),
                };
                let timezone = match args.get(2) {
                    Some(argument) => {
                        let (text, _) = self.expr(argument)?;
                        format!("({text})")
                    }
                    None => "nil".to_owned(),
                };
                Ok((
                    format!(
                        "noctalia.formatTime({format}, {seconds}, {timezone})"
                    ),
                    Ty::STR,
                ))
            }
            "plugin_data_dir_result" => Ok((
                "(noctalia.pluginDataDir())".to_owned(),
                Ty::OPT_STR,
            )),
            "plugin_dir_result" => {
                Ok(("(noctalia.pluginDir())".to_owned(), Ty::OPT_STR))
            }
            "os_clock" => Ok(("os.clock()".to_owned(), Ty::FLOAT)),
            "os_time" => Ok(("os.time()".to_owned(), Ty::INT)),
            "math_random" => Ok(("math.random()".to_owned(), Ty::FLOAT)),
            "global_state" => {
                let key = self.string_arg(args, 0, call)?;
                Ok((
                    format!("state.get({key})"),
                    Ty::OPT_VALUE,
                ))
            }
            "render_source" => {
                self.uses_source = true;
                Ok(("source".to_owned(), Ty::STR))
            }
            "is_load_event" => {
                let text = match self.bound_event.as_deref() {
                    Some("load") => "true",
                    Some(_) => {
                        self.uses_source = true;
                        "(source == \"load\")"
                    }
                    None => {
                        self.uses_source = true;
                        "(event == \"load\" or source == \"load\")"
                    }
                };
                Ok((text.to_owned(), Ty::BOOL))
            }
            other => Err(Error::at(
                call.method.span(),
                format!(
                    "context method `{other}` is not supported by the Luau compiler"
                ),
            )),
        }
    }

    fn coerced_string(&mut self, value: String, allow_bool: bool) -> String {
        let name = self.temp();
        self.line(&format!("local {name} = {value}"));
        if allow_bool {
            format!(
                "((type({name}) == \"string\" and {name}) or ((type({name}) == \"number\" or type({name}) == \"boolean\") and tostring({name})) or nil)"
            )
        } else {
            format!(
                "((type({name}) == \"string\" and {name}) or (type({name}) == \"number\" and tostring({name})) or nil)"
            )
        }
    }

    fn tr_params(&mut self, expr: &Expr) -> Result<String, Error> {
        let Expr::Array(array) = expr else {
            return Err(Error::at(
                expr.span(),
                "ctx.tr params must be an array of tuples",
            ));
        };
        let mut parts = Vec::new();
        for element in &array.elems {
            let Expr::Tuple(tuple) = element else {
                return Err(Error::at(
                    element.span(),
                    "ctx.tr params must be (name, value) tuples",
                ));
            };
            let mut values = tuple.elems.iter();
            let name = values.next().ok_or_else(|| {
                Error::at(element.span(), "missing parameter name")
            })?;
            let value = values.next().ok_or_else(|| {
                Error::at(element.span(), "missing parameter value")
            })?;
            let name = expr_string(name)?;
            let (value, _) = self.expr(value)?;
            parts.push(format!("{} = {value}", lua_key(&name)));
        }
        Ok(format!("{{ {} }}", parts.join(", ")))
    }

    fn string_arg(
        &mut self,
        args: &[&Expr],
        index: usize,
        call: &ExprMethodCall,
    ) -> Result<String, Error> {
        let value = arg(args, index, call)?;
        let (text, ty) = self.expr(value)?;
        if ty.base != Base::Str {
            return Err(Error::at(
                value.span(),
                "expected a string literal argument",
            ));
        }
        Ok(text)
    }

    fn std_constructor(
        &mut self,
        call: &ExprCall,
        args: &[&Expr],
    ) -> Result<OptionalValue, Error> {
        let Some(name) = path_last_name(&call.func) else {
            return Ok(None);
        };
        let parts = name.split("::").collect::<Vec<_>>();
        let variant = if parts.last().is_some_and(|part| *part == "new") {
            parts
                .get(parts.len().saturating_sub(2))
                .copied()
                .unwrap_or("new")
        } else {
            parts.last().copied().unwrap_or("")
        };
        match variant {
            "HashMap" | "BTreeMap" | "HashSet" | "BTreeSet" | "Map" => {
                Ok(Some(("{}".to_owned(), Ty::LIST)))
            }
            "Bool" | "String" | "Number" => {
                let inner = arg(args, 0, call)?;
                self.expr(inner).map(Some)
            }
            "Mutex" | "RwLock" | "Cell" | "RefCell" => {
                let inner = arg(args, 0, call)?;
                let (text, ty) = self.expr(inner)?;
                if variant == "Mutex" || variant == "RwLock" {
                    if let Some(class) = self.pending_class.take() {
                        self.classes.insert(text.clone(), class);
                    }
                    return Ok(Some((text, Ty::result(Base::Value))));
                }
                Ok(Some((text, ty)))
            }
            "OnceLock" | "LazyLock" => Ok(Some(("nil".to_owned(), Ty::VALUE))),
            "node" if name.contains("ui::") => {
                let kind = self.expr(arg(args, 0, call)?)?.0;
                let props = self.expr(arg(args, 1, call)?)?.0;
                let children = self.expr(arg(args, 2, call)?)?.0;
                Ok(Some((
                    format!(
                        "{{ kind = {kind}, props = {props}, children = {children} }}"
                    ),
                    Ty::NODE,
                )))
            }
            "Some" => self.expr(arg(args, 0, call)?).map(Some),
            "Ok" => self.expr(arg(args, 0, call)?).map(Some),
            _ => Ok(None),
        }
    }

    fn associated_call(
        &mut self,
        call: &ExprCall,
        args: &[&Expr],
    ) -> Result<OptionalValue, Error> {
        let Expr::Path(path) = &*call.func else {
            return Ok(None);
        };
        let Some(segments) = path_segments(&path.path) else {
            return Ok(None);
        };
        if segments.len() < 2 {
            return Ok(None);
        }
        let method = segments.last().map_or("", String::as_str);
        let class = segments[segments.len() - 2].as_str();
        let Some((module, function)) =
            self.program.resolve_method(&self.module, class, method)
        else {
            return Ok(None);
        };
        let function = function.clone();
        let helper = Helper {
            module,
            name: format!("{class}::{method}"),
            function,
        };
        let mut bindings = Vec::new();
        let params = typed_args(&helper.function);
        if params.len() != args.len() {
            return Err(Error::at(
                call.span(),
                format!(
                    "`{class}::{method}` takes {} arguments but {} were passed",
                    params.len(),
                    args.len()
                ),
            ));
        }
        for ((pattern, ty), argument) in params.iter().zip(args.iter()) {
            bindings.push(self.binding(pattern, ty, argument)?);
        }
        self.self_class = Some(class.to_owned());
        let result =
            self.inline_resolved(helper, Inline::Value, call.span(), bindings);
        self.self_class = None;
        result
    }

    fn call(&mut self, call: &ExprCall) -> Result<(String, Ty), Error> {
        if let Some(name) = path_last_name(&call.func) {
            let args = call.args.iter().collect::<Vec<_>>();
            if !name.contains("::")
                && self.lookup(&name).is_some()
                && !self.is_ctx_expr(&call.func)
            {
                let mut values = Vec::new();
                for argument in &args {
                    values.push(self.expr(argument)?.0);
                }
                return Ok((
                    format!("{name}({})", values.join(", ")),
                    Ty::VALUE,
                ));
            }
            match name.as_str() {
                "Some" => return self.expr(arg(&args, 0, call)?),
                "String::new" => return Ok(("\"\"".to_owned(), Ty::STR)),
                "Vec::new" => return Ok(("{}".to_owned(), Ty::LIST)),
                _ => {}
            }
            if let Some(value) = self.std_constructor(call, &args)? {
                return Ok(value);
            }
            if let Some(kind) = ui_constructor(&name) {
                return self.ui_node(call, kind);
            }
            if let Some(value) = self.associated_call(call, &args)? {
                return Ok(value);
            }
            if let Some(op) = op_constructor(&name) {
                self.op_call(call, &op)?;
                return Ok((String::new(), Ty::VOID));
            }
            if let Some(op) = unsupported_op(&name) {
                return Err(Error::at(call.span(), op));
            }
        }
        if let Some(value) = self.inline_call(call, Inline::Value)? {
            return Ok(value);
        }
        let label = path_last_name(&call.func).unwrap_or_default();
        Err(Error::at(
            call.span(),
            format!(
                "call to `{label}` is not supported by the Luau compiler: no function in the crate matches, and only ops, ui constructors and a small set of std calls are supported"
            ),
        ))
    }

    fn ui_node(
        &mut self,
        call: &ExprCall,
        kind: &str,
    ) -> Result<(String, Ty), Error> {
        let args = call.args.iter().collect::<Vec<_>>();
        let props = match args.first() {
            Some(expr) => self.expr(expr)?.0,
            None => "{}".to_owned(),
        };
        let children = match args.get(1) {
            Some(expr) => {
                let expr = strip_reference(expr);
                if let Expr::Call(vec) = expr
                    && is_vec_new(vec)
                {
                    "{}".to_owned()
                } else if let Expr::Macro(mac) = expr
                    && macro_name(mac) == "vec"
                {
                    let items = parse_vec(mac)?;
                    self.list(&items)?.0
                } else {
                    self.expr(expr)?.0
                }
            }
            None => "{}".to_owned(),
        };
        Ok((
            format!(
                "{{ kind = {}, props = {props}, children = {children} }}",
                quote(kind)
            ),
            Ty::NODE,
        ))
    }

    fn field(
        &mut self,
        _expr: &Expr,
        field: &syn::ExprField,
    ) -> Result<(String, Ty), Error> {
        let member = match &field.member {
            Member::Named(name) => name.to_string(),
            Member::Unnamed(index) => {
                let (base, _) = self.expr(&field.base)?;
                return Ok((format!("{base}[{}]", index.index + 1), Ty::VALUE));
            }
        };
        if self.is_ctx_expr(&field.base) {
            return match member.as_str() {
                "event" => Ok(match &self.bound_event {
                    Some(event) => (quote(event), Ty::STR),
                    None => {
                        self.uses_event = true;
                        ("event".to_owned(), Ty::STR)
                    }
                }),
                "now_ms" => Ok(("noctalia.nowMs()".to_owned(), Ty::INT)),
                "entry" => {
                    self.uses_entry = true;
                    Ok(("entry".to_owned(), Ty::STR))
                }
                "payload" => {
                    self.uses_payload = true;
                    self.payload_escapes = true;
                    Ok(("payload".to_owned(), Ty::VALUE))
                }
                "state" => {
                    self.uses_state = true;
                    Ok(("plugin_state".to_owned(), Ty::VALUE))
                }
                "data_dir" => Ok((
                    "(noctalia.pluginDataDir() or noctalia.pluginDir())"
                        .to_owned(),
                    Ty::STR,
                )),
                "config" => Ok(("noctalia".to_owned(), Ty::VALUE)),
                other => Err(Error::at(
                    field.member.span(),
                    format!(
                        "context field `{other}` is not supported by the Luau compiler"
                    ),
                )),
            };
        }
        let (base, _) = self.expr(&field.base)?;
        Ok((
            if is_identifier(&member) {
                format!("{base}.{member}")
            } else {
                format!("{base}[{}]", quote(&member))
            },
            Ty::OPT_VALUE,
        ))
    }

    fn index(&mut self, index: &syn::ExprIndex) -> Result<(String, Ty), Error> {
        let (key, key_ty) = self.expr(&index.index)?;
        if is_config_expr(&index.expr) {
            return Ok((
                self.coerced_string(format!("noctalia.getConfig({key})"), true),
                Ty::OPT_VALUE,
            ));
        }
        let (base, _) = self.expr(&index.expr)?;
        if key_ty.base == Base::Int {
            Ok((format!("{base}[({key}) + 1]"), Ty::OPT_VALUE))
        } else {
            Ok((format!("{base}[{key}]"), Ty::OPT_VALUE))
        }
    }

    fn cast(&mut self, cast: &syn::ExprCast) -> Result<(String, Ty), Error> {
        let (text, _) = self.expr(&cast.expr)?;
        Ok((text, type_hint(&cast.ty)))
    }

    fn try_expr(&mut self, expr: &Expr) -> Result<(String, Ty), Error> {
        let (text, ty) = self.expr(expr)?;
        let name = self.temp();
        self.line(&format!("local {name} = {text}"));
        if ty.optional {
            self.line(&format!("if {name} == nil then return end"));
        }
        Ok((
            name,
            Ty {
                optional: false,
                result: false,
                ..ty
            },
        ))
    }

    fn closure(
        &mut self,
        closure: &syn::ExprClosure,
    ) -> Result<(String, Ty), Error> {
        let saved_out = std::mem::take(&mut self.out);
        let saved_indent = self.indent;
        let saved_target = self.tail_target.clone();
        self.indent = 0;
        self.tail_target = None;
        self.scopes.push(HashMap::new());
        self.local_statics.push(HashMap::new());
        let mut params = Vec::new();
        for input in &closure.inputs {
            let input = match input {
                Pat::Type(typed) => &*typed.pat,
                other => other,
            };
            match input {
                syn::Pat::Ident(pattern) => {
                    let name = pattern.ident.to_string();
                    self.push(&name, Ty::VALUE);
                    params.push(name);
                }
                syn::Pat::Wild(_) => params.push("_".to_owned()),
                other => {
                    self.local_statics.pop();
                    self.scopes.pop();
                    self.out = saved_out;
                    self.indent = saved_indent;
                    self.tail_target = saved_target;
                    return Err(Error::at(
                        other.span(),
                        "closure parameters must be plain identifiers",
                    ));
                }
            }
        }
        self.out
            .push_str(&format!("function({})\n", params.join(", ")));
        self.indent = 1;
        let result = match &*closure.body {
            Expr::Block(block) => self.block_inner(&block.block, true),
            body => self.tail_expr(body),
        };
        self.indent = 0;
        self.line("end");
        let text = std::mem::replace(&mut self.out, saved_out);
        self.local_statics.pop();
        self.scopes.pop();
        self.indent = saved_indent;
        self.tail_target = saved_target;
        result?;
        Ok((text, Ty::VALUE))
    }

    fn closure_value(
        &mut self,
        closure: &syn::ExprClosure,
        args: &[String],
    ) -> Result<(String, Ty), Error> {
        if closure.inputs.len() != args.len() {
            return Err(Error::at(
                closure.span(),
                format!(
                    "closure takes {} arguments but {} were passed",
                    closure.inputs.len(),
                    args.len()
                ),
            ));
        }
        self.scopes.push(HashMap::new());
        self.local_statics.push(HashMap::new());
        for (input, value) in closure.inputs.iter().zip(args) {
            let input = match input {
                Pat::Type(typed) => &*typed.pat,
                other => other,
            };
            match input {
                syn::Pat::Ident(pattern) => {
                    let name = pattern.ident.to_string();
                    self.line(&format!("local {name} = {value}"));
                    self.push(&name, Ty::VALUE);
                }
                syn::Pat::Wild(_) => {}
                other => {
                    self.local_statics.pop();
                    self.scopes.pop();
                    return Err(Error::at(
                        other.span(),
                        "closure parameters must be plain identifiers",
                    ));
                }
            }
        }
        let result = match &*closure.body {
            Expr::Block(block) => match block.block.stmts.split_last() {
                Some((last, rest)) => {
                    for stmt in rest {
                        self.stmt(stmt)?;
                    }
                    match last {
                        Stmt::Expr(expr, _) => self.expr(expr),
                        _ => Ok(("nil".to_owned(), Ty::VALUE)),
                    }
                }
                None => Ok(("nil".to_owned(), Ty::VALUE)),
            },
            body => self.expr(body),
        };
        self.local_statics.pop();
        self.scopes.pop();
        result
    }

    fn macro_expr(&mut self, mac: &ExprMacro) -> Result<(String, Ty), Error> {
        match macro_name(mac).as_str() {
            "format" => self.format(mac),
            "matches" => self.matches_macro(mac),
            "json" => self.json(mac),
            "vec" => {
                let items = parse_vec(mac)?;
                if items.iter().any(is_op_expr) {
                    return Err(Error::at(
                        mac.span(),
                        "vec! of ops is only supported as Ok(vec![...]) or a let binding",
                    ));
                }
                self.list(&items)
            }
            other => Err(Error::at(
                mac.span(),
                format!(
                    "macro `{other}!` is not supported by the Luau compiler"
                ),
            )),
        }
    }

    fn format(&mut self, mac: &ExprMacro) -> Result<(String, Ty), Error> {
        let parsed: FormatArgs = syn::parse2(mac.mac.tokens.clone())?;
        let mut pieces = Vec::new();
        let mut text = String::new();
        let mut chars = parsed.literal.chars().peekable();
        let mut positional =
            parsed.arguments.iter().filter(|(name, _)| name.is_none());
        while let Some(character) = chars.next() {
            match character {
                '{' => {
                    if chars.peek() == Some(&'{') {
                        chars.next();
                        text.push('{');
                        continue;
                    }
                    let mut placeholder = String::new();
                    for inner in chars.by_ref() {
                        if inner == '}' {
                            break;
                        }
                        placeholder.push(inner);
                    }
                    if !text.is_empty() {
                        pieces.push(quote(&text));
                        text.clear();
                    }
                    if placeholder.contains(':') {
                        return Err(Error::at(
                            mac.span(),
                            "format specifiers are not supported by the Luau compiler",
                        ));
                    }
                    if placeholder.is_empty() {
                        let (_, expr) = positional.next().ok_or_else(|| {
                            Error::at(
                                mac.span(),
                                "format! has more placeholders than arguments",
                            )
                        })?;
                        let (value, ty) = self.expr(expr)?;
                        pieces.push(format_piece(value, ty));
                    } else if let Some((_, expr)) = parsed
                        .arguments
                        .iter()
                        .find(|(key, _)| key.as_deref() == Some(&placeholder))
                    {
                        let (value, ty) = self.expr(expr)?;
                        pieces.push(format_piece(value, ty));
                    } else {
                        let ty = self.lookup(&placeholder).ok_or_else(|| {
                            Error::at(
                                mac.span(),
                                format!(
                                    "format! placeholder `{placeholder}` has no matching argument"
                                ),
                            )
                        })?;
                        pieces.push(format_piece(placeholder, ty));
                    }
                }
                '}' => {
                    if chars.peek() == Some(&'}') {
                        chars.next();
                    }
                    text.push('}');
                }
                other => text.push(other),
            }
        }
        if !text.is_empty() {
            pieces.push(quote(&text));
        }
        if pieces.is_empty() {
            pieces.push("\"\"".to_owned());
        }
        Ok((pieces.join(" .. "), Ty::STR))
    }

    fn json(&mut self, mac: &ExprMacro) -> Result<(String, Ty), Error> {
        let node: JsonNode = syn::parse2(mac.mac.tokens.clone())?;
        self.json_node(&node)
    }
    fn matches_macro(
        &mut self,
        mac: &ExprMacro,
    ) -> Result<(String, Ty), Error> {
        let parsed: MatchesArgs = syn::parse2(mac.mac.tokens.clone())?;
        self.record_pattern_events(&parsed.expr, &parsed.pat);
        let (expr, _) = self.expr(&parsed.expr)?;
        let condition = self.pattern_condition(&expr, &parsed.pat)?;
        Ok((format!("({condition})"), Ty::BOOL))
    }

    fn pattern_condition(
        &self,
        value: &str,
        pattern: &Pat,
    ) -> Result<String, Error> {
        match pattern {
            Pat::Wild(_) => Ok("true".to_owned()),
            Pat::Lit(lit) => {
                let literal = Expr::Lit(lit.clone());
                let (text, _) = constant(&literal).ok_or_else(|| {
                    Error::at(lit.span(), "unsupported matches! literal")
                })?;
                Ok(format!("({value} == {text})"))
            }
            Pat::Or(or) => {
                let mut parts = Vec::new();
                for case in &or.cases {
                    parts.push(self.pattern_condition(value, case)?);
                }
                Ok(format!("({})", parts.join(" or ")))
            }
            Pat::Paren(paren) => self.pattern_condition(value, &paren.pat),
            Pat::Ident(pattern) => {
                let name = pattern.ident.to_string();
                match name.as_str() {
                    "None" | "Null" => Ok(format!("({value} == nil)")),
                    "Some" => Ok(format!("({value} ~= nil)")),
                    _ => Ok("true".to_owned()),
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
                    "None" | "Null" => Ok(format!("({value} == nil)")),
                    "Some" => Ok(format!("({value} ~= nil)")),
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
                            return Ok(format!(
                                "({value} == {})",
                                quote(&variant)
                            ));
                        }
                        Err(Error::at(
                            path.span(),
                            format!(
                                "matches! pattern `{name}` is not supported by the Luau compiler"
                            ),
                        ))
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
                    "Some" => {
                        let mut conditions = vec![format!("({value} ~= nil)")];
                        let mut bindings = Vec::new();
                        for inner in &item.elems {
                            self.nested_pattern(
                                value,
                                inner,
                                &mut conditions,
                                &mut bindings,
                            )?;
                        }
                        Ok(format!("({})", conditions.join(" and ")))
                    }
                    "Null" => Ok(format!("({value} == nil)")),
                    "Bool" | "String" | "Number" | "Object" | "Array" => {
                        let lua_type = match name.as_str() {
                            "Bool" => "boolean",
                            "String" => "string",
                            "Number" => "number",
                            _ => "table",
                        };
                        let mut conditions =
                            vec![format!("(type({value}) == \"{lua_type}\")")];
                        let mut bindings = Vec::new();
                        for inner in &item.elems {
                            self.nested_pattern(
                                value,
                                inner,
                                &mut conditions,
                                &mut bindings,
                            )?;
                        }
                        Ok(format!("({})", conditions.join(" and ")))
                    }
                    _ => Err(Error::at(
                        item.span(),
                        format!(
                            "matches! pattern `{name}` is not supported by the Luau compiler"
                        ),
                    )),
                }
            }
            _ => Err(Error::at(
                pattern.span(),
                "matches! pattern is not supported by the Luau compiler",
            )),
        }
    }

    fn json_node(&mut self, node: &JsonNode) -> Result<(String, Ty), Error> {
        match node {
            JsonNode::Object(entries) => {
                let mut parts = Vec::new();
                for (key, value) in entries {
                    let (text, _) = self.json_node(value)?;
                    if text == "nil" {
                        return Err(Error::at(
                            value.span(),
                            "json null inside an object is not supported; omit the key instead",
                        ));
                    }
                    parts.push(format!("{} = {text}", lua_key(key)));
                }
                Ok((format!("{{ {} }}", parts.join(", ")), Ty::TABLE))
            }
            JsonNode::Array(values) => {
                let mut parts = Vec::new();
                for value in values {
                    let (text, _) = self.json_node(value)?;
                    if text == "nil" {
                        return Err(Error::at(
                            value.span(),
                            "json null inside an array is not supported",
                        ));
                    }
                    parts.push(text);
                }
                Ok((format!("{{ {} }}", parts.join(", ")), Ty::LIST))
            }
            JsonNode::Expr(expr) => {
                if let Expr::Path(path) = &**expr
                    && path.path.is_ident("null")
                {
                    return Ok(("nil".to_owned(), Ty::VALUE));
                }
                self.expr(expr)
            }
        }
    }

    fn record_pattern_events(&mut self, expr: &Expr, pattern: &Pat) {
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

    fn record_event(&mut self, event: &Expr, literal: &Expr, negated: bool) {
        if !self.is_event_expr(event) {
            return;
        }
        if let Expr::Lit(ExprLit {
            lit: Lit::Str(value),
            ..
        }) = literal
        {
            if negated {
                self.negative_events.insert(value.value());
            } else {
                self.events.insert(value.value());
            }
        }
    }

    fn is_ctx_expr(&self, expr: &Expr) -> bool {
        let Expr::Path(path) = expr else {
            return false;
        };
        path.path.get_ident().is_some_and(|ident| {
            let name = ident.to_string();
            name == self.ctx || self.lookup(&name) == Some(Ty::CONTEXT)
        })
    }

    fn is_event_expr(&self, expr: &Expr) -> bool {
        match expr {
            Expr::Field(field) => {
                self.is_ctx_expr(&field.base)
                    && matches!(&field.member, Member::Named(name) if name == "event")
            }
            Expr::MethodCall(call) => {
                self.is_event_expr(&call.receiver) && call.method == "as_str"
            }
            Expr::Paren(paren) => self.is_event_expr(&paren.expr),
            Expr::Group(group) => self.is_event_expr(&group.expr),
            _ => false,
        }
    }

    fn op_expr(&mut self, expr: &Expr) -> Result<(), Error> {
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
                if let Some(message) = unsupported_op(&name) {
                    return Err(Error::at(call.span(), message));
                }
                Err(Error::at(call.span(), "expected an Op constructor"))
            }
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

    fn op_call(&mut self, call: &ExprCall, name: &str) -> Result<(), Error> {
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
            "state_watch" => {
                let key = self.expr(arg(&args, 0, call)?)?.0;
                let id = self.expr(arg(&args, 1, call)?)?.0;
                self.line(&format!(
                    "state.watch({key}, {{ __host_callback = {id} }})"
                ));
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
                    self.expr(arg(&args, 2, call)?)?.0
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
                    self.expr(arg(&args, 2, call)?)?.0
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

fn is_config_expr(expr: &Expr) -> bool {
    let Expr::Field(field) = strip_reference(expr) else {
        return false;
    };
    matches!(&field.member, Member::Named(name) if name == "config")
        && matches!(&*field.base, Expr::Path(_))
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Inline {
    Tail,
    Statement,
    Value,
    Ops,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Returns {
    Ops,
    VecOps,
    Unit,
    Value(Ty),
}

struct Helper {
    module: String,
    function: ItemFn,
    name: String,
}

enum Binding {
    Context(String),
    Ops(String),
    Local {
        name: String,
        text: String,
        ty: Ty,
        container: StaticKind,
    },
}

fn constant(expr: &Expr) -> Option<(String, Ty)> {
    let Expr::Lit(literal) = expr else {
        return None;
    };
    Some(match &literal.lit {
        Lit::Str(value) => (quote(&value.value()), Ty::STR),
        Lit::Int(value) => (value.base10_digits().to_owned(), Ty::INT),
        Lit::Bool(value) => (
            if value.value { "true" } else { "false" }.to_owned(),
            Ty::BOOL,
        ),
        _ => return None,
    })
}

fn value_accessor(name: &str) -> Option<&'static str> {
    match name {
        "as_str" => Some("string"),
        "as_bool" => Some("boolean"),
        "as_i64" | "as_u64" | "as_f64" => Some("number"),
        "as_array" | "as_object" => Some("table"),
        _ => None,
    }
}

fn path_segments(path: &syn::Path) -> Option<Vec<String>> {
    if path.leading_colon.is_some()
        || path
            .segments
            .iter()
            .any(|segment| !matches!(segment.arguments, PathArguments::None))
    {
        return None;
    }
    Some(
        path.segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect(),
    )
}

fn generic_arg(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let syn::GenericArgument::Type(inner) = arguments.args.first()? else {
        return None;
    };
    Some(inner)
}

fn is_generic(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == name))
}

fn is_result(ty: &Type) -> bool {
    matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Result"))
}

fn is_ops_vec(ty: &Type) -> bool {
    let ty = match ty {
        Type::Reference(reference) => &*reference.elem,
        other => other,
    };
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    if segment.ident != "Vec" {
        return false;
    }
    generic_arg(ty).is_some_and(|inner| {
        matches!(inner, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Op"))
    })
}

fn return_kind(function: &ItemFn) -> Result<Returns, Error> {
    let ty = match &function.sig.output {
        syn::ReturnType::Default => return Ok(Returns::Unit),
        syn::ReturnType::Type(_, ty) => &**ty,
    };
    if is_result(ty) {
        return match generic_arg(ty) {
            Some(inner) if is_ops_vec(inner) => Ok(Returns::Ops),
            Some(inner) => Ok(Returns::Value(type_hint(inner))),
            None => Ok(Returns::Value(Ty::VALUE)),
        };
    }
    if is_ops_vec(ty) {
        return Ok(Returns::VecOps);
    }
    if matches!(ty, Type::Tuple(tuple) if tuple.elems.is_empty()) {
        return Ok(Returns::Unit);
    }
    Ok(Returns::Value(type_hint(ty)))
}

#[derive(Default)]
struct Scan {
    return_span: Option<proc_macro2::Span>,
    try_span: Option<proc_macro2::Span>,
    closure_span: Option<proc_macro2::Span>,
}
impl Scan {
    fn block(&mut self, block: &syn::Block) {
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
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

    fn expr(&mut self, expr: &Expr) {
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
struct BodyCheck {
    needs_iife: bool,
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

fn compare_operator(operator: &BinOp) -> Option<&'static str> {
    match operator {
        BinOp::Eq(_) => Some("=="),
        BinOp::Ne(_) => Some("~="),
        BinOp::Lt(_) => Some("<"),
        BinOp::Le(_) => Some("<="),
        BinOp::Gt(_) => Some(">"),
        BinOp::Ge(_) => Some(">="),
        BinOp::And(_) => Some("and"),
        BinOp::Or(_) => Some("or"),
        _ => None,
    }
}

fn compound_operator(operator: &BinOp) -> Option<&'static str> {
    match operator {
        BinOp::AddAssign(_) => Some("+="),
        BinOp::SubAssign(_) => Some("-="),
        BinOp::MulAssign(_) => Some("*="),
        _ => None,
    }
}

fn format_piece(value: String, ty: Ty) -> String {
    if ty.is_string() || ty.is_numeric() {
        value
    } else {
        format!("tostring({value})")
    }
}

fn numeric(left: &Ty, right: &Ty) -> Ty {
    if left.base == Base::Float || right.base == Base::Float {
        Ty::FLOAT
    } else {
        Ty::INT
    }
}

fn wider(left: Ty, right: Ty) -> Ty {
    if left.base == right.base {
        left
    } else if left.is_numeric() && right.is_numeric() {
        Ty::FLOAT
    } else {
        Ty::VALUE
    }
}

fn type_hint(ty: &Type) -> Ty {
    let text = type_text(ty);
    if is_generic(ty, "Option") {
        let Some(inner) = generic_arg(ty) else {
            return Ty::OPT_VALUE;
        };
        let mut value = type_hint(inner);
        if value.base == Base::Ops {
            return Ty::LIST;
        }
        value.optional = true;
        return value;
    }
    if is_generic(ty, "Result") {
        return generic_arg(ty).map_or(Ty::VALUE, type_hint);
    }
    if text.contains("String") || text == "str" {
        Ty::STR
    } else if text == "bool" {
        Ty::BOOL
    } else if text.starts_with("Vec") || text.starts_with("Option<Vec") {
        Ty::LIST
    } else if matches!(text.as_str(), "i64" | "u64" | "usize" | "u32" | "i32") {
        Ty::INT
    } else if matches!(text.as_str(), "f64" | "f32") {
        Ty::FLOAT
    } else if text == "Op" {
        Ty::OPS
    } else {
        Ty::VALUE
    }
}

fn type_text(ty: &Type) -> String {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map_or_else(String::new, |segment| segment.ident.to_string()),
        Type::Reference(reference) => type_text(&reference.elem),
        _ => String::new(),
    }
}

fn turbofish_type(
    arguments: Option<&syn::AngleBracketedGenericArguments>,
) -> Ty {
    let Some(arguments) = arguments else {
        return Ty::OPT_VALUE;
    };
    let Some(syn::GenericArgument::Type(ty)) = arguments.args.first() else {
        return Ty::OPT_VALUE;
    };
    let text = type_text(ty);
    let base = if text == "String" || text == "str" {
        Base::Str
    } else if text == "bool" {
        Base::Bool
    } else if matches!(text.as_str(), "i64" | "u64" | "usize") {
        Base::Int
    } else if matches!(text.as_str(), "f64" | "f32") {
        Base::Float
    } else if text.starts_with("Vec") {
        Base::List
    } else {
        Base::Value
    };
    Ty::opt(base)
}

fn host_path(path: &str) -> Result<&'static str, Error> {
    let value = match path {
        "now_ms" => "noctalia.nowMs()",
        "dark" => "noctalia.isDarkMode()",
        "outputs" => "noctalia.outputs()",
        "focused_output_name" => "noctalia.focusedOutputName()",
        "pluginDir" => "noctalia.pluginDir()",
        "pluginDataDir" => "noctalia.pluginDataDir()",
        "clipboard" => "noctalia.clipboardText()",
        "calendar" => "noctalia.calendarEvents()",
        "media" => "noctalia.mediaPlayer()",
        "stats" => "noctalia.systemStats()",
        "nightlight.state" => "noctalia.nightlight.state",
        "compositor.name" => "noctalia.compositor.name",
        "bar.vertical" => "barWidget.isVertical()",
        "bar.output_name" => "barWidget.outputName()",
        _ => {
            return Err(Error::plain(format!(
                "host path `{path}` is not supported by the Luau compiler"
            )));
        }
    };
    Ok(value)
}

fn qualified_variant<'a>(name: &'a str, root: &str) -> Option<&'a str> {
    let mut parts = name.rsplit("::");
    let variant = parts.next()?;
    (parts.next()? == root).then_some(variant)
}

fn unsupported_op(name: &str) -> Option<String> {
    let variant = qualified_variant(name, "Op")?;
    let message = match variant {
        "run" | "run_with_timeout" => {
            "op `run` needs a host callback id, which compiled Luau cannot allocate; use `Op::host_call(\"noctalia\", &[\"runAsync\"], ...)` instead"
        }
        "run_stream" => {
            "op `run_stream` needs a host callback id, which compiled Luau cannot allocate"
        }
        "http" | "http_bytes" => {
            "op `http` needs a host callback id, which compiled Luau cannot allocate; use `Op::host_call(\"noctalia\", &[\"http\"], ...)` instead"
        }
        "query" => {
            "op `query` needs a response path and callback id, which compiled Luau cannot allocate"
        }
        _ => return None,
    };
    Some(message.to_owned())
}

fn op_constructor(name: &str) -> Option<String> {
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
    ];
    supported.contains(&variant).then(|| variant.to_owned())
}

fn ui_constructor(name: &str) -> Option<&'static str> {
    let variant = qualified_variant(name, "ui")?;
    match variant {
        "column" => Some("column"),
        "row" => Some("row"),
        "scroll" => Some("scroll"),
        "label" => Some("label"),
        "button" => Some("button"),
        "input" => Some("input"),
        "progress" => Some("progress"),
        _ => None,
    }
}

fn path_last_name(expr: &Expr) -> Option<String> {
    let Expr::Path(path) = expr else {
        return None;
    };
    let segments = path
        .path
        .segments
        .iter()
        .map(|segment| segment.ident.to_string())
        .collect::<Vec<_>>();
    if segments.is_empty() {
        None
    } else {
        Some(segments.join("::"))
    }
}

fn macro_name(mac: &ExprMacro) -> String {
    mac.mac
        .path
        .segments
        .last()
        .map_or_else(String::new, |segment| segment.ident.to_string())
}

fn is_ok_call(call: &ExprCall) -> bool {
    path_last_name(&call.func).as_deref() == Some("Ok")
}

fn static_kind(ty: &syn::Type) -> StaticKind {
    let text = type_text(ty);
    match text.as_str() {
        "Once" => StaticKind::Once,
        "OnceLock" => StaticKind::OnceLock,
        "LazyLock" => StaticKind::Lazy,
        "Mutex" => StaticKind::Mutex,
        "RwLock" => StaticKind::RwLock,
        _ if text.starts_with("Atomic") => StaticKind::Atomic,
        _ => StaticKind::Value,
    }
}
fn is_err_call(call: &ExprCall) -> bool {
    path_last_name(&call.func).as_deref() == Some("Err")
}

fn is_vec_new(call: &ExprCall) -> bool {
    path_last_name(&call.func).as_deref() == Some("Vec::new")
}

fn is_ops_push(call: &ExprCall) -> bool {
    let Expr::MethodCall(method) = &*call.func else {
        return false;
    };
    method.method == "push" && call.args.len() == 1
}

fn is_op_expr(expr: &Expr) -> bool {
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
        _ => false,
    }
}

fn arg<'a>(
    args: &[&'a Expr],
    index: usize,
    call: &impl Spanned,
) -> Result<&'a Expr, Error> {
    args.get(index).copied().ok_or_else(|| {
        Error::at(call.span(), format!("missing argument {index}"))
    })
}

fn expr_string(expr: &Expr) -> Result<String, Error> {
    let Expr::Lit(ExprLit {
        lit: Lit::Str(value),
        ..
    }) = expr
    else {
        return Err(Error::at(expr.span(), "expected a string literal"));
    };
    Ok(value.value())
}

fn unquote(value: &str) -> String {
    value.trim_matches('"').to_owned()
}

fn lua_key(key: &str) -> String {
    if is_identifier(key) {
        key.to_owned()
    } else {
        format!("[{}]", quote(key))
    }
}

fn strip_reference(expr: &Expr) -> &Expr {
    match expr {
        Expr::Reference(reference) => strip_reference(&reference.expr),
        Expr::Paren(paren) => strip_reference(&paren.expr),
        Expr::Group(group) => strip_reference(&group.expr),
        other => other,
    }
}

type IfChain<'a> = (Vec<(&'a Expr, &'a syn::Block)>, Option<&'a Expr>);

fn diverges(expr: &Expr) -> bool {
    match expr {
        Expr::Return(_) => true,
        Expr::Call(call) => is_ok_call(call),
        Expr::Paren(paren) => diverges(&paren.expr),
        Expr::Group(group) => diverges(&group.expr),
        _ => false,
    }
}

fn if_chain(item: &syn::ExprIf) -> IfChain<'_> {
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

fn parse_vec(mac: &ExprMacro) -> Result<Vec<Expr>, Error> {
    let list = syn::parse::Parser::parse2(
        Punctuated::<Expr, Token![,]>::parse_terminated,
        mac.mac.tokens.clone(),
    )?;
    Ok(list.into_iter().collect())
}

struct MatchesArgs {
    expr: Expr,
    pat: Pat,
}

impl Parse for MatchesArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let expr: Expr = input.parse()?;
        let _: Token![,] = input.parse()?;
        let pat: Pat = Pat::parse_multi(input)?;
        Ok(Self { expr, pat })
    }
}
struct FormatArgs {
    literal: String,
    arguments: Vec<(Option<String>, Expr)>,
}

impl Parse for FormatArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let literal: syn::LitStr = input.parse()?;
        let mut arguments = Vec::new();
        while !input.is_empty() {
            let _: Token![,] = input.parse()?;
            if input.is_empty() {
                break;
            }
            let mut name = None;
            if input.peek(syn::Ident) && input.peek2(Token![=]) {
                let ident: syn::Ident = input.parse()?;
                let _: Token![=] = input.parse()?;
                name = Some(ident.to_string());
            }
            let expr: Expr = input.parse()?;
            arguments.push((name, expr));
        }
        Ok(Self {
            literal: literal.value(),
            arguments,
        })
    }
}

enum JsonNode {
    Object(Vec<(String, JsonNode)>),
    Array(Vec<JsonNode>),
    Expr(Box<Expr>),
}

impl Parse for JsonNode {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        if input.peek(syn::token::Brace) {
            let content;
            syn::braced!(content in input);
            let mut entries = Vec::new();
            while !content.is_empty() {
                let key = if content.peek(syn::LitStr) {
                    content.parse::<syn::LitStr>()?.value()
                } else {
                    content.parse::<syn::Ident>()?.to_string()
                };
                let _: Token![:] = content.parse()?;
                let value: JsonNode = content.parse()?;
                entries.push((key, value));
                if content.peek(Token![,]) {
                    let _: Token![,] = content.parse()?;
                }
            }
            return Ok(Self::Object(entries));
        }
        if input.peek(syn::token::Bracket) {
            let content;
            syn::bracketed!(content in input);
            let mut values = Vec::new();
            while !content.is_empty() {
                values.push(content.parse()?);
                if content.peek(Token![,]) {
                    let _: Token![,] = content.parse()?;
                }
            }
            return Ok(Self::Array(values));
        }
        Ok(Self::Expr(Box::new(input.parse()?)))
    }
}

impl JsonNode {
    fn span(&self) -> proc_macro2::Span {
        match self {
            Self::Object(entries) => entries
                .first()
                .map_or_else(proc_macro2::Span::call_site, |(_, value)| {
                    value.span()
                }),
            Self::Array(values) => values
                .first()
                .map_or_else(proc_macro2::Span::call_site, |value| {
                    value.span()
                }),
            Self::Expr(expr) => expr.span(),
        }
    }
}
