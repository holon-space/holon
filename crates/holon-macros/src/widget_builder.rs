use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Attribute, Block, Expr, Ident, Token, Type,
    parse::{Parse, ParseStream},
};

// ─── Types ──────────────────────────────────────────────────────────

struct WidgetBuilderInput {
    name: Ident,
    params: Vec<WidgetParam>,
    body: Option<Block>,
}

struct WidgetParam {
    name: Ident,
    ty: ParamType,
    default: Option<Expr>,
}

#[derive(Clone, Copy, PartialEq)]
enum ParamType {
    String,
    OptionalString,
    Bool,
    F64,
    F32,
    Value,
    Collection,
    Expr,
}

// ─── Parsing ────────────────────────────────────────────────────────

impl Parse for WidgetBuilderInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // Optional visibility
        let _vis: Option<syn::Visibility> = if input.peek(Token![pub]) {
            Some(input.parse()?)
        } else {
            None
        };

        input.parse::<Token![fn]>()?;
        let name: Ident = input.parse()?;

        // Parse (params)
        let content;
        syn::parenthesized!(content in input);
        let params = parse_params(&content)?;

        // Optional return type
        if input.peek(Token![->]) {
            input.parse::<Token![->]>()?;
            let _ret_ty: Type = input.parse()?;
        }

        // Body or semicolon
        let body = if input.peek(Token![;]) {
            input.parse::<Token![;]>()?;
            None
        } else if input.peek(syn::token::Brace) {
            Some(input.parse::<Block>()?)
        } else {
            None
        };

        Ok(WidgetBuilderInput { name, params, body })
    }
}

fn parse_params(input: ParseStream) -> syn::Result<Vec<WidgetParam>> {
    let mut params = Vec::new();
    while !input.is_empty() {
        let attrs = input.call(Attribute::parse_outer)?;
        let default = extract_default(&attrs)?;

        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let ty = parse_param_type(input)?;

        params.push(WidgetParam { name, ty, default });

        if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
        }
    }
    Ok(params)
}

fn extract_default(attrs: &[Attribute]) -> syn::Result<Option<Expr>> {
    for attr in attrs {
        if attr.path().is_ident("default") {
            let nv = attr.meta.require_name_value()?;
            return Ok(Some(nv.value.clone()));
        }
    }
    Ok(None)
}

fn parse_param_type(input: ParseStream) -> syn::Result<ParamType> {
    let ty: Type = input.parse()?;
    let ty_str = match &ty {
        Type::Path(tp) => tp
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => String::new(),
    };
    match ty_str.as_str() {
        "String" => Ok(ParamType::String),
        "bool" => Ok(ParamType::Bool),
        "f64" => Ok(ParamType::F64),
        "f32" => Ok(ParamType::F32),
        "Value" => Ok(ParamType::Value),
        "Collection" => Ok(ParamType::Collection),
        "Expr" => Ok(ParamType::Expr),
        "Option" => {
            // Parse Option<String> — extract the inner type
            if let Type::Path(tp) = &ty {
                if let Some(seg) = tp.path.segments.last() {
                    if let syn::PathArguments::AngleBracketed(args) = &seg.arguments {
                        if let Some(syn::GenericArgument::Type(Type::Path(inner))) =
                            args.args.first()
                        {
                            if inner.path.is_ident("String") {
                                return Ok(ParamType::OptionalString);
                            }
                        }
                    }
                }
            }
            Err(syn::Error::new_spanned(
                ty,
                "only Option<String> is supported",
            ))
        }
        other => Err(syn::Error::new_spanned(
            ty,
            format!(
                "unsupported param type `{other}`, expected one of: String, Option<String>, bool, f32, f64, Value, Collection, Expr"
            ),
        )),
    }
}

// ─── Code Generation ────────────────────────────────────────────────

fn generate_extraction(params: &[WidgetParam]) -> proc_macro2::TokenStream {
    let mut positional_idx = 0usize;
    let mut extractions = Vec::new();

    for param in params {
        let name = &param.name;
        let name_str = name.to_string();

        let extraction = match param.ty {
            ParamType::String => {
                let idx = positional_idx;
                positional_idx += 1;
                let default = match &param.default {
                    Some(expr) => quote!(#expr),
                    None => quote!(""),
                };
                quote! {
                    let #name = ba.args.get_positional_string(#idx)
                        .or(ba.args.get_string(#name_str))
                        .unwrap_or(#default)
                        .to_string();
                }
            }
            ParamType::Bool => {
                let default = match &param.default {
                    Some(expr) => quote!(#expr),
                    None => quote!(false),
                };
                quote! {
                    let #name = ba.args.get_bool(#name_str).unwrap_or(#default);
                }
            }
            ParamType::OptionalString => {
                quote! {
                    let #name: Option<String> = ba.args.get_positional_string(#positional_idx)
                        .or(ba.args.get_string(#name_str))
                        .map(|s| s.to_string());
                }
            }
            ParamType::F64 => {
                let default = match &param.default {
                    Some(expr) => quote!(#expr),
                    None => quote!(0.0),
                };
                quote! {
                    let #name = ba.args.get_f64(#name_str).unwrap_or(#default);
                }
            }
            ParamType::F32 => {
                let default = match &param.default {
                    Some(expr) => quote!(#expr),
                    None => quote!(0.0_f32),
                };
                quote! {
                    let #name = ba.args.get_f64(#name_str).map(|v| v as f32).unwrap_or(#default);
                }
            }
            ParamType::Value => {
                let idx = positional_idx;
                positional_idx += 1;
                quote! {
                    let #name = ba.args.positional.get(#idx)
                        .cloned()
                        .or_else(|| ba.args.named.get(#name_str).cloned())
                        .unwrap_or(Value::Null);
                }
            }
            ParamType::Collection => {
                quote! {
                    let #name: Vec<ViewModel> = {
                        let __template = ba.args.get_template("item_template")
                            .or(ba.args.get_template("item"));
                        match __template {
                            Some(__tmpl) => {
                                if ba.ctx.data_rows.is_empty() {
                                    vec![(ba.interpret)(__tmpl, ba.ctx)]
                                } else {
                                    ba.ctx.data_rows.iter()
                                        .map(|__row| {
                                            let __row_ctx = ba.ctx.with_row(__row.clone());
                                            (ba.interpret)(__tmpl, &__row_ctx)
                                        })
                                        .collect()
                                }
                            }
                            None => {
                                if ba.args.positional_exprs.is_empty() {
                                    ba.ctx.data_rows.iter()
                                        .map(|__row| ViewModel::element("row", __row.clone(), vec![]))
                                        .collect()
                                } else {
                                    ba.args.positional_exprs.iter()
                                        .map(|__expr| (ba.interpret)(__expr, ba.ctx))
                                        .collect()
                                }
                            }
                        }
                    };
                }
            }
            ParamType::Expr => {
                quote! {
                    let #name = ba.args.get_template(#name_str);
                }
            }
        };

        extractions.push(extraction);
    }

    quote! { #(#extractions)* }
}

fn generate_auto_body(widget_name: &str, params: &[WidgetParam]) -> proc_macro2::TokenStream {
    use crate::builder_registry::snake_to_pascal;

    let variant = Ident::new(
        &snake_to_pascal(widget_name),
        proc_macro2::Span::call_site(),
    );

    let field_assignments: Vec<_> = params
        .iter()
        .map(|p| {
            let name = &p.name;
            match p.ty {
                ParamType::Collection => {
                    quote! { #name: LazyChildren::fully_materialized(#name) }
                }
                _ => {
                    quote! { #name }
                }
            }
        })
        .collect();

    let kind = if field_assignments.is_empty() {
        quote! { NodeKind::#variant }
    } else {
        quote! { NodeKind::#variant { #(#field_assignments),* } }
    };

    quote! {
        ViewModel {
            kind: #kind,
            ..Default::default()
        }
    }
}

fn generate_meta(widget_name: &str, params: &[WidgetParam]) -> proc_macro2::TokenStream {
    let has_collection = params.iter().any(|p| p.ty == ParamType::Collection);
    let has_expr = params.iter().any(|p| p.ty == ParamType::Expr);
    let data_count = params
        .iter()
        .filter(|p| !matches!(p.ty, ParamType::Collection | ParamType::Expr))
        .count();

    let category = if has_collection {
        quote! { holon_api::WidgetCategory::Collection }
    } else if has_expr {
        quote! { holon_api::WidgetCategory::Special }
    } else if data_count > 2 {
        quote! { holon_api::WidgetCategory::Element }
    } else {
        quote! { holon_api::WidgetCategory::Leaf }
    };

    let static_params: Vec<_> = params
        .iter()
        .map(|p| {
            let name = p.name.to_string();
            let type_hint = match p.ty {
                ParamType::String => "String",
                ParamType::OptionalString => "String",
                ParamType::Bool => "Bool",
                ParamType::F64 => "Number",
                ParamType::F32 => "Number",
                ParamType::Value => "Value",
                ParamType::Collection => "Collection",
                ParamType::Expr => "Expr",
            };
            let default_expr = match &p.default {
                Some(expr) => {
                    let s = quote!(#expr).to_string();
                    quote! { Some(#s) }
                }
                None => quote! { None },
            };
            quote! {
                holon_api::StaticParam {
                    name: #name,
                    type_hint: #type_hint,
                    default: #default_expr,
                }
            }
        })
        .collect();

    quote! {
        pub const WIDGET_META: holon_api::WidgetMeta = holon_api::WidgetMeta {
            name: #widget_name,
            category: #category,
            params: &[#(#static_params),*],
            doc: "",
        };
    }
}

// ─── Entry Point ────────────────────────────────────────────────────

/// Unified entry for `widget_builder!` function-like macro.
///
/// Supports three forms:
/// 1. `widget_builder! { fn badge(label: String); }` — auto-body
/// 2. `widget_builder! { fn section(title: String, children: Collection) { ... } }` — extraction + user body
/// 3. `widget_builder! { raw fn tree(ba: BA<'_>) -> ViewModel { ... } }` — raw, just WIDGET_META
pub fn widget_builder_impl(input: TokenStream) -> TokenStream {
    // Check if first token is `raw`
    let input_str = input.to_string();
    if input_str.starts_with("raw ") || input_str.starts_with("raw\n") {
        // Strip `raw` keyword and parse the rest as an ItemFn
        let trimmed: proc_macro2::TokenStream =
            syn::parse(input).expect("failed to parse widget_builder input");
        let mut iter = trimmed.into_iter();
        // Skip the `raw` ident
        let first = iter.next().expect("expected `raw` keyword");
        assert!(
            matches!(&first, proc_macro2::TokenTree::Ident(id) if id == "raw"),
            "expected `raw` keyword"
        );
        let rest: proc_macro2::TokenStream = iter.collect();
        let mut item_fn: syn::ItemFn =
            syn::parse2(rest).expect("widget_builder(raw) requires a complete function body");

        let widget_name = item_fn.sig.ident.to_string();
        let meta = generate_meta(&widget_name, &[]);

        // Rename function to `build` (builder_registry expects module::build)
        item_fn.sig.ident = Ident::new("build", item_fn.sig.ident.span());
        // Ensure pub visibility
        item_fn.vis = syn::Visibility::Public(syn::token::Pub {
            span: proc_macro2::Span::call_site(),
        });

        let expanded = quote! {
            #meta
            #item_fn
        };
        TokenStream::from(expanded)
    } else {
        let input: WidgetBuilderInput =
            syn::parse(input).expect("failed to parse widget_builder input");
        let widget_name = input.name.to_string();
        let meta = generate_meta(&widget_name, &input.params);
        let extraction = generate_extraction(&input.params);

        let body = match input.body {
            Some(block) => {
                let stmts = &block.stmts;
                quote! { #(#stmts)* }
            }
            None => generate_auto_body(&widget_name, &input.params),
        };

        let expanded = quote! {
            #meta

            pub fn build(ba: BA<'_>) -> ViewModel {
                #extraction
                #body
            }
        };
        TokenStream::from(expanded)
    }
}
