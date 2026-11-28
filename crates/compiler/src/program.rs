use std::collections::{BTreeSet, HashMap};

use crate::{Error, Module};
use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::{
    Attribute, Expr, ExprLit, Ident, Item, ItemFn, ItemStatic, Lit, Meta, Pat,
    Token, Type, UseTree, punctuated::Punctuated,
};

/// A module path and the function found at that path.
pub(crate) struct Resolved<'a> {
    pub module: String,
    pub function: &'a ItemFn,
}

#[derive(Clone)]
pub(crate) struct StructDef {
    pub fields: Vec<(String, Type)>,
    pub config: Option<Vec<crate::config::ConfigField>>,
    pub derives_default: bool,
    pub derives_serialize: bool,
    pub derives_deserialize: bool,
}

/// One field of a `#[derive(State)]` struct, as seen in the source text.
#[derive(Clone)]
pub(crate) struct StateFieldDef {
    pub name: String,
    pub ty: Type,
    pub default: Option<Expr>,
    pub skip: bool,
    pub span: Span,
}

/// The schema the compiler synthesizes helpers from. Proc macros are not
/// expanded in the source the compiler reads, so helper attributes are parsed
/// directly.
#[derive(Clone)]
pub(crate) struct StateDef {
    pub key: String,
    pub fields: Vec<StateFieldDef>,
    pub error: Option<(Span, String)>,
}

struct Unit {
    functions: HashMap<String, ItemFn>,
    statics: HashMap<String, ItemStatic>,
    enums: HashMap<String, Vec<String>>,
    newtypes: BTreeSet<String>,
    tuple_structs: HashMap<String, usize>,
    structs: HashMap<String, StructDef>,
    states: HashMap<String, StateDef>,
    impls: HashMap<String, HashMap<String, ItemFn>>,
    constants: HashMap<String, Expr>,
    associated: HashMap<String, HashMap<String, Expr>>,
    imports: HashMap<String, Vec<String>>,
    globs: Vec<Vec<String>>,
}

impl Unit {
    fn new() -> Self {
        Self {
            functions: HashMap::new(),
            statics: HashMap::new(),
            enums: HashMap::new(),
            newtypes: BTreeSet::new(),
            tuple_structs: HashMap::new(),
            structs: HashMap::new(),
            states: HashMap::new(),
            impls: HashMap::new(),
            constants: HashMap::new(),
            associated: HashMap::new(),
            imports: HashMap::new(),
            globs: Vec::new(),
        }
    }
}

/// Every module in the crate, keyed by path. The crate root is the `main`
/// module when it exists, so `crate::a::b` resolves against it.
pub(crate) struct Program {
    units: HashMap<String, Unit>,
    root: String,
}

impl Program {
    pub(crate) fn new(
        module: &str,
        source: &str,
        extra: &[Module<'_>],
    ) -> Result<Self, Error> {
        let module = normalize(module);
        let mut program = Self {
            units: HashMap::new(),
            root: module.clone(),
        };
        let file = syn::parse_file(source)
            .map_err(|error| Error::from(error).with_file(&module))?;
        program.insert(&module, &file)?;
        for item in extra {
            let path = normalize(item.path);
            if program.units.contains_key(&path) {
                continue;
            }
            let file = syn::parse_file(item.source)
                .map_err(|error| Error::from(error).with_file(&path))?;
            program.insert(&path, &file)?;
        }
        program.root = if program.units.contains_key("main") {
            "main".to_owned()
        } else {
            module
        };
        Ok(program)
    }

    pub(crate) fn root(&self) -> &str {
        &self.root
    }

    fn insert(&mut self, module: &str, file: &syn::File) -> Result<(), Error> {
        let mut units = Vec::new();
        collect(&mut units, module, true, &file.items)?;
        for (path, unit) in units {
            self.units.insert(path, unit);
        }
        Ok(())
    }

    /// Resolves the handler, following `use` aliases in its module.
    pub(crate) fn handler(
        &self,
        module: &str,
        name: &str,
    ) -> Result<Resolved<'_>, Error> {
        if let Some(resolved) = self.resolve_name(module, name, 0) {
            return Ok(resolved);
        }
        let mut nested = Vec::new();
        for (path, unit) in &self.units {
            if path != module
                && self.parent(path).as_deref() == Some(module)
                && let Some(function) = unit.functions.get(name)
            {
                nested.push((path, function));
            }
        }
        match nested.as_slice() {
            [(path, function)] => Ok(Resolved {
                module: (*path).clone(),
                function,
            }),
            [] => Err(Error::plain(format!(
                "no function named `{name}` in module `{module}`"
            ))),
            _ => Err(Error::plain(format!(
                "handler `{name}` is defined more than once in module `{module}`"
            ))),
        }
    }

    /// Resolves a call path in `module` to a function in the crate.
    pub(crate) fn resolve(
        &self,
        module: &str,
        segments: &[String],
    ) -> Option<Resolved<'_>> {
        self.resolve_in(module, segments, 0)
    }

    fn resolve_in(
        &self,
        module: &str,
        segments: &[String],
        depth: usize,
    ) -> Option<Resolved<'_>> {
        if depth > 32 {
            return None;
        }
        let (first, rest) = segments.split_first()?;
        match first.as_str() {
            "crate" => self.resolve_path(&self.root, rest, depth),
            "self" => self.resolve_path(module, rest, depth),
            "super" => {
                let parent = self.parent(module)?;
                self.resolve_in(&parent, rest, depth.saturating_add(1))
            }
            _ => self.resolve_path(module, segments, depth).or_else(|| {
                (module != self.root)
                    .then(|| self.resolve_path(&self.root, segments, depth))
                    .flatten()
            }),
        }
    }

    fn resolve_path(
        &self,
        base: &str,
        segments: &[String],
        depth: usize,
    ) -> Option<Resolved<'_>> {
        if depth > 32 {
            return None;
        }
        let (first, rest) = segments.split_first()?;
        if rest.is_empty() {
            return self.resolve_name(base, first, depth.saturating_add(1));
        }
        let child = self.child(base, first);
        if self.units.contains_key(&child) {
            return self.resolve_path(&child, rest, depth.saturating_add(1));
        }
        let unit = self.units.get(base)?;
        if let Some(target) = unit.imports.get(first) {
            let mut expanded = target.clone();
            expanded.extend_from_slice(rest);
            return self.resolve_in(base, &expanded, depth.saturating_add(1));
        }
        None
    }

    fn resolve_name(
        &self,
        module: &str,
        name: &str,
        depth: usize,
    ) -> Option<Resolved<'_>> {
        if depth > 32 {
            return None;
        }
        let unit = self.units.get(module)?;
        if let Some(function) = unit.functions.get(name) {
            return Some(Resolved {
                module: module.to_owned(),
                function,
            });
        }
        if let Some(target) = unit.imports.get(name) {
            return self.resolve_in(module, target, depth.saturating_add(1));
        }
        let mut found = Vec::new();
        for glob in &unit.globs {
            let mut expanded = glob.clone();
            expanded.push(name.to_owned());
            if let Some(resolved) =
                self.resolve_in(module, &expanded, depth.saturating_add(1))
            {
                found.push(resolved);
            }
        }
        match found.as_slice() {
            [resolved] => Some(Resolved {
                module: resolved.module.clone(),
                function: resolved.function,
            }),
            [first, rest @ ..]
                if rest.iter().all(|resolved| {
                    resolved.module == first.module
                        && std::ptr::eq(resolved.function, first.function)
                }) =>
            {
                Some(Resolved {
                    module: first.module.clone(),
                    function: first.function,
                })
            }
            _ => None,
        }
    }

    fn child(&self, base: &str, relative: &str) -> String {
        if base == self.root {
            relative.to_owned()
        } else {
            format!("{base}::{relative}")
        }
    }

    fn parent(&self, module: &str) -> Option<String> {
        if let Some((parent, _)) = module.rsplit_once("::") {
            return Some(parent.to_owned());
        }
        (module != self.root).then(|| self.root.clone())
    }

    pub(crate) fn constants(
        &self,
        module: &str,
    ) -> impl Iterator<Item = (&str, &Expr)> {
        self.units
            .get(module)
            .into_iter()
            .flat_map(|unit| unit.constants.iter())
            .map(|(name, value)| (name.as_str(), value))
    }

    /// Resolves a constant imported into `module` with a plain `use`.
    pub(crate) fn imported_constant(
        &self,
        module: &str,
        name: &str,
    ) -> Option<&Expr> {
        let unit = self.units.get(module)?;
        if let Some(target) = unit.imports.get(name) {
            return self.resolve_constant(module, target);
        }
        for glob in &unit.globs {
            let mut expanded = glob.clone();
            expanded.push(name.to_owned());
            if let Some(value) = self.resolve_constant(module, &expanded) {
                return Some(value);
            }
        }
        let mut found = None;
        for unit in self.units.values() {
            let Some(value) = unit.constants.get(name) else {
                continue;
            };
            if found.is_some() {
                return None;
            }
            found = Some(value);
        }
        found
    }

    /// Resolves a constant path such as `crate::common::STATE`.
    pub(crate) fn resolve_constant(
        &self,
        module: &str,
        segments: &[String],
    ) -> Option<&Expr> {
        self.constant_in(module, segments, 0)
    }

    fn constant_in(
        &self,
        module: &str,
        segments: &[String],
        depth: usize,
    ) -> Option<&Expr> {
        if depth > 32 {
            return None;
        }
        let (first, rest) = segments.split_first()?;
        match first.as_str() {
            "crate" => self.constant_from(&self.root, rest, depth),
            "self" => self.constant_from(module, rest, depth),
            "super" => {
                let parent = self.parent(module)?;
                self.constant_in(&parent, rest, depth.saturating_add(1))
            }
            _ => self.constant_from(module, segments, depth).or_else(|| {
                (module != self.root)
                    .then(|| self.constant_from(&self.root, segments, depth))
                    .flatten()
            }),
        }
    }

    fn constant_from(
        &self,
        base: &str,
        segments: &[String],
        depth: usize,
    ) -> Option<&Expr> {
        if depth > 32 {
            return None;
        }
        let (first, rest) = segments.split_first()?;
        if rest.is_empty() {
            let unit = self.units.get(base)?;
            if let Some(value) = unit.constants.get(first) {
                return Some(value);
            }
            if let Some(target) = unit.imports.get(first) {
                return self.constant_in(base, target, depth.saturating_add(1));
            }
            return None;
        }
        let child = self.child(base, first);
        self.constant_from(&child, rest, depth.saturating_add(1))
    }
}

impl Program {
    /// Whether any `impl` block in the crate defines `name`. A method whose
    /// receiver type the compiler lost is only treated as a `std` call when the
    /// crate could not have defined it.
    pub(crate) fn defines_method(&self, name: &str) -> bool {
        self.units.values().any(|unit| {
            unit.impls
                .values()
                .any(|methods| methods.contains_key(name))
        })
    }

    pub(crate) fn resolve_method(
        &self,
        module: &str,
        class: &str,
        name: &str,
    ) -> Option<(String, &ItemFn)> {
        let mut found = Vec::new();
        for (path, unit) in &self.units {
            if let Some(function) =
                unit.impls.get(class).and_then(|methods| methods.get(name))
            {
                found.push((path, function));
            }
        }
        if let Some(index) =
            found.iter().position(|(path, _)| path.as_str() == module)
        {
            return Some((found[index].0.clone(), found[index].1));
        }
        match found.as_slice() {
            [(path, function)] => Some(((*path).to_owned(), *function)),
            _ => None,
        }
    }
}

impl Program {
    pub(crate) fn resolve_static(
        &self,
        module: &str,
        segments: &[String],
    ) -> Option<(String, &ItemStatic)> {
        self.static_in(module, segments, 0)
    }

    fn static_in(
        &self,
        module: &str,
        segments: &[String],
        depth: usize,
    ) -> Option<(String, &ItemStatic)> {
        if depth > 32 {
            return None;
        }
        let (first, rest) = segments.split_first()?;
        match first.as_str() {
            "crate" => self.static_from(&self.root.clone(), rest, depth),
            "self" => self.static_from(module, rest, depth),
            "super" => {
                let parent = self.parent(module)?;
                self.static_in(&parent, rest, depth.saturating_add(1))
            }
            _ => self.static_from(module, segments, depth).or_else(|| {
                (module != self.root)
                    .then(|| {
                        self.static_from(&self.root.clone(), segments, depth)
                    })
                    .flatten()
            }),
        }
    }

    fn static_from(
        &self,
        module: &str,
        segments: &[String],
        depth: usize,
    ) -> Option<(String, &ItemStatic)> {
        if depth > 32 {
            return None;
        }
        let (first, rest) = segments.split_first()?;
        if rest.is_empty() {
            let unit = self.units.get(module)?;
            if let Some(item) = unit.statics.get(first) {
                return Some((module.to_owned(), item));
            }
            if let Some(target) = unit.imports.get(first) {
                return self.static_in(module, target, depth.saturating_add(1));
            }
            return None;
        }
        let child = self.child(module, first);
        if self.units.contains_key(&child) {
            return self.static_from(&child, rest, depth.saturating_add(1));
        }
        let unit = self.units.get(module)?;
        let target = unit.imports.get(first)?;
        let mut expanded = target.clone();
        expanded.extend_from_slice(rest);
        self.static_in(module, &expanded, depth.saturating_add(1))
    }
}

fn normalize(path: &str) -> String {
    let trimmed = path.trim().trim_start_matches("crate::");
    let trimmed = trimmed.trim_end_matches("::");
    if trimmed.is_empty() {
        "main".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn collect(
    units: &mut Vec<(String, Unit)>,
    module: &str,
    is_root: bool,
    items: &[Item],
) -> Result<(), Error> {
    let mut unit = Unit::new();
    let mut nested = Vec::new();
    for item in items {
        match item {
            Item::Fn(function) => {
                unit.functions
                    .insert(function.sig.ident.to_string(), function.clone());
            }
            Item::Const(item_const) => {
                unit.constants.insert(
                    item_const.ident.to_string(),
                    (*item_const.expr).clone(),
                );
            }
            Item::Use(item_use) => {
                collect_use(&mut unit, &item_use.tree, &mut Vec::new());
            }
            Item::Mod(item_mod) => {
                if let Some((_, inner)) = &item_mod.content {
                    let name = item_mod.ident.to_string();
                    let path = if is_root {
                        name
                    } else {
                        format!("{module}::{name}")
                    };
                    collect(&mut nested, &path, false, inner)?;
                }
            }
            Item::Static(item_static) => {
                unit.statics
                    .insert(item_static.ident.to_string(), item_static.clone());
            }
            Item::Struct(item_struct) => {
                if let syn::Fields::Named(fields) = &item_struct.fields {
                    let config = crate::config::parse_struct(item_struct)
                        .map(|def| def.map(|def| def.fields))
                        .map_err(Error::from)?;
                    let derives = derives(&item_struct.attrs);
                    let derives_default = derives.contains("Default");
                    let derives_serialize = derives.contains("Serialize");
                    let derives_deserialize = derives.contains("Deserialize");
                    let fields = fields
                        .named
                        .iter()
                        .filter_map(|field| {
                            field.ident.as_ref().map(|name| {
                                (name.to_string(), field.ty.clone())
                            })
                        })
                        .collect();
                    unit.structs.insert(
                        item_struct.ident.to_string(),
                        StructDef {
                            fields,
                            config,
                            derives_default,
                            derives_serialize,
                            derives_deserialize,
                        },
                    );
                    if derives.contains("State") {
                        let def = state_def(
                            &item_struct.attrs,
                            &item_struct.ident,
                            &item_struct.fields,
                        );
                        unit.states.insert(item_struct.ident.to_string(), def);
                    }
                } else if let syn::Fields::Unnamed(fields) = &item_struct.fields
                    && fields.unnamed.len() == 1
                {
                    unit.newtypes.insert(item_struct.ident.to_string());
                } else if let syn::Fields::Unnamed(fields) = &item_struct.fields
                {
                    unit.tuple_structs.insert(
                        item_struct.ident.to_string(),
                        fields.unnamed.len(),
                    );
                }
            }
            Item::Enum(item_enum) => {
                let variants = item_enum
                    .variants
                    .iter()
                    .map(|variant| variant.ident.to_string())
                    .collect();
                unit.enums.insert(item_enum.ident.to_string(), variants);
            }
            Item::Impl(item_impl) => {
                if let Some(name) = impl_name(&item_impl.self_ty) {
                    for item in &item_impl.items {
                        if let syn::ImplItem::Const(value) = item {
                            unit.associated
                                .entry(name.clone())
                                .or_default()
                                .insert(
                                    value.ident.to_string(),
                                    value.expr.clone(),
                                );
                        }
                    }
                    let methods = unit.impls.entry(name).or_default();
                    for item in &item_impl.items {
                        if let syn::ImplItem::Fn(function) = item {
                            methods.insert(
                                function.sig.ident.to_string(),
                                ItemFn {
                                    attrs: function.attrs.clone(),
                                    modifiers: syn::FnModifiers::default(),
                                    vis: function.vis.clone(),
                                    sig: function.sig.clone(),
                                    block: Box::new(function.block.clone()),
                                },
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
    units.push((module.to_owned(), unit));
    units.append(&mut nested);
    Ok(())
}

fn derives(attrs: &[syn::Attribute]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for attr in attrs {
        let syn::Meta::List(list) = &attr.meta else {
            continue;
        };
        if !list.path.is_ident("derive") {
            continue;
        }
        for name in list.tokens.to_string().split(',') {
            let name = name.trim();
            let name = name.rsplit("::").next().unwrap_or(name);
            if !name.is_empty() {
                names.insert(name.to_owned());
            }
        }
    }
    names
}

fn state_def(
    attrs: &[Attribute],
    name: &Ident,
    fields: &syn::Fields,
) -> StateDef {
    parse_state_def(attrs, name, fields).unwrap_or_else(|error| StateDef {
        key: String::new(),
        fields: Vec::new(),
        error: Some((error.span(), error.to_string())),
    })
}

fn parse_state_def(
    attrs: &[Attribute],
    name: &Ident,
    fields: &syn::Fields,
) -> Result<StateDef, syn::Error> {
    let mut key = snake_case(&name.to_string());
    for attr in state_attrs(attrs) {
        for meta in state_metas(attr)? {
            if let Meta::NameValue(value) = &meta
                && value.path.is_ident("key")
            {
                match &value.value {
                    Expr::Lit(ExprLit {
                        lit: Lit::Str(text),
                        ..
                    }) => key = text.value(),
                    other => {
                        return Err(syn::Error::new(
                            other.span(),
                            "state key must be a string literal",
                        ));
                    }
                }
            } else {
                return Err(syn::Error::new_spanned(
                    meta,
                    "unknown state struct option",
                ));
            }
        }
    }
    let syn::Fields::Named(named) = fields else {
        return Err(syn::Error::new(
            fields.span(),
            "State requires named fields",
        ));
    };
    let mut parsed = Vec::new();
    for field in &named.named {
        let ident = field.ident.clone().ok_or_else(|| {
            syn::Error::new(field.span(), "State requires a named field")
        })?;
        let mut default = None;
        let mut skip = false;
        for attr in state_attrs(&field.attrs) {
            for meta in state_metas(attr)? {
                match meta {
                    Meta::Path(path) if path.is_ident("skip") => skip = true,
                    Meta::NameValue(value)
                        if value.path.is_ident("default") =>
                    {
                        default = Some(value.value);
                    }
                    other => {
                        return Err(syn::Error::new_spanned(
                            other,
                            "unknown state field option",
                        ));
                    }
                }
            }
        }
        parsed.push(StateFieldDef {
            name: ident.to_string(),
            ty: field.ty.clone(),
            default,
            skip,
            span: field.span(),
        });
    }
    Ok(StateDef {
        key,
        fields: parsed,
        error: None,
    })
}

fn state_attrs(attrs: &[Attribute]) -> impl Iterator<Item = &Attribute> {
    attrs.iter().filter(|attr| attr.path().is_ident("state"))
}

fn state_metas(attr: &Attribute) -> Result<Vec<Meta>, syn::Error> {
    Ok(attr
        .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?
        .into_iter()
        .collect())
}

fn snake_case(name: &str) -> String {
    let mut out = String::new();
    let chars = name.chars().collect::<Vec<_>>();
    for (index, character) in chars.iter().enumerate() {
        if character.is_ascii_uppercase() {
            let previous_lower =
                index > 0 && chars[index - 1].is_ascii_lowercase();
            let next_lower = chars
                .get(index + 1)
                .is_some_and(|next| next.is_ascii_lowercase());
            if index > 0 && (previous_lower || next_lower) {
                out.push('_');
            }
            out.push(character.to_ascii_lowercase());
        } else {
            out.push(*character);
        }
    }
    out
}

fn collect_use(unit: &mut Unit, tree: &UseTree, prefix: &mut Vec<String>) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_use(unit, &path.tree, prefix);
            prefix.pop();
        }
        UseTree::Name(name) => {
            let name = name.ident.to_string();
            if name == "self" {
                return;
            }
            let mut target = prefix.clone();
            target.push(name.clone());
            unit.imports.insert(name, target);
        }
        UseTree::Rename(rename) => {
            let mut target = prefix.clone();
            target.push(rename.ident.to_string());
            unit.imports.insert(rename.rename.to_string(), target);
        }
        UseTree::Group(group) => {
            for item in &group.items {
                collect_use(unit, item, prefix);
            }
        }
        UseTree::Glob(_) => {
            if !prefix.is_empty() {
                unit.globs.push(prefix.clone());
            }
        }
    }
}

pub(crate) fn typed_args(function: &ItemFn) -> Vec<(&Pat, &syn::Type)> {
    use syn::FnArg;

    function
        .sig
        .inputs
        .iter()
        .filter_map(|arg| {
            let FnArg::Typed(typed) = arg else {
                return None;
            };
            Some((&*typed.pat, &*typed.ty))
        })
        .collect()
}

fn impl_name(ty: &Type) -> Option<String> {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        Type::Reference(reference) => impl_name(&reference.elem),
        Type::Paren(paren) => impl_name(&paren.elem),
        Type::Group(group) => impl_name(&group.elem),
        _ => None,
    }
}

impl Program {
    pub(crate) fn struct_def(
        &self,
        module: &str,
        name: &str,
    ) -> Option<&StructDef> {
        if let Some(def) = self.units.get(module)?.structs.get(name) {
            return Some(def);
        }
        let mut found = None;
        for unit in self.units.values() {
            if let Some(def) = unit.structs.get(name) {
                if found.is_some() {
                    return None;
                }
                found = Some(def);
            }
        }
        found
    }

    /// Config fields for a struct that derives `Config`.
    pub(crate) fn config_fields(
        &self,
        module: &str,
        name: &str,
    ) -> Option<&[crate::config::ConfigField]> {
        self.struct_def(module, name)?.config.as_deref()
    }

    /// Finds the schema of a `#[derive(State)]` struct by type name.
    pub(crate) fn state_def(
        &self,
        module: &str,
        name: &str,
    ) -> Option<&StateDef> {
        if let Some(def) = self.units.get(module)?.states.get(name) {
            return Some(def);
        }
        let mut found = None;
        for unit in self.units.values() {
            if let Some(def) = unit.states.get(name) {
                if found.is_some() {
                    return None;
                }
                found = Some(def);
            }
        }
        found
    }

    /// Resolves an associated constant such as `State::EMPTY`.
    pub(crate) fn associated_const(
        &self,
        module: &str,
        class: &str,
        name: &str,
    ) -> Option<&Expr> {
        if let Some(value) = self
            .units
            .get(module)
            .and_then(|unit| unit.associated.get(class))
            .and_then(|values| values.get(name))
        {
            return Some(value);
        }
        let mut found = None;
        for unit in self.units.values() {
            let Some(value) = unit
                .associated
                .get(class)
                .and_then(|values| values.get(name))
            else {
                continue;
            };
            if found.is_some() {
                return None;
            }
            found = Some(value);
        }
        found
    }

    pub(crate) fn tuple_struct_len(
        &self,
        module: &str,
        name: &str,
    ) -> Option<usize> {
        self.units
            .get(module)
            .and_then(|unit| unit.tuple_structs.get(name).copied())
            .or_else(|| {
                self.units
                    .values()
                    .find_map(|unit| unit.tuple_structs.get(name).copied())
            })
    }

    pub(crate) fn is_newtype(&self, module: &str, name: &str) -> bool {
        self.units
            .get(module)
            .is_some_and(|unit| unit.newtypes.contains(name))
            || self.units.values().any(|unit| unit.newtypes.contains(name))
    }
    pub(crate) fn enum_variant(
        &self,
        module: &str,
        segments: &[String],
    ) -> Option<String> {
        let unit = self.units.get(module)?;
        for (name, variants) in &unit.enums {
            if segments.iter().any(|part| part == name)
                && let Some(variant) = segments.last()
                && variants.contains(variant)
            {
                return Some(variant.clone());
            }
        }
        for unit in self.units.values() {
            for (name, variants) in &unit.enums {
                if segments.iter().any(|part| part == name)
                    && let Some(variant) = segments.last()
                    && variants.contains(variant)
                {
                    return Some(variant.clone());
                }
            }
        }
        None
    }
}
