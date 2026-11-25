//! Luau emission, split by concern.
//!
//! `mod.rs` holds `Emitter`, `Body` and the output buffer. `statements.rs`
//! lowers statements and patterns, `expressions.rs` expressions,
//! `methods.rs` method calls, `calls.rs` call forms, `macros.rs` macro
//! lowering, `ops.rs` `Op` construction, `native.rs` the native boundary,
//! `inline.rs` helper inlining, `constants.rs` constants and statics, and
//! `types.rs` the type model.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    BinOp, Expr, ExprBinary, ExprCall, ExprLit, ExprMacro, ExprMethodCall,
    ItemFn, Lit, Member, Pat, PathArguments, Stmt, Token, Type, UnOp,
};

use crate::program::{Program, typed_args};
use crate::{
    Error, NativeFn, NativeMode, NativeParam, NativeParamKind, NativePass,
    is_context, is_identifier, is_luau_keyword, quote, sanitize,
};

mod calls;
mod constants;
mod expressions;
mod inline;
mod macros;
mod methods;
mod native;
mod ops;
mod statements;
mod types;

#[allow(unused_imports)]
pub(crate) use {
    calls::*, expressions::*, inline::*, macros::*, methods::*, native::*,
    ops::*, statements::*, types::*,
};

impl<'a> Emitter<'a> {
    pub(crate) fn new(
        ctx: &syn::Ident,
        bound_event: Option<String>,
        strings: &[&str],
        payload_params: &'static [PayloadParam],
        program: &'a Program,
        module: &str,
        native_mode: NativeMode,
        known_native: &'a BTreeMap<String, NativeFn>,
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
            uses_query: false,
            uses_join: false,
            uses_bool_or: false,
            uses_replace: false,
            uses_source: false,
            emitted: false,
            temp: 0,
            native_mode,
            known_native,
            native_found: BTreeMap::new(),
            uses_native: false,
            uses_native_context: false,
            deferred: 0,
        }
    }

    pub(super) fn statement_block(
        &mut self,
        block: &syn::Block,
    ) -> Result<(), Error> {
        self.scopes.push(HashMap::new());
        self.local_statics.push(HashMap::new());
        let result = self.block_inner(block, false);
        self.local_statics.pop();
        self.scopes.pop();
        result
    }

    pub(crate) fn block(&mut self, block: &syn::Block) -> Result<(), Error> {
        self.scopes.push(HashMap::new());
        self.local_statics.push(HashMap::new());
        let result = self.block_inner(block, true);
        self.local_statics.pop();
        self.scopes.pop();
        result
    }

    pub(super) fn block_inner(
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
            uses_query: self.uses_query,
            uses_join: self.uses_join,
            uses_bool_or: self.uses_bool_or,
            uses_replace: self.uses_replace,
            uses_source: self.uses_source,
            params: self
                .payload_params
                .iter()
                .filter(|(_, body_param, _)| self.params.contains(body_param))
                .map(|(_, body_param, _)| *body_param)
                .collect(),
            is_empty: !self.emitted,
            uses_native: self.uses_native,
            uses_native_context: self.uses_native_context,
            native: self.native_found.into_values().collect(),
            statics: self
                .statics
                .values()
                .map(|def| (def.name.clone(), def.init.clone()))
                .collect(),
        }
    }

    pub(super) fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.out.push_str("  ");
        }
        self.out.push_str(text);
        self.out.push('\n');
    }

    pub(super) fn push(&mut self, name: &str, ty: Ty) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_owned(), ty);
        }
    }

    pub(super) fn lookup(&self, name: &str) -> Option<Ty> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    pub(super) fn temp(&mut self) -> String {
        self.temp = self.temp.saturating_add(1);
        format!("__t{}", self.temp)
    }
}

pub(crate) type PayloadParam = (&'static str, &'static str, &'static str);

pub(crate) type Constant = (String, Ty);

pub(crate) type Constants = HashMap<String, Constant>;

pub(crate) type LocalStatics = Vec<HashMap<String, (String, StaticKind)>>;

pub(crate) type PatternBindings = Vec<(String, String)>;

pub(crate) type OptionalValue = Option<(String, Ty)>;

pub(crate) type OptionalStatic = Option<(String, StaticKind)>;

pub(crate) type Inlined = Option<(String, Ty)>;

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
    pub uses_query: bool,
    pub uses_join: bool,
    pub uses_bool_or: bool,
    pub uses_replace: bool,
    pub uses_source: bool,
    pub params: Vec<&'static str>,
    pub is_empty: bool,
    pub uses_native: bool,
    pub uses_native_context: bool,
    pub native: Vec<NativeFn>,
    pub statics: Vec<(String, String)>,
}

#[derive(Clone)]
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
    uses_query: bool,
    uses_join: bool,
    uses_bool_or: bool,
    uses_replace: bool,
    uses_source: bool,
    emitted: bool,
    temp: usize,
    native_mode: NativeMode,
    known_native: &'a BTreeMap<String, NativeFn>,
    native_found: BTreeMap<String, NativeFn>,
    uses_native: bool,
    uses_native_context: bool,
    deferred: usize,
}

pub(crate) const MAX_INLINE_DEPTH: usize = 8;

pub(crate) const MAX_INLINE_CALLS: usize = 200;
