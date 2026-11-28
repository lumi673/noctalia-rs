//! Name resolution for the typed frontend.
//!
//! Every binding gets an identity. When a name is declared twice in one
//! function, both declarations and every use get distinct generated names, so
//! the emitted Luau cannot conflate them. Item paths are resolved against the
//! crate's item index; unknown lowercase roots fail closed with a position.

use std::collections::{BTreeMap, HashMap};

use syn::spanned::Spanned;
use syn::visit_mut::{self, VisitMut};
use syn::{Expr, File, Item};

use crate::Error;

/// A local binding and the identity the resolver gave it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BindingInfo {
    pub name: String,
    pub id: usize,
    pub line: usize,
    pub column: usize,
}

/// What name resolution proved about one crate.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct NameReport {
    pub bindings: Vec<BindingInfo>,
    pub functions: Vec<String>,
}

#[derive(Default)]
pub(super) struct ItemIndex {
    functions: BTreeMap<String, usize>,
    constants: BTreeMap<String, usize>,
}

impl ItemIndex {
    pub(super) fn new(files: &[File]) -> Self {
        let mut index = Self::default();
        for file in files {
            let mut at: usize = 0;
            for item in &file.items {
                at = at.saturating_add(1);
                match item {
                    Item::Fn(item) => {
                        index.functions.insert(item.sig.ident.to_string(), at);
                    }
                    Item::Const(item) => {
                        index.constants.insert(item.ident.to_string(), at);
                    }
                    Item::Mod(module) => {
                        let Some((_, items)) = &module.content else {
                            continue;
                        };
                        for nested in items {
                            if let Item::Fn(item) = nested {
                                index
                                    .functions
                                    .insert(item.sig.ident.to_string(), at);
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        index
    }

    fn contains(&self, name: &str) -> bool {
        self.functions.contains_key(name) || self.constants.contains_key(name)
    }
}

pub(super) fn resolve_file(
    file: &mut File,
    index: &ItemIndex,
    report: &mut NameReport,
) -> Result<(), Error> {
    let mut counts = HashMap::new();
    {
        let mut counter = Counter {
            counts: &mut counts,
        };
        counter.visit_file_mut(file);
    }
    let mut resolver = Resolver {
        index,
        counts,
        scopes: Vec::new(),
        next: 0,
        names: BTreeMap::new(),
        report,
        error: None,
    };
    resolver.visit_file_mut(file);
    match resolver.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

struct Counter<'a> {
    counts: &'a mut HashMap<String, usize>,
}

impl Counter<'_> {
    fn count(&mut self, name: &str) {
        let entry: &mut usize = self.counts.entry(name.to_owned()).or_insert(0);
        *entry = entry.saturating_add(1);
    }
}

impl VisitMut for Counter<'_> {
    fn visit_pat_ident_mut(&mut self, node: &mut syn::PatIdent) {
        self.count(&node.ident.to_string());
        visit_mut::visit_pat_ident_mut(self, node);
    }
}

#[derive(Clone)]
struct Binding {
    id: usize,
    name: String,
}

struct Resolver<'a> {
    index: &'a ItemIndex,
    counts: HashMap<String, usize>,
    scopes: Vec<BTreeMap<String, Binding>>,
    next: usize,
    names: BTreeMap<usize, String>,
    report: &'a mut NameReport,
    error: Option<Error>,
}

impl Resolver<'_> {
    fn push(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, ident: &mut syn::Ident) {
        let name = ident.to_string();
        if name == "self" || name == "_" {
            return;
        }
        let id = self.next;
        self.next = self.next.saturating_add(1);
        let generated = if self.counts.get(&name).copied().unwrap_or(0) > 1 {
            format!("{name}__{id}")
        } else {
            name.clone()
        };
        let start = ident.span().start();
        self.report.bindings.push(BindingInfo {
            name: generated.clone(),
            id,
            line: start.line,
            column: start.column.saturating_add(1),
        });
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(
                name,
                Binding {
                    id,
                    name: generated.clone(),
                },
            );
        }
        self.names.insert(id, generated.clone());
        *ident = syn::Ident::new(&generated, ident.span());
    }

    fn lookup(&self, name: &str) -> Option<&Binding> {
        self.scopes.iter().rev().find_map(|scope| scope.get(name))
    }

    fn known_root(&self, name: &str) -> bool {
        name.starts_with(|character: char| character.is_ascii_uppercase())
            || matches!(
                name,
                "std"
                    | "crate"
                    | "self"
                    | "super"
                    | "noctalia_rs"
                    | "serde_json"
                    | "fs"
                    | "json"
                    | "format"
                    | "vec"
                    | "matches"
                    | "write"
                    | "writeln"
            )
            || self.index.contains(name)
    }

    fn function(
        &mut self,
        attrs: &[syn::Attribute],
        sig: &mut syn::Signature,
        block: &mut syn::Block,
    ) {
        if attrs.iter().any(|attr| attr.path().is_ident("luau")) {
            return;
        }
        let saved_scopes = std::mem::take(&mut self.scopes);
        let saved_names = std::mem::take(&mut self.names);
        let saved_next = self.next;
        self.push();
        for input in &mut sig.inputs {
            if let syn::FnArg::Typed(arg) = input {
                self.visit_pat_mut(&mut arg.pat);
            }
        }
        self.visit_block_mut(block);
        self.pop();
        self.scopes = saved_scopes;
        self.names = saved_names;
        self.next = saved_next;
    }
}

impl VisitMut for Resolver<'_> {
    fn visit_item_fn_mut(&mut self, node: &mut syn::ItemFn) {
        self.function(&node.attrs, &mut node.sig, &mut node.block);
    }

    fn visit_impl_item_fn_mut(&mut self, node: &mut syn::ImplItemFn) {
        self.function(&node.attrs, &mut node.sig, &mut node.block);
    }

    fn visit_block_mut(&mut self, node: &mut syn::Block) {
        self.push();
        visit_mut::visit_block_mut(self, node);
        self.pop();
    }

    fn visit_pat_ident_mut(&mut self, node: &mut syn::PatIdent) {
        if node.by_ref.is_some() || node.subpat.is_some() {
            visit_mut::visit_pat_ident_mut(self, node);
            return;
        }
        self.declare(&mut node.ident);
        visit_mut::visit_pat_ident_mut(self, node);
    }

    fn visit_expr_path_mut(&mut self, node: &mut syn::ExprPath) {
        let Some(ident) = node.path.get_ident().cloned() else {
            visit_mut::visit_expr_path_mut(self, node);
            return;
        };
        let name = ident.to_string();
        if let Some(binding) = self.lookup(&name) {
            let generated = binding.name.clone();
            let id = binding.id;
            let mut path = node.path.clone();
            if let Some(segment) = path.segments.last_mut() {
                segment.ident =
                    syn::Ident::new(&generated, segment.ident.span());
            }
            node.path = path;
            self.report.bindings.push(BindingInfo {
                name: generated,
                id,
                line: ident.span().start().line,
                column: ident.span().start().column.saturating_add(1),
            });
            return;
        }
        if self.index.functions.contains_key(&name) {
            self.report.functions.push(name.clone());
            visit_mut::visit_expr_path_mut(self, node);
            return;
        }
        if !self.known_root(&name) {
            self.error.get_or_insert(Error::at(
                ident.span(),
                format!("unresolved name `{name}` in typed frontend"),
            ));
        }
        visit_mut::visit_expr_path_mut(self, node);
    }

    fn visit_expr_mut(&mut self, node: &mut Expr) {
        if let Expr::Closure(closure) = node {
            self.push();
            for input in &mut closure.inputs {
                self.visit_pat_mut(input);
            }
            self.visit_expr_mut(&mut closure.body);
            self.pop();
            return;
        }
        visit_mut::visit_expr_mut(self, node);
    }

    fn visit_local_mut(&mut self, node: &mut syn::Local) {
        if let Some(init) = &mut node.init {
            self.visit_expr_mut(&mut init.expr);
            if let Some((_, diverge)) = &mut init.diverge {
                self.visit_expr_mut(diverge);
            }
        }
        self.visit_pat_mut(&mut node.pat);
    }

    fn visit_field_value_mut(&mut self, node: &mut syn::FieldValue) {
        if node.colon_token.is_none()
            && let syn::Member::Named(member) = &node.member
            && let Some(binding) = self.lookup(&member.to_string()).cloned()
        {
            let span = member.span();
            node.colon_token = Some(Default::default());
            if let Expr::Path(path) = &mut node.expr
                && let Some(segment) = path.path.segments.last_mut()
            {
                segment.ident = syn::Ident::new(&binding.name, span);
            }
            return;
        }
        visit_mut::visit_field_value_mut(self, node);
    }

    fn visit_field_pat_mut(&mut self, node: &mut syn::FieldPat) {
        if node.colon_token.is_none()
            && let syn::Member::Named(member) = &node.member
            && let syn::Pat::Ident(_) = node.pat.as_ref()
            && let Some(binding) = self.lookup(&member.to_string()).cloned()
        {
            let span = member.span();
            node.colon_token = Some(Default::default());
            if let syn::Pat::Ident(pattern) = &mut *node.pat {
                pattern.ident = syn::Ident::new(&binding.name, span);
            }
            return;
        }
        visit_mut::visit_field_pat_mut(self, node);
    }
}
/// Name of a plain identifier pattern, used for method parameters.
pub(super) fn pattern_name(pattern: &syn::Pat) -> Option<String> {
    match pattern {
        syn::Pat::Ident(ident) => Some(ident.ident.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(source: &str) -> NameReport {
        let mut file = syn::parse_file(source).expect("parse");
        crate::frontend::expand::expand_file(&mut file).expect("expand");
        let index = ItemIndex::new(std::slice::from_ref(&file));
        let mut report = NameReport::default();
        resolve_file(&mut file, &index, &mut report).expect("resolve");
        report
    }
    fn resolved_source(source: &str) -> String {
        let mut file = syn::parse_file(source).expect("parse");
        crate::frontend::expand::expand_file(&mut file).expect("expand");
        let index = ItemIndex::new(std::slice::from_ref(&file));
        let mut report = NameReport::default();
        resolve_file(&mut file, &index, &mut report).expect("resolve");
        quote::quote!(#file).to_string()
    }

    #[test]
    fn shadowed_bindings_get_different_identities() {
        let found = report(
            "fn helper(value: &str) -> &str { value } fn main() { let label = \"a\"; { let label = helper(label); let _ = label; } let _ = label; }",
        );
        let labels = found
            .bindings
            .iter()
            .filter(|binding| binding.name.starts_with("label"))
            .collect::<Vec<_>>();
        assert_eq!(labels.len(), 5, "{found:?}");
        let ids = labels
            .iter()
            .map(|binding| binding.id)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), 2, "{found:?}");
        assert!(found.functions.contains(&"helper".to_owned()));
    }

    #[test]
    fn unknown_lowercase_roots_fail_closed() {
        let mut file =
            syn::parse_file("fn main() { mystery(); }").expect("parse");
        let index = ItemIndex::new(std::slice::from_ref(&file));
        let mut report = NameReport::default();
        let error = resolve_file(&mut file, &index, &mut report)
            .expect_err("unknown function");
        assert!(error.message.contains("unresolved name `mystery`"));
        assert!(error.line > 0);
    }
    #[test]
    fn closures_keep_captured_and_parameter_bindings() {
        let found = report(
            "fn main() { let x = 1; let capture = || x; let call = |v| v + x; let _ = capture(); let _ = call(2); }",
        );
        let captured = found
            .bindings
            .iter()
            .filter(|binding| binding.name.starts_with('x'))
            .collect::<Vec<_>>();
        assert_eq!(captured.len(), 3, "{found:?}");
        assert!(
            captured.iter().all(|binding| binding.id == captured[0].id),
            "{found:?}"
        );
        let parameters = found
            .bindings
            .iter()
            .filter(|binding| binding.name.starts_with('v'))
            .collect::<Vec<_>>();
        assert_eq!(parameters.len(), 2, "{found:?}");
        assert!(
            parameters
                .iter()
                .all(|binding| binding.id == parameters[0].id),
            "{found:?}"
        );
    }

    #[test]
    fn shadowed_initializers_read_the_outer_binding() {
        let output = resolved_source(
            "fn helper(value: &str) -> &str { value } fn main() { let label = \"a\"; let label = helper(label); let _ = label; }",
        );
        assert!(output.contains("label__1 = helper (label__0)"), "{output}");
    }

    #[test]
    fn struct_shorthand_keeps_the_renamed_binding() {
        let output = resolved_source(
            "struct Msg { label: &'static str } fn main() { let label = \"a\"; let label = label; let msg = Msg { label }; let _ = msg; }",
        );
        assert!(output.contains("label : label__1"), "{output}");
    }

    #[test]
    fn let_else_pattern_shadowing_uses_the_outer_binding_in_else() {
        let output = resolved_source(
            "fn main() { let label = \"outer\"; let Some(label) = Some(\"inner\") else { let _ = label; return; }; let _ = label; }",
        );
        assert!(output.contains("else { let _ = label__0"), "{output}");
        assert!(output.contains("let _ = label__1"), "{output}");
    }
}
