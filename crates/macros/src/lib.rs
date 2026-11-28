use proc_macro::TokenStream;
use proc_macro2::{Literal, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::spanned::Spanned;
use syn::{
    Data, DeriveInput, Error, Expr, ExprLit, Fields, Ident, ItemFn, Lit,
    LitStr, Meta, Result, Token, parse::Parse, parse::ParseStream,
    punctuated::Punctuated,
};

use noctalia_rs_compiler::config::{
    self, ConfigField, ConfigKind, ConfigLiteral,
};

struct Args {
    kind: String,
    id: String,
}

impl Parse for Args {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut kind_value = None;
        let mut id_value = None;

        while !input.is_empty() {
            parse_assignment(input, &mut kind_value, &mut id_value)?;
            consume_comma(input)?;
        }

        let kind = kind_value
            .ok_or_else(|| Error::new(input.span(), "missing kind"))?;
        let id =
            id_value.ok_or_else(|| Error::new(input.span(), "missing id"))?;

        Ok(Self { kind, id })
    }
}

fn parse_assignment(
    input: ParseStream<'_>,
    kind_value: &mut Option<String>,
    id_value: &mut Option<String>,
) -> Result<()> {
    let key: Ident = input.parse()?;
    input.parse::<Token![=]>()?;
    let value: LitStr = input.parse()?;

    match key.to_string().as_str() {
        "kind" => *kind_value = Some(value.value()),
        "id" => *id_value = Some(value.value()),
        other => {
            return Err(Error::new(
                value.span(),
                format!("unknown key {other}"),
            ));
        }
    }

    Ok(())
}

fn consume_comma(input: ParseStream<'_>) -> Result<()> {
    if input.peek(Token![,]) {
        input.parse::<Token![,]>()?;
    }
    Ok(())
}

/// Reads declared config keys into a typed struct.
///
/// Each field needs a literal `#[config(default = ...)]` that matches the
/// manifest. The compiler lowers `Config::from_ctx` to direct table reads;
/// this impl is what an `off` build runs natively.
#[proc_macro_derive(Config, attributes(config))]
pub fn config_derive(tokens: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(tokens as DeriveInput);
    match expand_config(&input) {
        Ok(expanded) => expanded.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand_config(input: &DeriveInput) -> Result<TokenStream2> {
    let def = config::parse(input)?.ok_or_else(|| {
        Error::new(input.ident.span(), "Config derive was not applied")
    })?;
    let name = &input.ident;
    let (impl_generics, type_generics, where_clause) =
        input.generics.split_for_impl();
    let reads = def.fields.iter().map(field_read).collect::<Vec<_>>();
    let defaults = def
        .fields
        .iter()
        .map(|field| {
            let ident = format_ident!("{}", field.name);
            let value = literal(&field.default);
            quote!(#ident: #value)
        })
        .collect::<Vec<_>>();
    Ok(quote! {
        impl #impl_generics #name #type_generics #where_clause {
            #[must_use]
            pub fn from_ctx(ctx: &::noctalia_rs::Context) -> Self {
                Self { #(#reads),* }
            }
        }

        impl #impl_generics ::std::default::Default for #name #type_generics #where_clause {
            fn default() -> Self {
                Self { #(#defaults),* }
            }
        }
    })
}

fn field_read(field: &ConfigField) -> TokenStream2 {
    let ident = format_ident!("{}", field.name);
    let key = &field.key;
    let default = literal(&field.default);
    match field.kind {
        ConfigKind::Str => quote! {
            #ident: ctx.config_str(#key)
                .map(|value| value.into_owned())
                .unwrap_or_else(|| #default.to_owned())
        },
        ConfigKind::Bool => quote! {
            #ident: ctx.config_bool(#key, #default)
        },
        ConfigKind::I32 => quote! {
            #ident: ctx.config_str(#key)
                .and_then(|value| value.parse::<i32>().ok())
                .unwrap_or(#default)
        },
        ConfigKind::U32 => quote! {
            #ident: ctx.config_str(#key)
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(#default)
        },
        ConfigKind::F64 => quote! {
            #ident: ctx.config_str(#key)
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or(#default)
        },
    }
}

fn literal(default: &ConfigLiteral) -> TokenStream2 {
    match default {
        ConfigLiteral::Str(text) => quote!(#text),
        ConfigLiteral::Bool(flag) => quote!(#flag),
        ConfigLiteral::Int(number) => {
            let token = Literal::i128_unsuffixed(*number);
            quote!(#token)
        }
        ConfigLiteral::Float(number) => {
            let token = Literal::f64_unsuffixed(*number);
            quote!(#token)
        }
    }
}
/// Keeps a function in the plugin binary and calls it across the native
/// boundary. Inert: the build script reads the attribute from the source text.
#[proc_macro_attribute]
pub fn native(_attr: TokenStream, item: TokenStream) -> TokenStream {
    unchanged(item)
}

/// Requires the function to compile to Luau, failing the build otherwise.
/// Inert: the build script reads the attribute from the source text.
#[proc_macro_attribute]
pub fn luau(_attr: TokenStream, item: TokenStream) -> TokenStream {
    unchanged(item)
}

/// Marker attributes must leave the item byte-for-byte identical: the build
/// script parses the source text, not the expanded item.
const fn unchanged(item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn entry(attr: TokenStream, item: TokenStream) -> TokenStream {
    match expand(attr.into(), item.into()) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

fn expand(attr: TokenStream2, item: TokenStream2) -> Result<TokenStream2> {
    let args: Args = syn::parse2(attr)?;
    let function: ItemFn = syn::parse2(item)?;
    let name = &function.sig.ident;
    let entry_name = format_ident!("{}_entry", name);
    let id = &args.id;
    let kind = kind_expression(&args.kind)?;

    Ok(quote! {
        #function

        #[must_use]
        pub fn #entry_name() -> ::noctalia_rs::Entry {
            ::noctalia_rs::Entry::new(
                (#id.to_string(), #kind, ::std::vec::Vec::new()),
                #name,
            )
        }
    })
}

fn kind_expression(kind: &str) -> Result<TokenStream2> {
    let expression = match kind {
        "service" => quote!(::noctalia_rs::EntryKind::Service),
        "widget" => quote!(::noctalia_rs::EntryKind::Widget),
        "shortcut" => quote!(::noctalia_rs::EntryKind::Shortcut),
        "desktop_widget" => quote!(::noctalia_rs::EntryKind::DesktopWidget),
        "launcher" => quote!(::noctalia_rs::EntryKind::LauncherProvider {
            prefix: "plugin".to_owned(),
            glyph: "puzzle".to_owned(),
        }),
        "panel" => quote!(::noctalia_rs::EntryKind::Panel {
            width: 320,
            height: 240,
        }),
        "layer" => quote!(::noctalia_rs::EntryKind::Layer {
            config: ::std::boxed::Box::new(::noctalia_rs::LayerConfig::new()),
        }),
        other => {
            return Err(Error::new(
                proc_macro2::Span::call_site(),
                format!("unknown entry kind {other}"),
            ));
        }
    };

    Ok(expression)
}
/// Derives a typed state table with a host key, default values, and
/// load/save/watch helpers that compile to direct plugin API 57 calls.
#[proc_macro_derive(State, attributes(state))]
pub fn derive_state(item: TokenStream) -> TokenStream {
    match expand_state(item.into()) {
        Ok(tokens) => tokens.into(),
        Err(error) => error.to_compile_error().into(),
    }
}

struct StateField {
    name: Ident,
    default: Option<Expr>,
    skip: bool,
}

#[expect(
    clippy::too_many_lines,
    reason = "derive emits the trait impl and the inherent API together"
)]
fn expand_state(item: TokenStream2) -> Result<TokenStream2> {
    let input: DeriveInput = syn::parse2(item)?;
    let Data::Struct(data) = &input.data else {
        return Err(Error::new(
            input.ident.span(),
            "State can only be derived for named structs",
        ));
    };
    let Fields::Named(fields) = &data.fields else {
        return Err(Error::new(
            input.ident.span(),
            "State requires a struct with named fields",
        ));
    };
    let name = &input.ident;
    let key = state_key(&input.attrs, name)?;
    let parsed = state_fields(fields)?;
    let reads = parsed.iter().map(|field| {
        let ident = &field.name;
        let label = ident.to_string();
        let default = field_default(field);
        if field.skip {
            quote!(#ident: #default)
        } else {
            quote!(#ident: ::noctalia_rs::state::field(value.get(#label), || #default))
        }
    });
    let writes = parsed.iter().filter(|field| !field.skip).map(|field| {
        let ident = &field.name;
        let label = ident.to_string();
        quote!(::noctalia_rs::state::insert(&mut object, #label, &self.#ident))
    });
    let (impl_generics, type_generics, where_clause) =
        input.generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics ::noctalia_rs::State for #name #type_generics
            #where_clause
        {
            const KEY: &'static str = #key;

            fn defaults() -> ::noctalia_rs::Value {
                let value = <Self as ::noctalia_rs::State>::from_value(
                    &::noctalia_rs::Value::Object(
                        ::noctalia_rs::state::object(),
                    ),
                );
                <Self as ::noctalia_rs::State>::to_value(&value)
            }

            fn from_value(value: &::noctalia_rs::Value) -> Self {
                Self { #(#reads),* }
            }

            fn to_value(&self) -> ::noctalia_rs::Value {
                let mut object = ::noctalia_rs::state::object();
                #(#writes;)*
                ::noctalia_rs::Value::Object(object)
            }
        }

        impl #impl_generics #name #type_generics #where_clause {
            /// Host state key for this table.
            pub const KEY: &'static str = #key;

            /// Default table written when the host has no stored state.
            pub fn defaults() -> ::noctalia_rs::Value {
                <Self as ::noctalia_rs::State>::defaults()
            }

            /// Reads a typed table from a JSON value.
            pub fn from_value(value: &::noctalia_rs::Value) -> Self {
                <Self as ::noctalia_rs::State>::from_value(value)
            }

            /// Writes this table as JSON, omitting `None` fields.
            pub fn to_value(&self) -> ::noctalia_rs::Value {
                <Self as ::noctalia_rs::State>::to_value(self)
            }

            /// Reads this table from the host state service.
            pub fn load(ctx: &::noctalia_rs::Context) -> Self {
                ::noctalia_rs::state::load::<Self>(ctx)
            }

            /// Writes this table to the host state service.
            pub fn save(
                &self,
                ops: &mut ::std::vec::Vec<::noctalia_rs::Op>,
            ) {
                ::noctalia_rs::state::save(self, ops)
            }

            /// Watches the host key of this table.
            pub fn watch(
                ops: &mut ::std::vec::Vec<::noctalia_rs::Op>,
                callback_id: &str,
            ) {
                ::noctalia_rs::state::watch::<Self>(ops, callback_id)
            }
        }
    })
}

fn state_key(attrs: &[syn::Attribute], name: &Ident) -> Result<String> {
    let mut key = snake_case(&name.to_string());
    let groups = state_attrs(attrs)
        .map(state_metas)
        .collect::<Result<Vec<_>>>()?;
    for meta in groups.into_iter().flatten() {
        let Some(value) = key_meta(meta)? else {
            continue;
        };
        key = value;
    }
    Ok(key)
}

fn state_attrs(
    attrs: &[syn::Attribute],
) -> impl Iterator<Item = &syn::Attribute> {
    attrs.iter().filter(|attr| attr.path().is_ident("state"))
}

fn key_meta(meta: Meta) -> Result<Option<String>> {
    let Meta::NameValue(value) = meta else {
        return Err(Error::new_spanned(meta, "unknown state struct option"));
    };
    if !value.path.is_ident("key") {
        return Err(Error::new_spanned(value, "unknown state struct option"));
    }
    let Expr::Lit(ExprLit {
        lit: Lit::Str(text),
        ..
    }) = &value.value
    else {
        return Err(Error::new(
            value.value.span(),
            "state key must be a string literal",
        ));
    };
    Ok(Some(text.value()))
}

fn state_fields(named: &syn::FieldsNamed) -> Result<Vec<StateField>> {
    named.named.iter().map(field_spec).collect()
}

fn field_spec(field: &syn::Field) -> Result<StateField> {
    let name = field.ident.clone().ok_or_else(|| {
        Error::new(field.span(), "State requires a named field")
    })?;
    let mut default = None;
    let mut skip = false;
    for attr in state_attrs(&field.attrs) {
        for meta in state_metas(attr)? {
            field_meta(meta, &mut default, &mut skip)?;
        }
    }
    Ok(StateField {
        name,
        default,
        skip,
    })
}

fn field_meta(
    meta: Meta,
    default: &mut Option<Expr>,
    skip: &mut bool,
) -> Result<()> {
    match meta {
        Meta::Path(path) if path.is_ident("skip") => *skip = true,
        Meta::NameValue(value) if value.path.is_ident("default") => {
            *default = Some(value.value);
        }
        other @ (Meta::Path(_) | Meta::List(_) | Meta::NameValue(_)) => {
            return Err(Error::new_spanned(
                other,
                "unknown state field option",
            ));
        }
    }
    Ok(())
}

fn state_metas(attr: &syn::Attribute) -> Result<Vec<Meta>> {
    Ok(attr
        .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?
        .into_iter()
        .collect())
}

fn field_default(field: &StateField) -> TokenStream2 {
    field.default.as_ref().map_or_else(
        || quote!(::std::default::Default::default()),
        |value| quote!(#value),
    )
}

fn snake_case(name: &str) -> String {
    let mut out = String::new();
    let mut chars = name.chars().peekable();
    let mut previous: Option<char> = None;
    while let Some(character) = chars.next() {
        let separator = character.is_ascii_uppercase()
            && (previous.is_some_and(|item| item.is_ascii_lowercase())
                || (previous.is_some()
                    && chars.peek().is_some_and(char::is_ascii_lowercase)));
        if separator {
            out.push('_');
        }
        if character.is_ascii_uppercase() {
            out.push(character.to_ascii_lowercase());
        } else {
            out.push(character);
        }
        previous = Some(character);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_generates_entry_function() {
        let tokens = expand(
            quote!(kind = "service", id = "service"),
            quote!(
                fn update(
                    _ctx: &::noctalia_rs::Context,
                ) -> ::noctalia_rs::Result<Vec<::noctalia_rs::Op>>
                {
                    Ok(Vec::new())
                }
            ),
        )
        .expect("valid input")
        .to_string();

        assert!(tokens.contains("update_entry"));
        assert!(tokens.contains("EntryKind :: Service"));
    }

    #[test]
    fn test_expand_rejects_missing_id() {
        let error = expand(
            quote!(kind = "service"),
            quote!(
                fn update() {}
            ),
        )
        .expect_err("missing id");

        assert_eq!(error.to_string(), "missing id");
    }

    #[test]
    fn test_expand_rejects_unknown_kind() {
        let error = expand(
            quote!(kind = "other", id = "other"),
            quote!(
                fn update() {}
            ),
        )
        .expect_err("unknown kind");

        assert_eq!(error.to_string(), "unknown entry kind other");
    }
    #[test]
    fn config_derive_generates_typed_reads_and_defaults() {
        let input: DeriveInput = syn::parse_quote! {
            #[derive(Config)]
            struct Settings {
                #[config(key = "message", default = "hello")]
                message: String,
                #[config(key = "enabled", default = false)]
                enabled: bool,
                #[config(key = "count", default = 3)]
                count: u32,
            }
        };
        let tokens = expand_config(&input)
            .expect("valid config struct")
            .to_string();

        assert!(tokens.contains("from_ctx"), "{tokens}");
        assert!(tokens.contains("config_str"), "{tokens}");
        assert!(tokens.contains("config_bool"), "{tokens}");
        assert!(tokens.contains("parse :: < u32 >"), "{tokens}");
        assert!(tokens.contains("Default for Settings"), "{tokens}");
    }

    #[test]
    fn config_derive_rejects_an_unsupported_field_type() {
        let input: DeriveInput = syn::parse_quote! {
            #[derive(Config)]
            struct Settings {
                #[config(default = 1)]
                count: i64,
            }
        };
        let error = expand_config(&input).expect_err("i64 is not exact");
        assert!(error.to_string().contains("unsupported config type"));
    }

    #[test]
    fn state_derive_generates_defaults_and_persistence() {
        let tokens = expand_state(quote! {
            #[state(key = "panel.counter")]
            struct Counter {
                #[state(default = 4)]
                count: i64,
                label: Option<String>,
                #[state(skip)]
                cache: i64,
            }
        })
        .expect("valid state")
        .to_string();

        assert!(tokens.contains("\"panel.counter\""), "{tokens}");
        assert!(tokens.contains("value . get (\"count\")"), "{tokens}");
        assert!(tokens.contains('4'), "{tokens}");
        assert!(
            !tokens.contains("insert (& mut object , \"cache\""),
            "{tokens}"
        );
        assert!(tokens.contains("state :: load"), "{tokens}");
        assert!(tokens.contains("state :: save"), "{tokens}");
        assert!(tokens.contains("state :: watch"), "{tokens}");
    }

    #[test]
    fn state_derive_defaults_to_a_snake_case_key() {
        assert_eq!(snake_case("AppState"), "app_state");
        assert_eq!(snake_case("Counter"), "counter");

        let tokens = expand_state(quote! {
            struct AppState { enabled: bool }
        })
        .expect("valid state")
        .to_string();
        assert!(tokens.contains("\"app_state\""), "{tokens}");
    }

    #[test]
    fn state_derive_rejects_tuple_structs() {
        let error = expand_state(quote! {
            struct Counter(i64);
        })
        .expect_err("tuple struct");
        assert_eq!(
            error.to_string(),
            "State requires a struct with named fields"
        );
    }
}
