use proc_macro::TokenStream;
use quote::quote;
use syn::Attribute;
use syn::Block;
use syn::Expr;
use syn::Ident;
use syn::Token;
use syn::Type;
use syn::parse::Parse;
use syn::parse::ParseStream;

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
            if let Type::Path(tp) = &ty
                && let Some(seg) = tp.path.segments.last()
                && let syn::PathArguments::AngleBracketed(args) = &seg.arguments
                && let Some(syn::GenericArgument::Type(Type::Path(inner))) = args.args.first()
                && inner.path.is_ident("String")
            {
                return Ok(ParamType::OptionalString);
            }
            Err(syn::Error::new_spanned(
                ty,
                "only Option<String> is supported",
            ))
        }
        other => Err(syn::Error::new_spanned(
            ty,
            format!(
                "unsupported param type `{other}`, expected one of: String, Option<String>, bool, \
                 f32, f64, Value, Collection, Expr"
            ),
        )),
    }
}

// ─── Code Generation ────────────────────────────────────────────────

/// The positional slot each param binds to, in declaration order. String,
/// Option<String>, f64, f32 and Value params take the next slot; a bool binds
/// by name only. With a `Collection` param every positional arg is a child, so
/// no param has a slot and `row(col("a"), col("b"))` draws two children rather
/// than reading `col("a")` as the row's `gap`.
fn positional_slots(params: &[WidgetParam]) -> Vec<Option<usize>> {
    let has_collection = params.iter().any(|p| p.ty == ParamType::Collection);
    let mut next = 0;
    params
        .iter()
        .map(|p| match p.ty {
            ParamType::String
            | ParamType::OptionalString
            | ParamType::F64
            | ParamType::F32
            | ParamType::Value
                if !has_collection =>
            {
                next += 1;
                Some(next - 1)
            }
            _ => None,
        })
        .collect()
}

/// Extraction of a String / Option<String> / bool / f64 / f32 / Value param,
/// or `None` for a Collection / Expr param. Absent takes the default; a value
/// of the wrong type runs `on_err` with the message naming the key.
fn scalar_extraction(
    param: &WidgetParam,
    slot: Option<usize>,
    on_err: &proc_macro2::TokenStream,
) -> Option<proc_macro2::TokenStream> {
    let name = &param.name;
    let name_str = name.to_string();
    let slot = || match slot {
        Some(idx) => quote!(Some(#idx)),
        None => quote!(None),
    };
    let default = |unset: proc_macro2::TokenStream| match &param.default {
        Some(expr) => quote!(#expr),
        None => unset,
    };
    let read = match param.ty {
        ParamType::String => {
            let slot = slot();
            let default = default(quote!(""));
            quote! {
                ba.args.param_string(#slot, #name_str)
                    .map(|v| v.unwrap_or_else(|| #default.to_string()))
            }
        }
        ParamType::OptionalString => {
            let slot = slot();
            quote! { ba.args.param_string(#slot, #name_str) }
        }
        ParamType::Bool => {
            let default = default(quote!(false));
            quote! { ba.args.param_bool(#name_str).map(|v| v.unwrap_or(#default)) }
        }
        ParamType::F64 => {
            let slot = slot();
            let default = default(quote!(0.0));
            quote! { ba.args.param_f64(#slot, #name_str).map(|v| v.unwrap_or(#default)) }
        }
        ParamType::F32 => {
            let slot = slot();
            let default = default(quote!(0.0_f32));
            quote! {
                ba.args.param_f64(#slot, #name_str).map(|v| v.map_or(#default, |v| v as f32))
            }
        }
        ParamType::Value => {
            let slot = slot();
            return Some(quote! {
                let #name = #slot.and_then(|__i: usize| ba.args.positional.get(__i))
                    .cloned()
                    .or_else(|| ba.args.named.get(#name_str).cloned())
                    .unwrap_or(Value::Null);
            });
        }
        ParamType::Collection | ParamType::Expr => return None,
    };
    Some(quote! {
        let #name = match #read {
            Ok(v) => v,
            Err(__e) => #on_err,
        };
    })
}

fn generate_extraction(widget_name: &str, params: &[WidgetParam]) -> proc_macro2::TokenStream {
    let mut extractions = Vec::new();

    for (param, slot) in params.iter().zip(positional_slots(params)) {
        let name = &param.name;
        let name_str = name.to_string();

        let on_err = quote!(return ViewModel::error(#widget_name, __e));
        if let Some(extraction) = scalar_extraction(param, slot, &on_err) {
            extractions.push(extraction);
            continue;
        }
        let extraction = match param.ty {
            ParamType::Collection => {
                quote! {
                    let #name: crate::reactive_view_model::CollectionData = {
                        let __template = ba.args.get_template("item_template")
                            .or(ba.args.get_template("item"))
                            .cloned();
                        // Extract sort_key as a plain column name. The DSL
                        // only supports `sort_key: col("name")`; anything else
                        // silently yields None (no sort). This matches the
                        // PR4 plan's restriction — expression-derived keys
                        // would hit Rhai int/float type ambiguity and have
                        // been ruled out.
                        let __sort_key: Option<String> = holon_api::render_eval::sort_key_column(ba.args)
                            .map(|s| s.to_string());
                        // Data-source precedence: an explicit `collection:`
                        // named arg (populated by `resolve_args_with` when a
                        // value-fn returns `InterpValue::Rows`) wins over
                        // the inherited `ctx.data_source`. The inherited
                        // value is kept as the secondary source so profiles
                        // that rely on parent-block data (no explicit
                        // `collection:`) keep working byte-for-byte. // ALLOW(fallback): doc comment about inheritance precedence
                        let __explicit_ds: Option<std::sync::Arc<dyn holon_api::ReactiveRowProvider>>
                            = ba.args.get_rows("collection");
                        let __ds: Option<std::sync::Arc<dyn holon_api::ReactiveRowProvider>> =
                            __explicit_ds.or_else(|| {
                                ba.ctx.data_source.clone()
                                    .map(|r| r as std::sync::Arc<dyn holon_api::ReactiveRowProvider>)
                            });
                        // Parse `rules:` once for both arms (FU-6). Streaming
                        // and Static both apply the same pipeline; positional
                        // context differs (streaming has no count/is_last,
                        // static has all four).
                        let __rules = match crate::row_pipeline::parse_rules_arg(
                            ba.args.named.get("rules"),
                        ) {
                            Ok(rules) => rules,
                            Err(msg) => return ViewModel::error(#widget_name, msg),
                        };
                        match (__template, __ds) {
                            // Live data source + explicit template → Streaming.
                            // Zero eager interpretation — the signal_vec driver
                            // seeds items from the current snapshot on first poll.
                            (Some(tmpl), Some(ds)) => {
                                crate::reactive_view_model::CollectionData::Streaming {
                                    item_template: tmpl,
                                    data_source: ds,
                                    sort_key: __sort_key,
                                    rules: __rules,
                                }
                            }
                            // Template but no data source: headless/snapshot path.
                            // Eagerly interpret ctx.data_rows through the template,
                            // sorted by sort_key if present (matches streaming behaviour).
                            //
                            // Per-row pipeline (rules: + profile/ops + interpret) goes
                            // through `crate::row_pipeline::apply_full_row_pipeline` so
                            // every collection-arm builder gets `rules:` support without
                            // touching the macro further. Positional context injects
                            // `position`/`count`/`is_first`/`is_last` so rules can match
                            // on collection position.
                            (Some(tmpl), None) => {
                                let __sorted = holon_api::render_eval::sorted_rows(
                                    &ba.ctx.data_rows,
                                    __sort_key.as_deref(),
                                );
                                let __count = __sorted.len();
                                let items = __sorted.into_iter()
                                    .enumerate()
                                    .map(|(__i, __row)| {
                                        let __positional = std::collections::HashMap::from([
                                            ("position".to_string(), holon_api::Value::Integer(__i as i64)),
                                            ("count".to_string(), holon_api::Value::Integer(__count as i64)),
                                            ("is_first".to_string(), holon_api::Value::Boolean(__i == 0)),
                                            ("is_last".to_string(), holon_api::Value::Boolean(__i + 1 == __count)),
                                            ("is_empty_collection".to_string(), holon_api::Value::Boolean(__count == 0)),
                                        ]);
                                        let (__node, _) = crate::row_pipeline::apply_full_row_pipeline(
                                            ba.services,
                                            ba.ctx,
                                            &tmpl,
                                            &__rules,
                                            &__row,
                                            __positional,
                                            |__expr, __c| (ba.interpret)(__expr, __c),
                                        );
                                        __node
                                    })
                                    .collect();
                                crate::reactive_view_model::CollectionData::Static { items }
                            }
                            // Positional children (col/row/section with literal children).
                            (None, _) if !ba.args.positional_exprs.is_empty() => {
                                let items = ba.args.positional_exprs.iter()
                                    .map(|__expr| (ba.interpret)(__expr, ba.ctx))
                                    .collect();
                                crate::reactive_view_model::CollectionData::Static { items }
                            }
                            // No template, no positional — render each row
                            // as a bare `row` element. Rare, mostly legacy. // ALLOW(fallback): rare-legacy default branch

                            (None, _) => {
                                let items = ba.ctx.data_rows.iter()
                                    .map(|__row| ViewModel::element("row", __row.clone(), vec![]))
                                    .collect();
                                crate::reactive_view_model::CollectionData::Static { items }
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
            _ => unreachable!("scalar params were extracted above"),
        };

        extractions.push(extraction);
    }

    quote! { #(#extractions)* }
}

/// Generate the body of `resolve_props_from_args`: extraction of simple params
/// (String, bool, f64, f32, Option<String>, Value) into a `HashMap<String,
/// Value>`. Skips Collection and Expr params entirely.
fn generate_resolve_props_body(params: &[WidgetParam]) -> proc_macro2::TokenStream {
    let mut stmts = Vec::new();

    for (param, slot) in params.iter().zip(positional_slots(params)) {
        let name = &param.name;
        let name_str = name.to_string();

        let Some(extraction) = scalar_extraction(param, slot, &quote!(return Err(__e))) else {
            continue;
        };
        stmts.push(extraction);

        // Insertion into __props
        let insertion = match param.ty {
            ParamType::String => quote! {
                __props.insert(#name_str.to_string(), holon_api::Value::String(#name));
            },
            ParamType::Bool => quote! {
                __props.insert(#name_str.to_string(), holon_api::Value::Boolean(#name));
            },
            ParamType::F64 => quote! {
                __props.insert(#name_str.to_string(), holon_api::Value::Float(#name));
            },
            ParamType::F32 => quote! {
                __props.insert(#name_str.to_string(), holon_api::Value::Float(#name as f64));
            },
            ParamType::OptionalString => quote! {
                if let Some(__val) = #name {
                    __props.insert(#name_str.to_string(), holon_api::Value::String(__val));
                }
            },
            ParamType::Value => quote! {
                __props.insert(#name_str.to_string(), #name);
            },
            ParamType::Collection | ParamType::Expr => unreachable!(),
        };
        stmts.push(insertion);
    }

    quote! {
        let mut __props = std::collections::HashMap::new();
        #(#stmts)*
        Ok(__props)
    }
}

/// Generate the `pub fn resolve_props_from_args` function.
fn generate_resolve_props_fn(params: &[WidgetParam]) -> proc_macro2::TokenStream {
    let body = generate_resolve_props_body(params);
    quote! {
        pub fn resolve_props_from_args(
            ba: &BA<'_>,
        ) -> Result<std::collections::HashMap<String, holon_api::Value>, String> {
            #body
        }
    }
}

fn generate_auto_body(widget_name: &str, params: &[WidgetParam]) -> proc_macro2::TokenStream {
    // Auto-body widgets must not have Collection or Expr params.
    for p in params {
        match p.ty {
            ParamType::Collection => {
                return quote! {
                    compile_error!(
                        "auto_body cannot be used with a Collection param — \
                         provide an explicit body that matches on CollectionData"
                    );
                };
            }
            ParamType::Expr => {
                return quote! {
                    compile_error!(
                        "auto_body cannot be used with an Expr param — \
                         provide an explicit body"
                    );
                };
            }
            _ => {}
        }
    }

    // Auto-body: delegate to resolve_props_from_args
    quote! {
        {
            match resolve_props_from_args(&ba) {
                Ok(__props) => ViewModel::from_widget(#widget_name, __props),
                Err(__e) => ViewModel::error(#widget_name, __e),
            }
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
        .zip(positional_slots(params))
        .map(|(p, slot)| {
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
            let slot = match slot {
                Some(idx) => quote! { Some(#idx) },
                None => quote! { None },
            };
            quote! {
                holon_api::StaticParam {
                    name: #name,
                    type_hint: #type_hint,
                    default: #default_expr,
                    slot: #slot,
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
/// 2. `widget_builder! { fn section(title: String, children: Collection) { ...
///    } }` — extraction + user body
/// 3. `widget_builder! { raw fn tree(ba: BA<'_>) -> ViewModel { ... } }` — raw,
///    just WIDGET_META
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
        let extraction = generate_extraction(&widget_name, &input.params);

        let resolve_props_fn = generate_resolve_props_fn(&input.params);

        let expanded = match input.body {
            Some(block) => {
                // Custom-body: extraction + user body in build, resolve_props_fn alongside
                let stmts = &block.stmts;
                quote! {
                    #meta

                    #resolve_props_fn

                    pub fn build(ba: BA<'_>) -> ViewModel {
                        #extraction
                        #(#stmts)*
                    }
                }
            }
            None => {
                // Auto-body: build delegates to resolve_props_from_args (no separate
                // extraction)
                let auto_body = generate_auto_body(&widget_name, &input.params);
                quote! {
                    #meta

                    #resolve_props_fn

                    pub fn build(ba: BA<'_>) -> ViewModel {
                        #auto_body
                    }
                }
            }
        };
        TokenStream::from(expanded)
    }
}
