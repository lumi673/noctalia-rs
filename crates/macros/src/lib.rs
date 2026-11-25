use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    Error, Ident, ItemFn, LitStr, Result, Token, parse::Parse,
    parse::ParseStream,
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
}
