use super::*;

impl<'a> Emitter<'a> {
    /// `std::fs` receivers: an open file handle and the `fileInfo` table that
    /// `metadata` and `fileInfo` return.
    pub(super) fn fs_method(
        &mut self,
        call: &ExprMethodCall,
        method: &str,
        args: &[&Expr],
        receiver: &str,
        ty: Ty,
    ) -> Result<OptionalValue, Error> {
        if ty.base != Base::Info {
            return Ok(None);
        }
        let value = match method {
            "len" if args.is_empty() => {
                Ok((format!("{receiver}.size"), Ty::INT))
            }
            "is_dir" if args.is_empty() => {
                Ok((format!("({receiver}.isDir == true)"), Ty::BOOL))
            }
            "is_file" if args.is_empty() => {
                Ok((format!("({receiver}.isDir ~= true)"), Ty::BOOL))
            }
            "modified" | "accessed" | "created" => Err(Error::at(
                call.method.span(),
                format!(
                    "`Metadata::{method}` has no Luau equivalent: fileInfo reports mtime as whole epoch seconds, so a SystemTime cannot be translated"
                ),
            )),
            "permissions" | "file_type" | "dev" | "ino" | "mode" => {
                Err(Error::at(
                    call.method.span(),
                    format!(
                        "`Metadata::{method}` has no API 57 member: fileInfo reports size, mtime and isDir only"
                    ),
                ))
            }
            _ => return Ok(None),
        };
        Ok(Some(value?))
    }
}

/// Class marker for a file opened through `OpenOptions::new().append(true)`.
/// The value is the path itself, because every append reads and rewrites it.
pub(super) const APPEND_HANDLE: &str = "__append_file";

impl<'a> Emitter<'a> {
    /// `OpenOptions::new().create(true).append(true).open(path)`. The host has
    /// no append open, so the handle is the path and each write appends by
    /// reading the file and writing it back.
    pub(super) fn open_append(
        &mut self,
        call: &ExprMethodCall,
        args: &[&Expr],
    ) -> Result<OptionalValue, Error> {
        let mut append = false;
        let mut truncate = false;
        let mut node = strip_reference(&call.receiver);
        loop {
            match node {
                Expr::MethodCall(chain) => {
                    let name = chain.method.to_string();
                    let enabled = chain.args.first().is_some_and(|value| {
                        self.const_bool(value) == Some(true)
                    });
                    match name.as_str() {
                        "append" => append = enabled,
                        "truncate" => truncate = enabled,
                        "create" | "create_new" | "write" | "read" => {}
                        _ => return Ok(None),
                    }
                    node = strip_reference(&chain.receiver);
                }
                Expr::Call(new) => {
                    let Some(name) = path_last_name(&new.func) else {
                        return Ok(None);
                    };
                    if name != "OpenOptions::new" && name != "File::options" {
                        return Ok(None);
                    }
                    break;
                }
                _ => return Ok(None),
            }
        }
        if !append || truncate {
            return Err(Error::at(
                call.method.span(),
                "`OpenOptions` has no API 57 mapping: only `append(true)` without `truncate` maps to a read plus write",
            ));
        }
        let path = self.expr(arg(args, 0, call)?)?.0;
        self.pending_class = Some(APPEND_HANDLE.to_owned());
        Ok(Some((path, Ty::result(Base::Path))))
    }

    /// Writes on an append handle. The read and the write stay in one place so
    /// the appended text lands in a single `writeFile` call.
    pub(super) fn append_method(
        &mut self,
        call: &ExprMethodCall,
        method: &str,
        args: &[&Expr],
        receiver: &str,
    ) -> Result<OptionalValue, Error> {
        let value: Result<(String, Ty), Error> = match method {
            "write_all" | "write" if args.len() == 1 => {
                let text = self.expr(arg(args, 0, call)?)?.0;
                Ok((append_write(receiver, &text), Ty::result(Base::Void)))
            }
            "flush" if args.is_empty() => Ok((
                "(function() return true end)()".to_owned(),
                Ty::result(Base::Void),
            )),
            _ => return Ok(None),
        };
        Ok(Some(value?))
    }
}

/// `write!(handle, ...)` and `writeln!(handle, ...)`, which only make sense on
/// an append handle.
pub(super) struct WriteArgs {
    pub(super) writer: Expr,
    pub(super) format: FormatArgs,
}

impl Parse for WriteArgs {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let writer: Expr = input.parse()?;
        if input.is_empty() {
            return Ok(Self {
                writer,
                format: FormatArgs {
                    literal: String::new(),
                    arguments: Vec::new(),
                },
            });
        }
        let _: Token![,] = input.parse()?;
        Ok(Self {
            writer,
            format: input.parse()?,
        })
    }
}

impl<'a> Emitter<'a> {
    pub(crate) fn write_macro(
        &mut self,
        mac: &ExprMacro,
        newline: bool,
    ) -> Result<(String, Ty), Error> {
        let parsed: WriteArgs = syn::parse2(mac.mac.tokens.clone())?;
        if self.class_of_receiver(&parsed.writer).as_deref()
            != Some(APPEND_HANDLE)
        {
            return Err(Error::at(
                mac.span(),
                "`write!` needs a file opened with `OpenOptions::new().append(true)`",
            ));
        }
        let (path, _) = self.expr(&parsed.writer)?;
        let text = self.format_text(mac, &parsed.format)?;
        let text = if newline {
            format!("({text} .. \"\\n\")")
        } else {
            text
        };
        Ok((append_write(&path, &text), Ty::result(Base::Void)))
    }
}

/// Reads the handle, appends, and writes it back: one `writeFile` call.
pub(super) fn append_write(handle: &str, text: &str) -> String {
    format!(
        "(function() local __path, __text = {handle}, {text}; local __previous = noctalia.readFile(__path); local __ok, __error = noctalia.writeFile(__path, (__previous or \"\") .. __text); if not __ok then return nil, __error end; return true end)()"
    )
}
