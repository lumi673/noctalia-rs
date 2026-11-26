use super::*;

impl<'a> Emitter<'a> {
    /// `std::path` receivers. A `Path` or `PathBuf` is the Luau string the host
    /// returns, so every component operation is string surgery on that value.
    pub(super) fn path_method(
        &mut self,
        call: &ExprMethodCall,
        method: &str,
        args: &[&Expr],
        receiver: &str,
        ty: Ty,
    ) -> Result<OptionalValue, Error> {
        let typed = ty.base == Base::Path;
        if !typed && !self.claims_path_method(method) {
            return Ok(None);
        }
        let value: Result<(String, Ty), Error> = match method {
            "join" if typed => {
                let part = self.expr(arg(args, 0, call)?)?.0;
                Ok((path_join(receiver, &part), Ty::PATH))
            }
            "push" if typed && args.len() == 1 => {
                let part = self.expr(arg(args, 0, call)?)?.0;
                self.line(&format!(
                    "{receiver} = {}",
                    path_join(receiver, &part)
                ));
                Ok((String::new(), Ty::VOID))
            }
            "parent" if args.is_empty() => {
                Ok((path_parent(receiver), Ty::OPT_PATH))
            }
            "file_name" if args.is_empty() => {
                Ok((path_file_name(receiver), Ty::OPT_PATH))
            }
            "file_stem" if args.is_empty() => {
                Ok((path_stem(receiver, false), Ty::OPT_PATH))
            }
            "extension" if args.is_empty() => {
                Ok((path_stem(receiver, true), Ty::OPT_PATH))
            }
            "with_extension" => {
                let extension = self.expr(arg(args, 0, call)?)?.0;
                Ok((path_with_extension(receiver, &extension), Ty::PATH))
            }
            "set_extension" => {
                let extension = self.expr(arg(args, 0, call)?)?.0;
                self.line(&format!(
                    "{receiver} = {}",
                    path_with_extension(receiver, &extension)
                ));
                Ok((String::new(), Ty::VOID))
            }
            "exists" if args.is_empty() => {
                Ok((format!("noctalia.fileExists({receiver})"), Ty::BOOL))
            }
            "is_dir" if args.is_empty() => {
                Ok((path_is_dir(receiver), Ty::BOOL))
            }
            "is_file" if args.is_empty() => {
                Ok((path_is_file(receiver), Ty::BOOL))
            }
            "is_absolute" if args.is_empty() => Ok((
                format!("(string.sub({receiver}, 1, 1) == \"/\")"),
                Ty::BOOL,
            )),
            "starts_with" if typed => {
                let prefix = self.expr(arg(args, 0, call)?)?.0;
                Ok((path_starts_with(receiver, &prefix), Ty::BOOL))
            }
            "ends_with" if typed => {
                let suffix = self.expr(arg(args, 0, call)?)?.0;
                Ok((path_ends_with(receiver, &suffix), Ty::BOOL))
            }
            "metadata" if args.is_empty() => Ok((
                format!("noctalia.fileInfo({receiver})"),
                Ty::result(Base::Info),
            )),
            "read_dir" if args.is_empty() => Ok((
                format!("noctalia.listDir({receiver})"),
                Ty::result(Base::List),
            )),
            "path" if args.is_empty() => Ok((receiver.to_owned(), Ty::PATH)),
            "display" | "to_string_lossy" => Ok((receiver.to_owned(), Ty::STR)),
            "to_str" if args.is_empty() => {
                Ok((receiver.to_owned(), Ty::OPT_STR))
            }
            "as_os_str" | "as_path" | "to_path_buf" if args.is_empty() => {
                Ok((receiver.to_owned(), Ty::PATH))
            }
            "canonicalize" | "symlink_metadata" if args.is_empty() => {
                Err(Error::at(
                    call.method.span(),
                    format!(
                        "`{method}` has no API 57 member: the host resolves paths with expandPath, fileInfo and fileExists only"
                    ),
                ))
            }
            _ => return Ok(None),
        };
        Ok(Some(value?))
    }

    /// Whether a receiver whose type the compiler lost can still be a path.
    /// Rust only has these methods on `Path`, `PathBuf` and `OsStr`, so they
    /// stay path operations unless the plugin defines a method of that name.
    fn claims_path_method(&self, method: &str) -> bool {
        if self.program.defines_method(method) {
            return false;
        }
        matches!(
            method,
            "parent"
                | "file_name"
                | "file_stem"
                | "extension"
                | "exists"
                | "is_dir"
                | "is_file"
                | "is_absolute"
                | "path"
                | "metadata"
                | "read_dir"
                | "as_os_str"
                | "as_path"
                | "to_path_buf"
                | "to_str"
                | "display"
                | "to_string_lossy"
        )
    }
}

/// `Path::join` and `PathBuf::push`: an absolute part replaces the path, and an
/// empty part adds one separator.
pub(super) fn path_join(receiver: &str, part: &str) -> String {
    format!(
        "(function() local __base, __part = {receiver}, {part}; if __base == \"\" then return __part end; if string.sub(__part, 1, 1) == \"/\" then return __part end; if string.sub(__base, -1) == \"/\" then return __base .. __part end; return __base .. \"/\" .. __part end)()"
    )
}

/// The components of a path, so root, `.`, `..` and repeated separators keep
/// Rust's meaning instead of a byte prefix's.
const COMPONENTS: &str = "local __parts = {}; for __part in string.gmatch(__trim, \"[^/]+\") do __parts[#__parts + 1] = __part end";

/// `Path::parent`: the path without its last component, `\"\"` for a single
/// component, `/` for a depth-one absolute path, and `nil` for the root.
fn path_parent(receiver: &str) -> String {
    format!(
        "(function() local __path = {receiver}; local __trim = __path; while #__trim > 1 and string.sub(__trim, -1) == \"/\" do __trim = string.sub(__trim, 1, -2) end; if __trim == \"\" or __trim == \"/\" then return nil end; {COMPONENTS}; if #__parts == 0 then return nil end; table.remove(__parts); if #__parts == 0 then if string.sub(__trim, 1, 1) == \"/\" then return \"/\" end; return \"\" end; if string.sub(__trim, 1, 1) == \"/\" then return \"/\" .. table.concat(__parts, \"/\") end; return table.concat(__parts, \"/\") end)()"
    )
}

/// The shared prologue of the name-shaped readers: trailing separators are
/// insignificant, and the last component must not be `.` or `..`.
fn name_prelude(receiver: &str) -> String {
    format!(
        "local __path = {receiver}; local __trim = __path; while #__trim > 1 and string.sub(__trim, -1) == \"/\" do __trim = string.sub(__trim, 1, -2) end; {COMPONENTS}; local __name = __parts[#__parts]; if __name == nil or __name == \".\" or __name == \"..\" then return nil end"
    )
}

/// The last dot of a name, ignoring a leading one so `.bashrc` has no
/// extension.
const LAST_DOT: &str = "local __dot = nil; for __i = #__name, 2, -1 do if string.sub(__name, __i, __i) == \".\" then __dot = __i break end end";

/// `Path::file_name`: the last component, `nil` for the root, for `.` and for a
/// path that ends in `..`.
fn path_file_name(receiver: &str) -> String {
    format!(
        "(function() {}; return __name end)()",
        name_prelude(receiver)
    )
}

/// `Path::file_stem` and `Path::extension` of the file name.
fn path_stem(receiver: &str, extension: bool) -> String {
    let tail = if extension {
        "if __dot == nil then return nil end; return string.sub(__name, __dot + 1)"
    } else {
        "if __dot == nil then return __name end; return string.sub(__name, 1, __dot - 1)"
    };
    format!(
        "(function() {}; {LAST_DOT}; {tail} end)()",
        name_prelude(receiver)
    )
}

/// `Path::with_extension`, which leaves a path with no file name alone.
fn path_with_extension(receiver: &str, extension: &str) -> String {
    format!(
        "(function() local __extension = {extension}; {}; {LAST_DOT}; local __stem = __name; if __dot ~= nil then __stem = string.sub(__name, 1, __dot - 1) end; if __extension ~= \"\" then __stem = __stem .. \".\" .. __extension end; return string.sub(__path, 1, #__path - #__name) .. __stem end)()",
        name_prelude(receiver)
    )
}

/// `Path::is_dir` and `Path::is_file` through `fileInfo`, `false` when the host
/// reports an error for the path.
fn path_is_dir(receiver: &str) -> String {
    format!(
        "(function() local __info = noctalia.fileInfo({receiver}); return (__info ~= nil and __info.isDir == true) end)()"
    )
}

fn path_is_file(receiver: &str) -> String {
    format!(
        "(function() local __info = noctalia.fileInfo({receiver}); return (__info ~= nil and __info.isDir ~= true) end)()"
    )
}

/// `Path::starts_with` compares components, not bytes: `/a/bc` does not start
/// with `/a/b`.
fn path_starts_with(receiver: &str, prefix: &str) -> String {
    format!(
        "(function() local __path, __prefix = {receiver}, {prefix}; if __prefix == \"\" then return true end; __path = string.gsub(__path, \"//+\", \"/\"); __prefix = string.gsub(__prefix, \"//+\", \"/\"); while #__path > 1 and string.sub(__path, -1) == \"/\" do __path = string.sub(__path, 1, -2) end; while #__prefix > 1 and string.sub(__prefix, -1) == \"/\" do __prefix = string.sub(__prefix, 1, -2) end; if __prefix == \"/\" then return string.sub(__path, 1, 1) == \"/\" end; if #__path < #__prefix or string.sub(__path, 1, #__prefix) ~= __prefix then return false end; return #__path == #__prefix or string.sub(__path, #__prefix + 1, #__prefix + 1) == \"/\" end)()"
    )
}

/// `Path::ends_with`, by components from the end.
fn path_ends_with(receiver: &str, suffix: &str) -> String {
    format!(
        "(function() local __path, __suffix = {receiver}, {suffix}; if __suffix == \"\" then return true end; __path = string.gsub(__path, \"//+\", \"/\"); __suffix = string.gsub(__suffix, \"//+\", \"/\"); while #__path > 1 and string.sub(__path, -1) == \"/\" do __path = string.sub(__path, 1, -2) end; while #__suffix > 1 and string.sub(__suffix, -1) == \"/\" do __suffix = string.sub(__suffix, 1, -2) end; if __suffix == \"/\" then return __path == \"/\" end; if #__path < #__suffix then return false end; local __at = #__path - #__suffix + 1; if string.sub(__path, __at) ~= __suffix then return false end; return __at == 1 or string.sub(__path, __at - 1, __at - 1) == \"/\" end)()"
    )
}
