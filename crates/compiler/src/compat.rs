//! Strict-mode guard for helper-era APIs.
//!
//! Strict entries run as one Luau script. Anything that needs the helper
//! process, a native thread, or a raw address has no Luau meaning, so the
//! compiler rejects it at the source position instead of emitting code that
//! only works with the bridge.

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{ItemExternCrate, ItemUse, UseTree};

use crate::Error;

const TYPES: &[&str] = &[
    "compat",
    "Parts",
    "OpSink",
    "ClockBudget",
    "Coroutine",
    "CoroutinePoll",
];
const FUNCTIONS: &[&str] = &[
    "yield_now",
    "yield_value",
    "run_budgeted",
    "from_raw_parts",
    "from_raw_parts_mut",
];
const METHODS: &[&str] = &[
    "set_op_sink",
    "take_pending_ops",
    "as_ptr",
    "as_mut_ptr",
    "into_raw",
];

pub(crate) fn reject(
    source: &str,
    module: &str,
    primary: bool,
) -> Result<(), Error> {
    let file = syn::parse_file(source)
        .map_err(|error| Error::from(error).with_file(module))?;
    let mut collector = AliasCollector::default();
    collector.visit_file(&file);
    let mut checker = Checker {
        module,
        primary,
        aliases: collector.aliases,
        error: None,
    };
    checker.visit_file(&file);
    match checker.error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[derive(Default)]
struct Aliases {
    crates: Vec<String>,
    compat: Vec<String>,
    threads: Vec<String>,
}

#[derive(Default)]
struct AliasCollector {
    aliases: Aliases,
}

impl AliasCollector {
    fn walk(&mut self, tree: &UseTree, prefix: &[String]) {
        match tree {
            UseTree::Path(item) => {
                let mut path = prefix.to_vec();
                path.push(item.ident.to_string());
                self.walk(&item.tree, &path);
            }
            UseTree::Name(item) => self.record(&item.ident, None, prefix),
            UseTree::Rename(item) => {
                self.record(&item.ident, Some(&item.rename), prefix);
            }
            UseTree::Group(group) => {
                for item in &group.items {
                    self.walk(item, prefix);
                }
            }
            UseTree::Glob(_) => {}
        }
    }

    fn record(
        &mut self,
        ident: &syn::Ident,
        rename: Option<&syn::Ident>,
        prefix: &[String],
    ) {
        let name = rename
            .map_or_else(|| ident.to_string(), |rename| rename.to_string());
        match prefix {
            [] if ident == "noctalia_rs" => self.aliases.crates.push(name),
            [root] if root == "noctalia_rs" && ident == "compat" => {
                self.aliases.compat.push(name);
            }
            [root] if root == "std" && ident == "thread" => {
                self.aliases.threads.push(name);
            }
            _ => {}
        }
    }
}

impl<'ast> Visit<'ast> for AliasCollector {
    fn visit_item_use(&mut self, node: &'ast ItemUse) {
        self.walk(&node.tree, &[]);
    }

    fn visit_item_extern_crate(&mut self, node: &'ast ItemExternCrate) {
        if node.ident == "noctalia_rs"
            && let Some((_, rename)) = &node.rename
        {
            self.aliases.crates.push(rename.to_string());
        }
    }
}

struct Checker<'a> {
    module: &'a str,
    primary: bool,
    aliases: Aliases,
    error: Option<Error>,
}

impl Checker<'_> {
    fn normalize(&self, segments: &[String]) -> Vec<String> {
        let Some(first) = segments.first() else {
            return Vec::new();
        };
        if self.aliases.compat.iter().any(|alias| alias == first) {
            let mut out = vec!["noctalia_rs".to_owned(), "compat".to_owned()];
            out.extend(segments.iter().skip(1).cloned());
            return out;
        }
        if self.aliases.threads.iter().any(|alias| alias == first) {
            let mut out = vec!["std".to_owned(), "thread".to_owned()];
            out.extend(segments.iter().skip(1).cloned());
            return out;
        }
        segments
            .iter()
            .map(|segment| {
                if self.aliases.crates.iter().any(|alias| alias == segment) {
                    "noctalia_rs".to_owned()
                } else {
                    segment.clone()
                }
            })
            .collect()
    }

    fn rooted(segments: &[String], root: &str) -> bool {
        segments.first().is_some_and(|segment| segment == root)
    }

    fn from_compat(segments: &[String]) -> bool {
        segments
            .windows(2)
            .any(|pair| pair[0] == "noctalia_rs" && pair[1] == "compat")
    }

    fn fail(&mut self, span: Span, name: &str) {
        if self.error.is_some() {
            return;
        }
        let message = format!(
            "`{name}` needs the helper runtime and cannot run in strict mode; use `noctalia_rs::compat` with `auto` or `off`"
        );
        let error = Error::at(span, message);
        self.error = Some(if self.primary {
            error
        } else {
            error.with_file(self.module)
        });
    }

    fn check_segments(&mut self, segments: &[String], span: Span) {
        let segments = self.normalize(segments);
        if Self::from_compat(&segments) {
            self.fail(span, "noctalia_rs::compat");
            return;
        }
        if segments
            .windows(2)
            .any(|pair| pair[0] == "std" && pair[1] == "ptr")
        {
            self.fail(span, "std::ptr");
            return;
        }
        let Some(last) = segments.last() else {
            return;
        };
        let thread_yield = Self::rooted(&segments, "std")
            && segments.get(1).is_some_and(|part| part == "thread")
            && last == "yield_now";
        let legacy_api = Self::rooted(&segments, "noctalia_rs")
            && (TYPES.contains(&last.as_str())
                || FUNCTIONS.contains(&last.as_str()));
        if thread_yield || legacy_api || last == "NonNull" {
            self.fail(span, last);
        }
    }

    fn check_import(&mut self, ident: &syn::Ident, prefix: &[String]) {
        let prefix = self.normalize(prefix);
        let name = ident.to_string();
        let thread_yield = prefix.len() == 2
            && prefix[0] == "std"
            && prefix[1] == "thread"
            && name == "yield_now";
        let legacy_api = Self::rooted(&prefix, "noctalia_rs")
            && (Self::from_compat(&prefix)
                || TYPES.contains(&name.as_str())
                || FUNCTIONS.contains(&name.as_str()));
        if thread_yield || legacy_api {
            self.fail(ident.span(), &name);
        }
    }

    fn check_use(&mut self, tree: &UseTree, prefix: &[String]) {
        match tree {
            UseTree::Path(item) => {
                let mut path = prefix.to_vec();
                path.push(item.ident.to_string());
                self.check_use(&item.tree, &path);
            }
            UseTree::Name(item) => self.check_import(&item.ident, prefix),
            UseTree::Rename(item) => self.check_import(&item.ident, prefix),
            UseTree::Group(group) => {
                for item in &group.items {
                    self.check_use(item, prefix);
                }
            }
            UseTree::Glob(item) => {
                let prefix = self.normalize(prefix);
                if Self::from_compat(&prefix) {
                    self.fail(item.span(), "noctalia_rs::compat::*");
                }
            }
        }
    }
}

impl<'ast> Visit<'ast> for Checker<'_> {
    fn visit_item_use(&mut self, node: &'ast ItemUse) {
        self.check_use(&node.tree, &[]);
    }

    fn visit_path(&mut self, node: &'ast syn::Path) {
        let segments = node
            .segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect::<Vec<_>>();
        self.check_segments(&segments, node.span());
        visit::visit_path(self, node);
    }

    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        let name = node.method.to_string();
        if METHODS.contains(&name.as_str()) {
            self.fail(node.method.span(), &name);
        }
        visit::visit_expr_method_call(self, node);
    }

    fn visit_type(&mut self, node: &'ast syn::Type) {
        if let syn::Type::Ptr(pointer) = node {
            self.fail(pointer.span(), "raw pointer");
        }
        visit::visit_type(self, node);
    }

    fn visit_expr_raw_addr(&mut self, node: &'ast syn::ExprRawAddr) {
        self.fail(node.span(), "raw address");
        visit::visit_expr_raw_addr(self, node);
    }
}
