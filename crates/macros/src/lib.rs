use proc_macro::TokenStream;
use proc_macro2::{Literal, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{
    DeriveInput, Error, Ident, ItemFn, LitStr, Result, Token, parse::Parse,
    parse::ParseStream,
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
}
