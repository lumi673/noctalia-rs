//! Typed HIR frontend for strict compilation.
//!
//! This is a second frontend beside the source translator in `crate::compile`.
//! It lowers the tokens rustc parses into an expanded, name-resolved, typed
//! form before emitting Luau. The original frontend stays unchanged: existing
//! callers of `compile`, including auto and off builds, keep their output.
//!
//! The frontend proves the parts of Rust that change Luau semantics. Macro
//! calls are expanded, shadowed bindings get separate identities, receiver
//! types select one trait impl, const arithmetic is checked at the declared
//! width, and generic calls are specialized before emission. A construct the
//! frontend cannot prove is a positioned error instead of an approximation.

mod consts;
mod expand;
mod mono;
mod names;
mod types;

use quote::ToTokens;
use syn::File;

pub use mono::LoweringReport;
pub use names::{BindingInfo, NameReport};

use crate::{Compiled, Error, Module, Options};
#[cfg(test)]
use crate::{EntryKind, NativeMode, StrictOptions};

/// Which frontend compiles a handler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Frontend {
    /// The existing source translator. This is what `compile` uses.
    Source,
    /// The expanded, resolved and monomorphized HIR frontend.
    Hir,
}

/// What the typed frontend resolved for one compilation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FrontendReport {
    pub names: NameReport,
    pub lowering: LoweringReport,
}

/// Compiles with the selected frontend. `Source` is byte for byte the original
/// `compile` path; `Hir` runs the typed passes before the same emitter.
pub fn compile_with_frontend(
    source: &str,
    options: &Options<'_>,
    frontend: Frontend,
) -> Result<Compiled, Error> {
    match frontend {
        Frontend::Source => crate::compile(source, options),
        Frontend::Hir => compile_typed(source, options),
    }
}

/// Compiles through macro expansion, name resolution, const evaluation, trait
/// resolution and monomorphization.
pub fn compile_typed(
    source: &str,
    options: &Options<'_>,
) -> Result<Compiled, Error> {
    compile_typed_with_report(source, options).map(|(compiled, _)| compiled)
}

/// As `compile_typed`, with the proof of each pass attached.
pub fn compile_typed_with_report(
    source: &str,
    options: &Options<'_>,
) -> Result<(Compiled, FrontendReport), Error> {
    let mut files = Vec::with_capacity(options.modules.len().saturating_add(1));
    files.push(parse(source, options.module)?);
    for module in options.modules {
        files.push(parse(module.source, module.path)?);
    }
    let file_names = std::iter::once(options.module.to_owned())
        .chain(options.modules.iter().map(|module| module.path.to_owned()))
        .collect::<Vec<_>>();
    let report = transform_named(&mut files, &file_names)?;
    let entry_source = files[0].to_token_stream().to_string();
    let module_sources = files[1..]
        .iter()
        .map(|file| file.to_token_stream().to_string())
        .collect::<Vec<_>>();
    let modules = options
        .modules
        .iter()
        .zip(module_sources.iter())
        .map(|(module, source)| Module {
            path: module.path,
            source,
        })
        .collect::<Vec<_>>();
    let compiled = crate::compile(
        &entry_source,
        &Options {
            handler: options.handler,
            entry: options.entry,
            state_key: options.state_key,
            kind: options.kind,
            strings: options.strings,
            settings: options.settings,
            module: options.module,
            modules: &modules,
            native: options.native,
            strict: options.strict,
        },
    )?;
    Ok((compiled, report))
}

fn transform_named(
    files: &mut [File],
    file_names: &[String],
) -> Result<FrontendReport, Error> {
    for (file, name) in files.iter_mut().zip(file_names) {
        expand::expand_file(file).map_err(|error| error.with_file(name))?;
    }
    let index = names::ItemIndex::new(files);
    let mut name_report = NameReport::default();
    for (file, name) in files.iter_mut().zip(file_names) {
        names::resolve_file(file, &index, &mut name_report)
            .map_err(|error| error.with_file(name))?;
    }
    let env = consts::ConstEnv::new(files);
    for (file, name) in files.iter().zip(file_names) {
        consts::validate_file(file, &env)
            .map_err(|error| error.with_file(name))?;
    }
    for file in files.iter_mut() {
        consts::fold_file(file, &env)?;
    }
    let traits = types::Traits::new(files);
    let lowering = mono::lower(files, &env, &traits)?;
    Ok(FrontendReport {
        names: name_report,
        lowering,
    })
}

fn parse(source: &str, module: &str) -> Result<File, Error> {
    syn::parse_file(source)
        .map_err(crate::Error::from)
        .map_err(|error| error.with_file(module))
}

#[cfg(test)]
fn test_options<'a>(
    handler: &'a str,
    strict: Option<StrictOptions<'a>>,
    native: NativeMode,
) -> Options<'a> {
    Options {
        handler,
        entry: "widget",
        state_key: "hello.state",
        kind: EntryKind::Widget,
        strings: &["message", "name"],
        settings: &["message", "name"],
        module: "main",
        modules: &[],
        native,
        strict,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(handler: &str) -> Options<'_> {
        test_options(handler, None, NativeMode::Deny)
    }

    fn source(handler: &str) -> String {
        format!(
            "use noctalia_rs::{{Context, Op, Result}};\nuse serde_json::json;\n\n{handler}\n"
        )
    }

    #[test]
    fn source_frontend_stays_available() {
        let text = source(
            "fn handler(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![Op::log(\"hello\")]) }",
        );
        let source_output =
            compile_with_frontend(&text, &options("handler"), Frontend::Source)
                .expect("source compile");
        let typed_output =
            compile_with_frontend(&text, &options("handler"), Frontend::Hir)
                .expect("typed compile");
        assert!(!source_output.source.is_empty());
        assert!(!typed_output.source.is_empty());
    }

    #[test]
    fn typed_frontend_expands_a_macro() {
        let text = source(
            "macro_rules! log_op { ($value:expr) => { Op::log($value) }; }\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![log_op!(\"macro\")]) }",
        );
        let (compiled, report) =
            compile_typed_with_report(&text, &options("handler"))
                .expect("typed compile");
        assert!(report.lowering.specializations.is_empty());
        assert!(
            compiled.source.contains("noctalia.log(\"macro\")"),
            "{compiled:?}"
        );
    }

    #[test]
    fn typed_frontend_monomorphizes_a_generic_helper() {
        let text = source(
            "fn show<T: ToString>(value: T) -> String { value.to_string() }\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![Op::log(show(1i32))]) }",
        );
        let (compiled, report) =
            compile_typed_with_report(&text, &options("handler"))
                .expect("typed compile");
        assert_eq!(report.lowering.specializations.len(), 1, "{report:?}");
        assert!(compiled.source.contains("noctalia.log"), "{compiled:?}");
    }

    #[test]
    fn typed_frontend_resolves_trait_dispatch() {
        let text = source(
            "trait Label { fn label(&self) -> &'static str; }\nstruct A { text: &'static str } struct B { text: &'static str }\nimpl Label for A { fn label(&self) -> &'static str { \"a\" } }\nimpl Label for B { fn label(&self) -> &'static str { \"b\" } }\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { let a = A { text: \"a\" }; let b = B { text: \"b\" }; Ok(vec![Op::log(a.label()), Op::log(b.label())]) }",
        );
        let (compiled, report) =
            compile_typed_with_report(&text, &options("handler"))
                .expect("typed compile");
        assert_eq!(report.lowering.dispatches.len(), 2, "{report:?}");
        assert!(
            compiled.source.contains("noctalia.log(\"a\")"),
            "{compiled:?}"
        );
        assert!(
            compiled.source.contains("noctalia.log(\"b\")"),
            "{compiled:?}"
        );
    }

    #[test]
    fn typed_frontend_folds_consts_with_checked_width() {
        let text = source(
            "const COUNT: u8 = 200 + 55;\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![Op::log(format!(\"{}\", COUNT))]) }",
        );
        let compiled =
            compile_typed(&text, &options("handler")).expect("typed compile");
        assert!(compiled.source.contains("255"), "{compiled:?}");
    }

    #[test]
    fn typed_frontend_rejects_const_overflow_with_a_position() {
        let text = source(
            "const COUNT: u8 = 255;\nconst NEXT: u8 = COUNT + 1;\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { let _ = NEXT; Ok(Vec::new()) }",
        );
        let error =
            compile_typed(&text, &options("handler")).expect_err("overflow");
        let rendered = error.render("main.rs");
        assert!(rendered.contains("main:5:18:"), "{rendered}");
        assert!(rendered.contains("overflows u8"), "{rendered}");
    }

    #[test]
    fn typed_frontend_rejects_unproven_receivers() {
        let text = source(
            "trait Label { fn label(&self) -> &'static str; }\nstruct A;\nfn handler(ctx: &Context) -> Result<Vec<Op>> { let a = ctx.payload_str(\"a\").unwrap_or_default(); Ok(vec![Op::log(a.label())]) }",
        );
        let error = compile_typed(&text, &options("handler"))
            .expect_err("unproven receiver");
        let rendered = error.render("widget.rs");
        assert!(rendered.contains("widget.rs:"), "{rendered}");
        assert!(rendered.contains("cannot prove the receiver"), "{rendered}");
    }
    #[test]
    fn typed_frontend_rejects_null_in_a_json_container() {
        let text = source(
            "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let value = json!({ \"key\": null }); Ok(vec![Op::log(value.to_string())]) }",
        );
        let error = compile_typed(&text, &options("handler"))
            .expect_err("null in an object");
        let rendered = error.render("main.rs");
        assert!(rendered.contains("main.rs:"), "{rendered}");
        assert!(rendered.contains("null"), "{rendered}");
    }

    #[test]
    fn typed_frontend_keeps_argument_evaluation_order() {
        let text = source(
            "macro_rules! logs { ($($value:expr),*) => { vec![$($value),*] }; }\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { Ok(logs![Op::log(\"first\"), Op::log(\"second\")]) }",
        );
        let compiled =
            compile_typed(&text, &options("handler")).expect("typed compile");
        let first = compiled.source.find("noctalia.log(\"first\")");
        let second = compiled.source.find("noctalia.log(\"second\")");
        assert!(first.is_some_and(|at| second.is_some_and(|other| at < other)));
    }

    #[test]
    fn typed_frontend_emits_strict_abi_without_bridge_markers() {
        let text = source(
            "fn handler(_ctx: &Context) -> Result<Vec<Op>> { Ok(vec![Op::log(\"strict\")]) }",
        );
        let strict = StrictOptions {
            config: "local CONFIG = {}\n",
            translations: "local LANGUAGES = {}\nlocal TRANSLATIONS = {}\n",
            assets: &[],
        };
        let mut opts = options("handler");
        opts.strict = Some(strict);
        let compiled = compile_typed(&text, &opts).expect("strict compile");
        assert!(compiled.source.contains("noctalia.log(\"strict\")"));
        assert!(!compiled.source.contains("onHostCallback"));
        assert!(!compiled.source.contains("__host_callback"));
        assert!(!compiled.source.contains("__strict_callback"));
    }

    #[test]
    fn typed_frontend_keeps_closure_and_shadowed_binding_identity() {
        let text = source(
            "fn handler(_ctx: &Context) -> Result<Vec<Op>> { let text = \"outer\"; { let text = \"inner\"; let show = || text; Ok(vec![Op::log(show())]) } }",
        );
        let compiled =
            compile_typed(&text, &options("handler")).expect("typed compile");
        assert!(compiled.source.contains("\"inner\""), "{compiled:?}");
        assert!(!compiled.source.contains("unresolved"), "{compiled:?}");
    }

    #[test]
    fn typed_frontend_binds_trait_method_arguments() {
        let text = source(
            "trait Add { fn add(&self, b: i32) -> i32; }\nstruct A { value: i32 }\nimpl Add for A { fn add(&self, b: i32) -> i32 { b + self.value } }\nfn handler(_ctx: &Context) -> Result<Vec<Op>> { let a = A { value: 1 }; Ok(vec![Op::log(a.add(2).to_string())]) }",
        );
        let (compiled, report) =
            compile_typed_with_report(&text, &options("handler"))
                .expect("typed compile");
        assert_eq!(report.lowering.dispatches.len(), 1, "{report:?}");
        assert!(compiled.source.contains("= 2"), "{compiled:?}");
        assert!(compiled.source.contains("__self_0.value"), "{compiled:?}");
    }
}
