//! Effect classification for strict emission and auto boundaries.
//!
//! Every call that can leave the generated Luau script is classified here.
//! Effects with an API 57 member stay in Luau and are emitted as a direct
//! call. Effects the frozen host cannot express become a function-level native
//! boundary in `auto`, and a positioned error in `strict`.

use std::collections::BTreeSet;

use proc_macro2::Span;
use syn::spanned::Spanned;
use syn::{Expr, Item, ItemFn, Stmt, Type};

use crate::program::Program;
use crate::strict;

/// Where an effect runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Effect {
    /// Pure Luau data and control flow. No host boundary is needed.
    Pure,
    /// An API 57 member the strict emitter calls directly.
    Api57(&'static str, &'static str),
    /// An effect the frozen API cannot express. `auto` crosses a native
    /// boundary, `strict` fails at the source position.
    Native(&'static str),
}

/// One effect the compiler cannot lower to API 57.
#[derive(Clone, Debug)]
pub(crate) struct Finding {
    pub(crate) span: Span,
    pub(crate) reason: String,
}

/// A reachable function that must run in the helper.
#[derive(Clone)]
pub(crate) struct Boundary {
    pub(crate) module: String,
    pub(crate) function: ItemFn,
    pub(crate) reason: String,
}

pub(crate) type Classified = (Vec<(String, Finding)>, Vec<Boundary>);

/// Classifies one call path against the frozen API. Callers resolve local
/// helper paths before lowering, so the table only decides host and native
/// effects.
pub(crate) fn classify_call(segments: &[String]) -> Effect {
    let path = segments.join("::");
    match path.as_str() {
        "std::fs::read_to_string" | "std::fs::read" => {
            return Effect::Api57("noctalia", "readFile");
        }
        "std::fs::write" => return Effect::Api57("noctalia", "writeFile"),
        "std::fs::create_dir_all" => {
            return Effect::Api57("noctalia", "mkdirAll");
        }
        "std::fs::remove_file" => {
            return Effect::Api57("noctalia", "removeFile");
        }
        "std::fs::rename" => return Effect::Api57("noctalia", "renameFile"),
        "std::fs::read_dir" => return Effect::Api57("noctalia", "listDir"),
        "std::fs::metadata" => return Effect::Api57("noctalia", "fileInfo"),
        "std::env::var" | "std::env::var_os" => {
            return Effect::Api57("noctalia", "getenv");
        }
        "std::time::SystemTime::now" => {
            return Effect::Api57("noctalia", "nowMs");
        }
        "serde_json::from_str" | "serde_json::from_value" => {
            return Effect::Api57("noctalia.json", "decode");
        }
        "serde_json::to_string" => {
            return Effect::Api57("noctalia.json", "encode");
        }
        "serde_json::to_string_pretty" => {
            return Effect::Native("API 57 has no pretty JSON encoder");
        }
        "std::env::temp_dir" => {
            return Effect::Native(
                "scratch space belongs in pluginDataDir, not temp_dir",
            );
        }
        "std::env::split_paths" => {
            return Effect::Native(
                "command lookup has the commandExists member, not split_paths",
            );
        }
        _ => {}
    }
    let short = segments
        .iter()
        .skip_while(|part| matches!(part.as_str(), "std" | "core"))
        .cloned()
        .collect::<Vec<_>>();
    match short.join("::").as_str() {
        "fs::read_to_string" | "fs::read" => {
            return Effect::Api57("noctalia", "readFile");
        }
        "fs::write" => return Effect::Api57("noctalia", "writeFile"),
        "fs::create_dir_all" => return Effect::Api57("noctalia", "mkdirAll"),
        "fs::remove_file" => return Effect::Api57("noctalia", "removeFile"),
        "fs::rename" => return Effect::Api57("noctalia", "renameFile"),
        "fs::read_dir" => return Effect::Api57("noctalia", "listDir"),
        "fs::metadata" => return Effect::Api57("noctalia", "fileInfo"),
        "env::var" | "env::var_os" => {
            return Effect::Api57("noctalia", "getenv");
        }
        "time::SystemTime::now" => return Effect::Api57("noctalia", "nowMs"),
        "env::temp_dir" => {
            return Effect::Native(
                "scratch space belongs in pluginDataDir, not temp_dir",
            );
        }
        "env::split_paths" => {
            return Effect::Native(
                "command lookup has the commandExists member, not split_paths",
            );
        }
        _ => {}
    }
    match short.first().map(String::as_str) {
        Some("thread") => {
            return Effect::Native("threads have no API 57 member");
        }
        Some("net") => return Effect::Native("sockets have no API 57 member"),
        Some("process") => {
            return Effect::Native("process control has no API 57 member");
        }
        Some("io") => {
            return Effect::Native("blocking IO has no API 57 member");
        }
        Some("ptr") => {
            return Effect::Native("raw pointers have no Luau value");
        }
        _ => {}
    }
    if matches!(
        segments.last().map(String::as_str),
        Some(
            "localtime_r"
                | "strftime"
                | "setenv"
                | "system"
                | "popen"
                | "dlopen"
        )
    ) {
        return Effect::Native("FFI calls have no API 57 member");
    }
    let root = segments.first().map(String::as_str).unwrap_or_default();
    match root {
        "std" | "core" => match segments.get(1).map(String::as_str) {
            Some("thread") => {
                return Effect::Native("threads have no API 57 member");
            }
            Some("net") => {
                return Effect::Native("sockets have no API 57 member");
            }
            Some("process") => {
                return Effect::Native("process control has no API 57 member");
            }
            Some("io") => {
                return Effect::Native("blocking IO has no API 57 member");
            }
            Some("ptr") => {
                return Effect::Native("raw pointers have no Luau value");
            }
            _ => {}
        },
        "tokio" | "async_std" => {
            return Effect::Native("the async runtime has no API 57 member");
        }
        "reqwest" | "hyper" | "curl" | "ureq" | "socket" => {
            return Effect::Native("network clients have no API 57 member");
        }
        "libc" | "openssl" | "sqlite" | "rusqlite" | "mysql" | "postgres" => {
            return Effect::Native("native libraries have no API 57 member");
        }
        "noctalia_rs"
            if path.contains("coroutine") || path.contains("yield") =>
        {
            return Effect::Native(
                "coroutines are replaced by API 57 callbacks",
            );
        }
        _ => {}
    }
    Effect::Pure
}

fn classify_method(name: &str) -> Option<&'static str> {
    match name {
        "set_op_sink" | "take_pending_ops" => {
            Some("the helper op sink has no API 57 member")
        }
        "sleep" | "park" | "yield_now" => {
            Some("thread scheduling has no API 57 member")
        }
        "parts" => Some("Context::Parts has no API 57 value"),
        _ => None,
    }
}

fn classify_macro(name: &str) -> Option<&'static str> {
    match name {
        "println" | "eprintln" | "print" => {
            Some("stdout and stderr have no API 57 member")
        }
        "panic" | "todo" | "unimplemented" | "unreachable" | "assert"
        | "assert_eq" | "assert_ne" | "dbg" => {
            Some("native panics have no Luau equivalent")
        }
        "thread_local" => Some("thread-local state has no API 57 member"),
        _ => None,
    }
}

struct Scanner<'a> {
    program: &'a Program,
    module: &'a str,
    findings: Vec<Finding>,
    callees: Vec<(String, &'a ItemFn)>,
}

impl<'a> Scanner<'a> {
    fn call_path(func: &Expr) -> Option<Vec<String>> {
        let Expr::Path(path) = func else {
            return None;
        };
        if path.path.leading_colon.is_some()
            || path.path.segments.iter().any(|segment| {
                !matches!(segment.arguments, syn::PathArguments::None)
            })
        {
            return None;
        }
        Some(
            path.path
                .segments
                .iter()
                .map(|segment| segment.ident.to_string())
                .collect(),
        )
    }

    fn call(&mut self, call: &syn::ExprCall) {
        if let Some(segments) = Self::call_path(&call.func) {
            match classify_call(&segments) {
                Effect::Native(reason) => {
                    if let Some(resolved) =
                        self.program.resolve(self.module, &segments)
                    {
                        self.callees
                            .push((resolved.module.clone(), resolved.function));
                    } else {
                        self.findings.push(Finding {
                            span: call.span(),
                            reason: format!(
                                "effect `{}` has no API 57 mapping: {reason}",
                                segments.join("::")
                            ),
                        });
                    }
                }
                Effect::Api57(root, member) => {
                    let path = [member.to_owned()];
                    if strict::member(root, &path).is_none() {
                        self.findings.push(Finding {
                            span: call.span(),
                            reason: format!(
                                "effect `{}` maps to missing API 57 member `{root}.{member}`",
                                segments.join("::")
                            ),
                        });
                    }
                }
                Effect::Pure => {
                    if let Some(resolved) =
                        self.program.resolve(self.module, &segments)
                    {
                        self.callees
                            .push((resolved.module.clone(), resolved.function));
                    }
                }
            }
        }
        self.expr(&call.func);
        for argument in &call.args {
            self.expr(argument);
        }
    }

    fn method(&mut self, call: &syn::ExprMethodCall) {
        if let Some(reason) = classify_method(&call.method.to_string()) {
            self.findings.push(Finding {
                span: call.method.span(),
                reason: format!(
                    "effect `.{}()` has no API 57 mapping: {reason}",
                    call.method
                ),
            });
        }
        self.expr(&call.receiver);
        for argument in &call.args {
            self.expr(argument);
        }
    }

    fn macro_expr(&mut self, mac: &syn::ExprMacro) {
        let name = mac
            .mac
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            .unwrap_or_default();
        if let Some(reason) = classify_macro(&name) {
            self.findings.push(Finding {
                span: mac.span(),
                reason: format!(
                    "effect `{name}!` has no API 57 mapping: {reason}"
                ),
            });
        }
    }

    fn expr(&mut self, expr: &Expr) {
        match expr {
            Expr::Call(call) => self.call(call),
            Expr::MethodCall(call) => self.method(call),
            Expr::Macro(mac) => self.macro_expr(mac),
            Expr::Unsafe(unsafe_block) => {
                self.findings.push(Finding {
                    span: unsafe_block.span(),
                    reason: "unsafe blocks have no Luau equivalent".to_owned(),
                });
                self.block(&unsafe_block.block);
            }
            Expr::Array(array) => {
                for element in &array.elems {
                    self.expr(element);
                }
            }
            Expr::Assign(assign) => {
                self.expr(&assign.left);
                self.expr(&assign.right);
            }
            Expr::Async(run) => {
                self.findings.push(Finding {
                    span: run.span(),
                    reason: "native async blocks have no API 57 member"
                        .to_owned(),
                });
                self.block(&run.block);
            }
            Expr::Await(await_expr) => {
                self.findings.push(Finding {
                    span: await_expr.span(),
                    reason: "native futures have no API 57 member".to_owned(),
                });
                self.expr(&await_expr.base);
            }
            Expr::Binary(binary) => {
                self.expr(&binary.left);
                self.expr(&binary.right);
            }
            Expr::Block(block) => self.block(&block.block),
            Expr::Break(run) => {
                if let Some(value) = &run.expr {
                    self.expr(value);
                }
            }
            Expr::Cast(cast) => self.expr(&cast.expr),
            Expr::Closure(closure) => self.expr(&closure.body),
            Expr::Continue(_)
            | Expr::Lit(_)
            | Expr::Path(_)
            | Expr::Verbatim(_) => {}
            Expr::Field(field) => self.expr(&field.base),
            Expr::ForLoop(run) => {
                self.expr(&run.expr);
                self.block(&run.body);
            }
            Expr::Group(group) => self.expr(&group.expr),
            Expr::If(run) => {
                self.expr(&run.cond);
                self.block(&run.then_branch);
                if let Some((_, other)) = &run.else_branch {
                    self.expr(other);
                }
            }
            Expr::Index(index) => {
                self.expr(&index.expr);
                self.expr(&index.index);
            }
            Expr::Let(let_expr) => self.expr(&let_expr.expr),
            Expr::Loop(run) => self.block(&run.body),
            Expr::Match(run) => {
                self.expr(&run.expr);
                for arm in &run.arms {
                    self.expr(&arm.body);
                }
            }
            Expr::Paren(paren) => self.expr(&paren.expr),
            Expr::Range(range) => {
                if let Some(start) = &range.start {
                    self.expr(start);
                }
                if let Some(end) = &range.end {
                    self.expr(end);
                }
            }
            Expr::Reference(reference) => self.expr(&reference.expr),
            Expr::Repeat(repeat) => self.expr(&repeat.expr),
            Expr::Return(run) => {
                if let Some(value) = &run.expr {
                    self.expr(value);
                }
            }
            Expr::Struct(value) => {
                for field in &value.fields {
                    self.expr(&field.expr);
                }
            }
            Expr::Try(run) => self.expr(&run.expr),
            Expr::TryBlock(run) => {
                self.findings.push(Finding {
                    span: run.span(),
                    reason: "native try blocks have no Luau equivalent"
                        .to_owned(),
                });
                self.block(&run.block);
            }
            Expr::Tuple(tuple) => {
                for element in &tuple.elems {
                    self.expr(element);
                }
            }
            Expr::Unary(unary) => self.expr(&unary.expr),
            Expr::While(run) => {
                self.expr(&run.cond);
                self.block(&run.body);
            }
            _ => {}
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Local(local) => {
                if let Some(init) = &local.init {
                    self.expr(&init.expr);
                }
            }
            Stmt::Item(item) => {
                if let Item::Fn(function) = item
                    && matches!(function.sig.safety, syn::Safety::Unsafe(_))
                {
                    self.findings.push(Finding {
                        span: function.sig.ident.span(),
                        reason: "unsafe functions have no Luau equivalent"
                            .to_owned(),
                    });
                }
            }
            Stmt::Expr(expr, _) => self.expr(expr),
            Stmt::Macro(stmt) => {
                let name = stmt
                    .mac
                    .path
                    .segments
                    .last()
                    .map(|segment| segment.ident.to_string())
                    .unwrap_or_default();
                if let Some(reason) = classify_macro(&name) {
                    self.findings.push(Finding {
                        span: stmt.span(),
                        reason: format!(
                            "effect `{name}!` has no API 57 mapping: {reason}"
                        ),
                    });
                }
            }
        }
    }

    fn block(&mut self, block: &syn::Block) {
        for stmt in &block.stmts {
            self.stmt(stmt);
        }
    }

    fn signature(&mut self, function: &ItemFn) {
        if matches!(function.sig.safety, syn::Safety::Unsafe(_)) {
            self.findings.push(Finding {
                span: function.sig.ident.span(),
                reason: "unsafe functions have no Luau equivalent".to_owned(),
            });
        }
        if function.sig.inputs.iter().any(|input| {
            matches!(input, syn::FnArg::Typed(typed) if has_raw_pointer(&typed.ty))
        })
            || matches!(&function.sig.output, syn::ReturnType::Type(_, ty) if has_raw_pointer(ty))
        {
            self.findings.push(Finding {
                span: function.sig.ident.span(),
                reason: "raw pointers have no Luau value".to_owned(),
            });
        }
        for attribute in &function.attrs {
            if attribute.path().is_ident("extern") {
                self.findings.push(Finding {
                    span: attribute.span(),
                    reason: "extern functions have no Luau equivalent"
                        .to_owned(),
                });
            }
        }
    }
}

fn has_raw_pointer(ty: &Type) -> bool {
    match ty {
        Type::Ptr(_) => true,
        Type::Reference(reference) => has_raw_pointer(&reference.elem),
        Type::Path(path) => path.path.segments.iter().any(|segment| {
            if let syn::PathArguments::AngleBracketed(arguments) =
                &segment.arguments
            {
                arguments.args.iter().any(|argument| {
                    matches!(argument, syn::GenericArgument::Type(inner) if has_raw_pointer(inner))
                })
            } else {
                false
            }
        }),
        Type::Tuple(tuple) => tuple.elems.iter().any(has_raw_pointer),
        Type::Array(array) => has_raw_pointer(&array.elem),
        Type::Slice(slice) => has_raw_pointer(&slice.elem),
        Type::Group(group) => has_raw_pointer(&group.elem),
        Type::Paren(paren) => has_raw_pointer(&paren.elem),
        _ => false,
    }
}

struct Analysis {
    findings: Vec<Finding>,
    callees: Vec<(String, ItemFn)>,
}

fn analyze(program: &Program, module: &str, function: &ItemFn) -> Analysis {
    let mut scanner = Scanner {
        program,
        module,
        findings: Vec::new(),
        callees: Vec::new(),
    };
    scanner.signature(function);
    scanner.block(&function.block);
    Analysis {
        findings: scanner.findings,
        callees: scanner
            .callees
            .into_iter()
            .map(|(module, function)| (module, function.clone()))
            .collect(),
    }
}

/// Walks the functions reachable from the handler.
///
/// A native effect in the handler itself cannot be isolated behind a function
/// boundary, so it is returned as a finding for both modes. A native effect in
/// a reachable helper becomes a boundary in `auto` and a finding in `strict`.
pub(crate) fn classify_program(
    program: &Program,
    handler_module: &str,
    handler: &ItemFn,
    strict_mode: bool,
) -> Classified {
    let mut findings = Vec::new();
    let mut boundaries = Vec::new();
    let mut seen = BTreeSet::new();
    let mut work = vec![(handler_module.to_owned(), handler.clone())];
    let mut first = true;
    while let Some((module, function)) = work.pop() {
        let key = format!("{module}::{}", function.sig.ident);
        if !seen.insert(key) {
            continue;
        }
        let analysis = analyze(program, &module, &function);
        if let Some(finding) = analysis.findings.into_iter().next() {
            if strict_mode || first {
                findings.push((module.clone(), finding));
                continue;
            }
            let start = finding.span.start();
            let reason = format!(
                "{module}:{}:{}: {}",
                start.line,
                start.column.saturating_add(1),
                finding.reason
            );
            boundaries.push(Boundary {
                module,
                function,
                reason,
            });
            continue;
        }
        first = false;
        work.extend(analysis.callees);
    }
    (findings, boundaries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(value: &str) -> Vec<String> {
        value.split("::").map(str::to_owned).collect()
    }

    #[test]
    fn maps_file_effects_to_api_57_members() {
        assert_eq!(
            classify_call(&path("std::fs::read_to_string")),
            Effect::Api57("noctalia", "readFile")
        );
        assert_eq!(
            classify_call(&path("std::env::var")),
            Effect::Api57("noctalia", "getenv")
        );
        assert_eq!(
            classify_call(&path("serde_json::to_string")),
            Effect::Api57("noctalia.json", "encode")
        );
    }

    #[test]
    fn rejects_effects_outside_the_frozen_api() {
        assert!(matches!(
            classify_call(&path("std::thread::spawn")),
            Effect::Native(_)
        ));
        assert!(matches!(
            classify_call(&path("reqwest::blocking::get")),
            Effect::Native(_)
        ));
        assert!(matches!(
            classify_call(&path("std::env::temp_dir")),
            Effect::Native(_)
        ));
        assert_eq!(
            classify_call(&path("std::fs::metadata")),
            Effect::Api57("noctalia", "fileInfo")
        );
    }
}
