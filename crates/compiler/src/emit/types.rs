use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Base {
    Str,
    /// A filesystem path. The value is a Luau string, and the base only selects
    /// component-wise path semantics over string ones.
    Path,
    /// A `{ size, mtime, isDir }` table from `fileInfo`.
    Info,
    Int,
    Float,
    Bool,
    Value,
    List,
    Node,
    Table,
    Ops,
    Context,
    Void,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Ty {
    pub(crate) base: Base,
    pub(crate) optional: bool,
    pub(crate) result: bool,
    /// The expression is a JSON null, which a Luau container cannot hold.
    pub(crate) null: bool,
    /// Element type of a list that is known, used by `next()`.
    pub(crate) element: Option<Base>,
}

impl Ty {
    pub(crate) const fn new(base: Base) -> Self {
        Self {
            base,
            optional: false,
            result: false,
            null: false,
            element: None,
        }
    }

    pub(crate) const fn opt(base: Base) -> Self {
        Self {
            base,
            optional: true,
            result: false,
            null: false,
            element: None,
        }
    }

    pub(crate) const fn result(base: Base) -> Self {
        Self {
            base,
            optional: false,
            result: true,
            null: false,
            element: None,
        }
    }

    pub(crate) const STR: Self = Self::new(Base::Str);
    pub(crate) const PATH: Self = Self::new(Base::Path);
    pub(crate) const OPT_PATH: Self = Self::opt(Base::Path);
    pub(crate) const INFO: Self = Self::new(Base::Info);
    pub(crate) const OPT_STR: Self = Self::opt(Base::Str);
    pub(crate) const INT: Self = Self::new(Base::Int);
    pub(crate) const OPT_INT: Self = Self::opt(Base::Int);
    pub(crate) const FLOAT: Self = Self::new(Base::Float);
    pub(crate) const OPT_FLOAT: Self = Self::opt(Base::Float);
    pub(crate) const BOOL: Self = Self::new(Base::Bool);
    pub(crate) const OPT_BOOL: Self = Self::opt(Base::Bool);
    pub(crate) const VALUE: Self = Self::new(Base::Value);
    pub(crate) const OPT_VALUE: Self = Self::opt(Base::Value);
    pub(crate) const LIST: Self = Self::new(Base::List);
    pub(crate) const OPT_LIST: Self = Self::opt(Base::List);
    pub(crate) const NODE: Self = Self::new(Base::Node);
    pub(crate) const TABLE: Self = Self::new(Base::Table);
    pub(crate) const OPS: Self = Self::new(Base::Ops);
    pub(crate) const CONTEXT: Self = Self::new(Base::Context);
    pub(crate) const VOID: Self = Self::new(Base::Void);
    pub(crate) const NULL: Self = Self {
        base: Base::Value,
        optional: false,
        result: false,
        null: true,
        element: None,
    };
    pub(crate) const LIST_OF_KEYS: Self = Self {
        base: Base::List,
        optional: false,
        result: false,
        null: false,
        element: Some(Base::Str),
    };
    pub(crate) const STRING_LIST: Self = Self {
        base: Base::List,
        optional: false,
        result: false,
        null: false,
        element: Some(Base::Str),
    };

    pub(crate) const fn is_numeric(self) -> bool {
        matches!(self.base, Base::Int | Base::Float)
    }

    /// Whether the value prints and behaves as text, which includes a path.
    pub(super) fn is_string(self) -> bool {
        matches!(self.base, Base::Str | Base::Path)
    }

    pub(crate) const fn default_literal(self) -> Option<&'static str> {
        match self.base {
            Base::Str | Base::Path => Some("\"\""),
            Base::Int | Base::Float => Some("0"),
            Base::Bool => Some("false"),
            Base::List | Base::Table => Some("{}"),
            Base::Info
            | Base::Value
            | Base::Node
            | Base::Ops
            | Base::Context
            | Base::Void => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StaticKind {
    Once,
    OnceLock,
    Lazy,
    Mutex,
    RwLock,
    Atomic,
    Value,
}

#[derive(Clone, Debug)]
pub(crate) struct StaticDef {
    pub(crate) name: String,
    pub(crate) kind: StaticKind,
    pub(crate) init: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Returns {
    Ops,
    VecOps,
    Unit,
    Value(Ty),
}

pub(super) fn json_base_for_type(ty: &Type) -> Result<Base, Error> {
    let name = type_text(ty);
    let base = match name.as_str() {
        "Value" => Base::Value,
        "String" | "str" => Base::Str,
        "bool" => Base::Bool,
        "i64" | "u64" | "usize" | "u32" | "i32" => Base::Int,
        "f64" | "f32" => Base::Float,
        "Vec" => Base::List,
        "HashMap" | "BTreeMap" | "Map" => Base::Table,
        _ => {
            return Err(Error::at(
                ty.span(),
                format!(
                    "JSON parsing into `{name}` is not supported by the Luau compiler"
                ),
            ));
        }
    };
    Ok(base)
}

pub(super) fn json_parse_base(call: &ExprCall) -> Result<Base, Error> {
    let Expr::Path(path) = &*call.func else {
        return Ok(Base::Value);
    };
    let Some(segment) = path.path.segments.last() else {
        return Ok(Base::Value);
    };
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return Ok(Base::Value);
    };
    let Some(syn::GenericArgument::Type(ty)) = arguments.args.first() else {
        return Ok(Base::Value);
    };
    json_base_for_type(ty)
}

pub(super) fn constant(expr: &Expr) -> Option<(String, Ty)> {
    match expr {
        Expr::Lit(literal) => Some(match &literal.lit {
            Lit::Str(value) => (quote(&value.value()), Ty::STR),
            Lit::Char(value) => (quote(&value.value().to_string()), Ty::STR),
            Lit::Byte(value) => (value.value().to_string(), Ty::INT),
            Lit::ByteStr(value) => {
                (quote(&String::from_utf8_lossy(&value.value())), Ty::STR)
            }
            Lit::Int(value) => (value.base10_digits().to_owned(), Ty::INT),
            Lit::Float(value) => (value.base10_digits().to_owned(), Ty::FLOAT),
            Lit::Bool(value) => (
                if value.value { "true" } else { "false" }.to_owned(),
                Ty::BOOL,
            ),
            _ => return None,
        }),
        Expr::Reference(reference) => constant(&reference.expr),
        Expr::Paren(paren) => constant(&paren.expr),
        Expr::Group(group) => constant(&group.expr),
        Expr::Unary(unary) => {
            let (text, ty) = constant(&unary.expr)?;
            match unary.op {
                UnOp::Neg(_) => Some((format!("(-{text})"), ty)),
                UnOp::Deref(_) => Some((text, ty)),
                UnOp::Not(_) => Some((format!("(not {text})"), Ty::BOOL)),
                _ => None,
            }
        }
        Expr::Array(array) => {
            let mut values = Vec::new();
            for item in &array.elems {
                values.push(constant(item)?.0);
            }
            Some((format!("{{ {} }}", values.join(", ")), Ty::LIST))
        }
        Expr::Tuple(tuple) => {
            let mut values = Vec::new();
            for item in &tuple.elems {
                values.push(constant(item)?.0);
            }
            Some((format!("{{ {} }}", values.join(", ")), Ty::LIST))
        }
        Expr::Path(path) => {
            let segments = path_segments(&path.path)?;
            let last = segments.last()?;
            match last.as_str() {
                "MAX" | "MIN" => {
                    let value = segments.iter().find_map(|name| {
                        integer_bound(name.as_str(), last == "MAX")
                    })?;
                    Some((value.to_owned(), Ty::INT))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// The exact `MAX`/`MIN` bound of an integer type.
fn integer_bound(kind: &str, max: bool) -> Option<&'static str> {
    Some(match (kind, max) {
        ("i8", true) => "127",
        ("i8", false) => "(-128)",
        ("i16", true) => "32767",
        ("i16", false) => "(-32768)",
        ("i32", true) => "2147483647",
        ("i32", false) => "(-2147483648)",
        ("i64" | "isize", true) => "9223372036854775807",
        ("i64" | "isize", false) => "(-9223372036854775808)",
        ("i128", true) => "170141183460469231731687303715884105727",
        ("i128", false) => "(-170141183460469231731687303715884105728)",
        ("u8", true) => "255",
        ("u16", true) => "65535",
        ("u32", true) => "4294967295",
        ("u64" | "usize", true) => "18446744073709551615",
        ("u128", true) => "340282366920938463463374607431768211455",
        ("u8" | "u16" | "u32" | "u64" | "u128" | "usize", false) => "0",
        _ => return None,
    })
}

/// The finite `MAX`/`MIN` bound of a float type.
pub(super) fn float_bound(kind: &str, bound: &str) -> Option<&'static str> {
    Some(match (kind, bound) {
        ("f64", "MAX") => "1.7976931348623157e308",
        ("f64", "MIN") => "(-1.7976931348623157e308)",
        ("f32", "MAX") => "3.4028234663852886e38",
        ("f32", "MIN") => "(-3.4028234663852886e38)",
        _ => return None,
    })
}

/// JSON predicates used as callback values, such as `Value::is_object`.
pub(super) fn value_predicate(name: &str) -> Option<&'static str> {
    match name {
        "is_string" => Some("string"),
        "is_number" | "is_i64" | "is_u64" | "is_f64" => Some("number"),
        "is_boolean" => Some("boolean"),
        "is_array" | "is_object" => Some("table"),
        _ => None,
    }
}

/// The empty value a container constructor returns when used as a callback.
pub(super) fn container_default(name: &str) -> Option<&'static str> {
    match name {
        "String" => Some("\"\""),
        "Vec" | "VecDeque" | "HashMap" | "BTreeMap" | "HashSet"
        | "BTreeSet" | "Map" | "IndexMap" | "IndexSet" => Some("{}"),
        _ => None,
    }
}

pub(super) fn value_accessor(name: &str) -> Option<(&'static str, Ty)> {
    match name {
        "as_str" => Some(("string", Ty::OPT_STR)),
        "as_bool" => Some(("boolean", Ty::OPT_BOOL)),
        "as_i64" | "as_u64" => Some(("number", Ty::OPT_INT)),
        "as_f64" => Some(("number", Ty::OPT_FLOAT)),
        "as_array" => Some(("table", Ty::OPT_LIST)),
        "as_object" => Some(("table", Ty::OPT_VALUE)),
        _ => None,
    }
}

pub(super) fn path_segments(path: &syn::Path) -> Option<Vec<String>> {
    if path.leading_colon.is_some()
        || path
            .segments
            .iter()
            .any(|segment| !matches!(segment.arguments, PathArguments::None))
    {
        return None;
    }
    Some(
        path.segments
            .iter()
            .map(|segment| segment.ident.to_string())
            .collect(),
    )
}

pub(super) fn generic_arg(ty: &Type) -> Option<&Type> {
    let Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    let PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    let syn::GenericArgument::Type(inner) = arguments.args.first()? else {
        return None;
    };
    Some(inner)
}

fn is_generic(ty: &Type, name: &str) -> bool {
    matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == name))
}

pub(super) fn is_result(ty: &Type) -> bool {
    matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Result"))
}

pub(super) fn is_ops_vec(ty: &Type) -> bool {
    let ty = match ty {
        Type::Reference(reference) => &*reference.elem,
        other => other,
    };
    let Type::Path(path) = ty else {
        return false;
    };
    let Some(segment) = path.path.segments.last() else {
        return false;
    };
    if segment.ident != "Vec" {
        return false;
    }
    generic_arg(ty).is_some_and(|inner| {
        matches!(inner, Type::Path(path) if path.path.segments.last().is_some_and(|segment| segment.ident == "Op"))
    })
}

pub(super) fn return_kind(function: &ItemFn) -> Result<Returns, Error> {
    let ty = match &function.sig.output {
        syn::ReturnType::Default => return Ok(Returns::Unit),
        syn::ReturnType::Type(_, ty) => &**ty,
    };
    if is_result(ty) {
        return match generic_arg(ty) {
            Some(inner) if is_ops_vec(inner) => Ok(Returns::Ops),
            Some(inner) => Ok(Returns::Value(type_hint(inner))),
            None => Ok(Returns::Value(Ty::VALUE)),
        };
    }
    if is_ops_vec(ty) {
        return Ok(Returns::VecOps);
    }
    if matches!(ty, Type::Tuple(tuple) if tuple.elems.is_empty()) {
        return Ok(Returns::Unit);
    }
    Ok(Returns::Value(type_hint(ty)))
}

pub(super) fn value_type(returns: Returns) -> Ty {
    match returns {
        Returns::Value(ty) => ty,
        Returns::Ops | Returns::VecOps | Returns::Unit => Ty::VALUE,
    }
}

pub(super) fn type_hint(ty: &Type) -> Ty {
    let text = type_text(ty);
    if is_generic(ty, "Option") {
        let Some(inner) = generic_arg(ty) else {
            return Ty::OPT_VALUE;
        };
        let mut value = type_hint(inner);
        if value.base == Base::Ops {
            return Ty::LIST;
        }
        value.optional = true;
        return value;
    }
    if is_generic(ty, "Result") {
        return generic_arg(ty).map_or(Ty::VALUE, type_hint);
    }
    if text.contains("String") || text == "str" {
        Ty::STR
    } else if matches!(text.as_str(), "Path" | "PathBuf" | "OsStr" | "OsString")
    {
        Ty::PATH
    } else if text == "bool" {
        Ty::BOOL
    } else if text.starts_with("Vec") || text.starts_with("Option<Vec") {
        Ty::LIST
    } else if text.starts_with("HashMap")
        || text.starts_with("BTreeMap")
        || text.starts_with("Map")
    {
        Ty::TABLE
    } else if matches!(text.as_str(), "i64" | "u64" | "usize" | "u32" | "i32") {
        Ty::INT
    } else if matches!(text.as_str(), "f64" | "f32") {
        Ty::FLOAT
    } else if text == "Op" {
        Ty::OPS
    } else {
        Ty::VALUE
    }
}

pub(super) fn type_text(ty: &Type) -> String {
    match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map_or_else(String::new, |segment| segment.ident.to_string()),
        Type::Reference(reference) => type_text(&reference.elem),
        _ => String::new(),
    }
}

pub(super) fn turbofish_type(
    arguments: Option<&syn::AngleBracketedGenericArguments>,
) -> Ty {
    let Some(arguments) = arguments else {
        return Ty::OPT_VALUE;
    };
    let Some(syn::GenericArgument::Type(ty)) = arguments.args.first() else {
        return Ty::OPT_VALUE;
    };
    let text = type_text(ty);
    let base = if text == "String" || text == "str" {
        Base::Str
    } else if text == "bool" {
        Base::Bool
    } else if matches!(text.as_str(), "i64" | "u64" | "usize") {
        Base::Int
    } else if matches!(text.as_str(), "f64" | "f32") {
        Base::Float
    } else if text.starts_with("Vec") {
        Base::List
    } else {
        Base::Value
    };
    Ty::opt(base)
}

pub(super) fn static_kind(ty: &syn::Type) -> StaticKind {
    let text = type_text(ty);
    match text.as_str() {
        "Once" => StaticKind::Once,
        "OnceLock" => StaticKind::OnceLock,
        "LazyLock" => StaticKind::Lazy,
        "Mutex" => StaticKind::Mutex,
        "RwLock" => StaticKind::RwLock,
        _ if text.starts_with("Atomic") => StaticKind::Atomic,
        _ => StaticKind::Value,
    }
}
