//! Luau emission, split by concern.
//!
//! `mod.rs` holds `Emitter`, `Body` and the output buffer. `statements.rs`
//! lowers statements and patterns, `expressions.rs` expressions,
//! `methods` method calls, split there by receiver class, `calls.rs` call
//! forms, `macros.rs` macro lowering, `ops.rs` `Op` construction, `native.rs`
//! the native boundary, `inline.rs` helper inlining, `constants.rs` constants
//! and statics, and `types.rs` the type model.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::strict;
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
    StrictOptions, is_context, is_identifier, is_luau_keyword, quote, sanitize,
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
use {
    calls::*, expressions::*, inline::*, macros::*, methods::*, native::*,
    ops::*, statements::*, types::*,
};

pub(crate) use native::{native_runtime, stub_source};

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
        strict: Option<StrictOptions<'a>>,
        has_action: bool,
        has_host_callback: bool,
    ) -> Self {
        let emitter = Self {
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
            pending_null_fields: None,
            null_fields: HashMap::new(),
            tuple_types: HashMap::new(),
            tuple_expr_shapes: HashMap::new(),
            tuple_expr_text: HashMap::new(),
            last_tuple_shape: None,
            unknown_value_tuple: false,
            null_texts: std::collections::BTreeSet::new(),
            last_value_ty: None,
            newtype_types: HashMap::new(),
            value_helper: false,
            in_statement: false,
            value_needed: false,
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
            uses_deep_clone: false,
            uses_array_shape: false,
            uses_count_keys: false,
            emitted: false,
            temp: 0,
            kw_scopes: vec![HashMap::new()],
            char_scopes: vec![HashMap::new()],
            native_mode,
            known_native,
            native_found: BTreeMap::new(),
            uses_native: false,
            uses_native_context: false,
            deferred: 0,
            strict,
            values: HashMap::new(),
            actions: BTreeSet::new(),
            notify_actions: BTreeSet::new(),
            uses_config_table: false,
            uses_translations_table: false,
            uses_unpack: false,
            uses_pcall: false,
            uses_action_dispatch: false,
            uses_callback_dispatch: false,
            has_action,
            has_host_callback,
        };
        methods::reset_iter_done();
        emitter
    }

    /// Whether the entry is emitted as a standalone strict script.
    pub(super) fn strict(&self) -> bool {
        self.strict.is_some()
    }

    pub(super) fn statement_block(
        &mut self,
        block: &syn::Block,
    ) -> Result<(), Error> {
        self.push_scope();
        self.local_statics.push(HashMap::new());
        let result = self.block_inner(block, false);
        self.local_statics.pop();
        self.pop_scope();
        result
    }

    pub(crate) fn block(&mut self, block: &syn::Block) -> Result<(), Error> {
        self.push_scope();
        self.local_statics.push(HashMap::new());
        let result = self.block_inner(block, true);
        self.local_statics.pop();
        self.pop_scope();
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
        if self.uses_array_shape {
            source.push_str(&format!(
                concat!(
                    "  local function is_array(value)\n",
                    "    if type(value) ~= \"table\" then return false end\n",
                    "    local meta = getmetatable(value)\n",
                    "    if type(meta) == \"table\" and meta.{ARRAY_MARKER} then return true end\n",
                    "    local length = #value\n",
                    "    if length == 0 then return false end\n",
                    "    local count = 0\n",
                    "    for key in pairs(value) do\n",
                    "      if type(key) ~= \"number\" or key % 1 ~= 0 or key < 1 or key > length then return false end\n",
                    "      count = count + 1\n",
                    "    end\n",
                    "    return count == length\n",
                    "  end\n",
                ),
                ARRAY_MARKER = ARRAY_MARKER,
            ));
        }
        if self.uses_count_keys {
            source.push_str(concat!(
                "  local function count_keys(value)\n",
                "    local count = 0\n",
                "    for _ in pairs(value) do count = count + 1 end\n",
                "    return count\n",
                "  end\n",
            ));
        }
        if self.uses_deep_clone {
            source.push_str(concat!(
                "  local function deep_clone(value)\n",
                "    if type(value) ~= \"table\" then return value end\n",
                "    local copy = setmetatable({}, getmetatable(value))\n",
                "    for key, item in pairs(value) do copy[key] = deep_clone(item) end\n",
                "    return copy\n",
                "  end\n",
            ));
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
            actions: self.actions,
            notify_actions: self.notify_actions,
            uses_action_dispatch: self.uses_action_dispatch,
            uses_config_table: self.uses_config_table,
            uses_translations_table: self.uses_translations_table,
            uses_unpack: self.uses_unpack,
            uses_pcall: self.uses_pcall,
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
    pub(super) fn bind_name(&mut self, name: &str) -> String {
        let reserved = self
            .kw_scopes
            .iter()
            .flat_map(|scope| scope.values().cloned())
            .collect::<std::collections::BTreeSet<_>>();
        let mut suffix = 1;
        if !is_luau_keyword(name) {
            if !reserved.contains(name) {
                return name.to_owned();
            }
            let mut candidate = format!("{name}_{suffix}");
            while self
                .scopes
                .iter()
                .any(|scope| scope.contains_key(&candidate))
                || reserved.contains(&candidate)
            {
                suffix += 1;
                candidate = format!("{name}_{suffix}");
            }
            self.kw_scopes
                .last_mut()
                .expect("a scope always exists")
                .insert(name.to_owned(), candidate.clone());
            return candidate;
        }
        if let Some(existing) = self
            .kw_scopes
            .last()
            .and_then(|scope| scope.get(name))
            .cloned()
        {
            return existing;
        }
        let mut candidate = format!("__kw_{name}");
        while self
            .scopes
            .iter()
            .any(|scope| scope.contains_key(&candidate))
            || reserved.contains(&candidate)
        {
            candidate = format!("__kw_{name}_{suffix}");
            suffix += 1;
        }
        self.kw_scopes
            .last_mut()
            .expect("a scope always exists")
            .insert(name.to_owned(), candidate.clone());
        candidate
    }

    pub(super) fn local_name(&self, name: &str) -> String {
        if let Some(existing) = self
            .kw_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
        {
            return existing.clone();
        }
        if is_luau_keyword(name) {
            format!("__kw_{name}")
        } else {
            name.to_owned()
        }
    }
    pub(super) fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
        self.kw_scopes.push(HashMap::new());
        self.constant_scopes.push(HashMap::new());
        self.char_scopes.push(HashMap::new());
    }

    pub(super) fn pop_scope(&mut self) {
        self.scopes.pop();
        self.kw_scopes.pop();
        self.constant_scopes.pop();
        self.char_scopes.pop();
    }

    pub(super) fn unmark_chars(&mut self, name: &str) {
        if let Some(scope) = self.char_scopes.last_mut() {
            scope.insert(name.to_owned(), false);
        }
    }

    pub(super) fn mark_chars(&mut self, name: &str) {
        if let Some(scope) = self.char_scopes.last_mut() {
            scope.insert(name.to_owned(), true);
        }
    }

    pub(super) fn is_chars(&self, name: &str) -> bool {
        self.char_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
            .unwrap_or(false)
    }

    pub(super) fn temp(&mut self) -> String {
        self.temp = self.temp.saturating_add(1);
        format!("__t{}", self.temp)
    }
}

/// The Luau expression for an empty JSON array.
///
/// An empty table is the one container shape Luau cannot tell from an empty
/// object, so arrays carry a metatable marker that `noctalia.json.encode` and
/// `json.decode` honor. `crates/luau` keeps the same field name and asserts
/// the pair in `tests`.
pub(crate) const ARRAY_MARKER: &str = "__noctalia_array";

/// Where a list iterator keeps its position, in the table's metatable so the
/// elements stay a clean sequence.
pub(crate) const CURSOR_MARKER: &str = "__noctalia_cursor";

pub(crate) fn empty_array() -> String {
    format!("setmetatable({{}}, {{ {ARRAY_MARKER} = true }})")
}

/// The shape of a tuple value, kept so nested destructuring can carry the
/// nullness of an inner element through each step.
#[derive(Clone, PartialEq)]
pub(crate) enum TupleShape {
    Leaf(Ty),
    Tuple(Vec<TupleShape>),
}

impl TupleShape {
    pub(crate) fn any_null(&self) -> bool {
        match self {
            Self::Leaf(ty) => ty.null,
            Self::Tuple(shapes) => shapes.iter().any(Self::any_null),
        }
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
    pub actions: BTreeSet<String>,
    pub notify_actions: BTreeSet<String>,
    pub uses_action_dispatch: bool,
    pub uses_config_table: bool,
    pub uses_translations_table: bool,
    pub uses_unpack: bool,
    pub uses_pcall: bool,
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
    pending_null_fields: Option<BTreeSet<String>>,
    null_fields: HashMap<String, BTreeSet<String>>,
    tuple_types: HashMap<String, Vec<TupleShape>>,
    tuple_expr_shapes: HashMap<String, Vec<TupleShape>>,
    tuple_expr_text: HashMap<String, String>,
    last_tuple_shape: Option<Vec<TupleShape>>,
    unknown_value_tuple: bool,
    null_texts: std::collections::BTreeSet<String>,
    last_value_ty: Option<Ty>,
    newtype_types: HashMap<String, Ty>,
    value_helper: bool,
    in_statement: bool,
    value_needed: bool,
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
    uses_deep_clone: bool,
    uses_array_shape: bool,
    uses_count_keys: bool,
    emitted: bool,
    temp: usize,
    kw_scopes: Vec<HashMap<String, String>>,
    char_scopes: Vec<HashMap<String, bool>>,
    native_mode: NativeMode,
    known_native: &'a BTreeMap<String, NativeFn>,
    native_found: BTreeMap<String, NativeFn>,
    uses_native: bool,
    uses_native_context: bool,
    deferred: usize,
    strict: Option<StrictOptions<'a>>,
    values: HashMap<String, String>,
    actions: BTreeSet<String>,
    notify_actions: BTreeSet<String>,
    uses_config_table: bool,
    uses_translations_table: bool,
    uses_unpack: bool,
    uses_pcall: bool,
    uses_action_dispatch: bool,
    uses_callback_dispatch: bool,
    has_action: bool,
    has_host_callback: bool,
}

pub(crate) const MAX_INLINE_CALLS: usize = 300;
