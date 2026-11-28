//! Lowering for `#[derive(Config)]` reads.
//!
//! The derive macro generates a native `from_ctx` for `off` builds. The
//! compiler sees the raw struct instead, so it lowers the same method here and
//! reads the build's `CONFIG` table directly in strict mode.

use super::*;
use crate::config::{ConfigField, ConfigKind, ConfigLiteral};

const UNSUPPORTED_ARG: &str =
    "Config::from_ctx takes the handler's &Context as its only argument";

/// A strict config table has only manifest-declared keys, so a dynamic key
/// cannot be proved equivalent to the host `getConfig` call.
pub(super) fn strict_config_key(expr: &Expr) -> Result<(), Error> {
    if matches!(
        strip_reference(expr),
        Expr::Lit(ExprLit {
            lit: Lit::Str(_),
            ..
        })
    ) {
        return Ok(());
    }
    Err(Error::at(
        expr.span(),
        "strict config keys must be string literals declared in noctalia.toml",
    ))
}

impl<'a> Emitter<'a> {
    pub(super) fn config_from_ctx(
        &mut self,
        call: &ExprCall,
        args: &[&Expr],
    ) -> Result<OptionalValue, Error> {
        let Expr::Path(path) = strip_reference(&call.func) else {
            return Ok(None);
        };
        let Some(segments) = path_segments(&path.path) else {
            return Ok(None);
        };
        if segments.last().is_none_or(|name| name != "from_ctx")
            || segments.len() < 2
        {
            return Ok(None);
        }
        let class = segments[segments.len() - 2].clone();
        let Some(fields) = self
            .program
            .config_fields(&self.module, &class)
            .map(<[ConfigField]>::to_vec)
        else {
            return Ok(None);
        };
        if args.len() != 1 || !self.is_ctx_expr(args[0]) {
            return Err(Error::at(call.span(), UNSUPPORTED_ARG));
        }

        let result = self.temp();
        self.line(&format!("local {result} = {{}}"));
        self.classes.insert(result.clone(), class);
        for field in fields {
            self.config_field(&result, &field);
        }
        Ok(Some((result, Ty::VALUE)))
    }

    fn config_field(&mut self, result: &str, field: &ConfigField) {
        let raw = if self.strict() {
            self.uses_config_table = true;
            format!("CONFIG[{}]", quote(&field.key))
        } else {
            format!("noctalia.getConfig({})", quote(&field.key))
        };
        let target = format!("{result}.{}", lua_key(&field.name));
        let value = self.temp();
        self.line("do");
        self.indent = self.indent.saturating_add(1);
        self.line(&format!("local {value} = {raw}"));
        match field.kind {
            ConfigKind::Str => {
                let default = field.default.lua();
                self.line(&format!(
                    "if {value} == nil then {target} = {default} else {target} = tostring({value}) end"
                ));
            }
            ConfigKind::Bool => {
                let default = field.default.lua();
                self.line(&format!(
                    "if type({value}) == \"boolean\" then {target} = {value} else {target} = {default} end"
                ));
            }
            ConfigKind::F64 => {
                let number = self.temp();
                let default = field.default.lua();
                self.line(&format!("local {number} = tonumber({value})"));
                self.line(&format!(
                    "if {number} == nil then {target} = {default} else {target} = {number} end"
                ));
            }
            ConfigKind::I32 | ConfigKind::U32 => {
                let number = self.temp();
                let default = field.default.lua();
                let (minimum, maximum) =
                    ConfigLiteral::integer_range(field.kind)
                        .expect("integer config fields have a range");
                self.line(&format!("local {number} = tonumber({value})"));
                self.line(&format!(
                    "if {number} == nil or {number} % 1 ~= 0 or {number} < {minimum} or {number} > {maximum} then {target} = {default} else {target} = {number} end"
                ));
            }
        }
        self.indent = self.indent.saturating_sub(1);
        self.line("end");
    }
}
