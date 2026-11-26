use super::*;

impl<'a> Emitter<'a> {
    pub(crate) fn constants(&mut self) {
        for (name, value) in self.program.constants(&self.module) {
            if let Some(entry) = constant(value) {
                self.constants.insert(name.to_owned(), entry);
            }
        }
    }

    pub(super) fn register_static(
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
        if let Expr::Tuple(items) = item.expr.as_ref() {
            let shapes = self.tuple_shapes(
                &items.elems.iter().cloned().collect::<Vec<_>>(),
            )?;
            self.tuple_expr_shapes.insert(lua_name.clone(), shapes);
        }
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
        if let Expr::Tuple(items) = expr {
            let saved_out = std::mem::take(&mut self.out);
            let saved_indent = self.indent;
            self.indent = 0;
            let result = items
                .elems
                .iter()
                .map(|item| self.expr(item))
                .collect::<Result<Vec<_>, Error>>();
            let emitted = std::mem::replace(&mut self.out, saved_out);
            self.indent = saved_indent;
            let values = result?;
            let shapes = values
                .iter()
                .zip(items.elems.iter())
                .map(|((text, ty), _)| {
                    let mut ty = *ty;
                    if self.null_texts.contains(text) {
                        ty.null = true;
                    }
                    TupleShape::Leaf(ty)
                })
                .collect::<Vec<_>>();
            let text = format!(
                "{{ {} }}",
                values
                    .iter()
                    .map(|(text, _)| text.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            self.tuple_expr_shapes.insert(text.clone(), shapes);
            self.out.push_str(&emitted);
            return Ok(text);
        }
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

    pub(super) fn static_value(
        &mut self,
        expr: &Expr,
    ) -> Result<String, Error> {
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

    pub(super) fn constant(&self, name: &str) -> Option<Constant> {
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

    pub(super) fn module_constants(&self, module: &str) -> Constants {
        let mut scope = HashMap::new();
        for (name, value) in self.program.constants(module) {
            if let Some(entry) = constant(value) {
                scope.insert(name.to_owned(), entry);
            }
        }
        scope
    }

    pub(super) fn local_static(
        &mut self,
        item: &syn::ItemStatic,
    ) -> Result<(), Error> {
        let kind = static_kind(&item.ty);
        let init = self.static_init(&item.expr, kind)?;
        let name = format!("__local_{}", self.temp().trim_start_matches("__t"));
        self.line(&format!("local {name} = {init}"));
        if let Some(shapes) = self.tuple_expr_shapes.get(&init).cloned() {
            self.tuple_expr_shapes.insert(name.clone(), shapes);
        }
        if let Some(scope) = self.local_statics.last_mut() {
            scope.insert(item.ident.to_string(), (name, kind));
        }
        Ok(())
    }
}
