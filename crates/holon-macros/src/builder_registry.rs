use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{Ident, LitStr, Token, parse::Parse, parse::ParseStream, punctuated::Punctuated};

#[allow(dead_code)]
pub(crate) fn snake_to_pascal(s: &str) -> String {
    s.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

/// Parsed info about a builder's `pub fn render(...)` signature.
#[allow(dead_code)]
struct RenderSignature {
    /// Parameter names from ViewKind fields (excludes ctx).
    /// Empty if first param is `node` (pass-through mode).
    field_params: Vec<String>,
    /// True if first param is `node` — pass the full ViewModel instead of destructuring.
    pass_node: bool,
}

/// Read a builder source file and extract the `pub fn render(...)` parameter names.
#[allow(dead_code)]
fn parse_render_signature(path: &std::path::Path) -> RenderSignature {
    let source = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));

    let marker = "pub fn render(";
    let idx = source
        .find(marker)
        .unwrap_or_else(|| panic!("{}: missing `pub fn render(`", path.display()));
    let after = &source[idx + marker.len()..];

    // Find matching closing paren, tracking nesting depth for generics/brackets.
    let mut depth: i32 = 1;
    let mut end = 0;
    for (i, c) in after.char_indices() {
        match c {
            '(' | '<' | '[' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            '>' | ']' => depth -= 1,
            _ => {}
        }
    }
    assert!(
        depth == 0,
        "{}: unmatched parens in render()",
        path.display()
    );
    let params_str = &after[..end];

    // Split by comma respecting nested brackets.
    let mut params = Vec::new();
    let mut current = String::new();
    let mut nest: i32 = 0;
    for c in params_str.chars() {
        match c {
            '(' | '<' | '[' => {
                nest += 1;
                current.push(c);
            }
            ')' | '>' | ']' => {
                nest -= 1;
                current.push(c);
            }
            ',' if nest == 0 => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    params.push(trimmed);
                }
                current = String::new();
            }
            _ => current.push(c),
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        params.push(trimmed);
    }

    // Extract identifier names: "name: Type" → "name", stripping leading _ or &
    let names: Vec<String> = params
        .iter()
        .map(|p| {
            p.split(':')
                .next()
                .unwrap()
                .trim()
                .trim_start_matches('_')
                .to_string()
        })
        .collect();

    // Last param is always ctx — skip it.
    // If only one param (ctx), no ViewKind fields.
    if names.len() <= 1 {
        return RenderSignature {
            field_params: vec![],
            pass_node: false,
        };
    }

    let field_names: Vec<String> = names[..names.len() - 1].to_vec();

    // If first param is "node", pass the full ViewModel.
    if field_names.first().map(|s| s.as_str()) == Some("node") {
        return RenderSignature {
            field_params: vec![],
            pass_node: true,
        };
    }

    RenderSignature {
        field_params: field_names,
        pass_node: false,
    }
}

enum RegistrationMode {
    /// `register: Type` — generate `register_all(interp: &mut RenderInterpreter<Type, Ext>)`
    Register(syn::Type),
    /// `dispatch: Type` — generate `dispatch_build(name, args, ctx) -> Option<Type>` with a match
    Dispatch(syn::Type),
    /// `node_dispatch: Type` — generate `render_node(node, ctx) -> Type` dispatching on ViewKind
    NodeDispatch(syn::Type),
}

struct BuilderRegistryInput {
    dir: String,
    skip: Vec<String>,
    mode: Option<RegistrationMode>,
    #[allow(dead_code)]
    ext_type: Option<syn::Type>,
    /// Context type for `node_dispatch` mode (e.g. `RenderContext`)
    context_type: Option<syn::Type>,
    /// Node type for `node_dispatch` mode. Defaults to
    /// `holon_frontend::reactive_view_model::ReactiveViewModel` so existing
    /// consumers (GPUI) keep working without changes. Snapshot consumers
    /// (e.g. dioxus-web) pass `holon_frontend::view_model::ViewModel` to
    /// dispatch on the static `ViewKind` instead.
    node_type: Option<syn::Path>,
    /// Kind enum type for `node_dispatch` mode (legacy, no longer used).
    #[allow(dead_code)]
    kind_type: Option<syn::Path>,
    /// If true, generate `all_widget_metas()` collecting WIDGET_META from each builder
    widget_metas: bool,
    /// Optional transform template applied to each `node_dispatch` arm result.
    ///
    /// The template is a token-stream expression with two placeholders:
    ///   - `__inner` — substituted with the raw `widget::render(...)` call
    ///   - `__name`  — substituted with a `&'static str` literal of the variant's snake_case name
    ///
    /// Example: `transform: crate::tag(__name, __inner).into_any_element()`
    /// expands per-arm to `crate::tag("column", column::render(gap, children, ctx)).into_any_element()`.
    ///
    /// When omitted, the raw render result is used directly (identity transform).
    transform: Option<proc_macro2::TokenStream>,
}

/// Walk a template `TokenStream` and replace `__inner` / `__name` placeholder
/// idents with the per-arm substitutions. Recurses into `Group`s so placeholders
/// inside nested parens/brackets/braces are spliced too.
fn splice_placeholders(
    template: &proc_macro2::TokenStream,
    inner: &proc_macro2::TokenStream,
    name_lit: &str,
) -> proc_macro2::TokenStream {
    use proc_macro2::{Group, TokenTree};
    template
        .clone()
        .into_iter()
        .flat_map(|tt| -> Box<dyn Iterator<Item = TokenTree>> {
            match tt {
                TokenTree::Ident(id) if id == "__inner" => Box::new(inner.clone().into_iter()),
                TokenTree::Ident(id) if id == "__name" => {
                    let lit = proc_macro2::Literal::string(name_lit);
                    Box::new(std::iter::once(TokenTree::Literal(lit)))
                }
                TokenTree::Group(g) => {
                    let new_stream = splice_placeholders(&g.stream(), inner, name_lit);
                    let new_group = Group::new(g.delimiter(), new_stream);
                    Box::new(std::iter::once(TokenTree::Group(new_group)))
                }
                other => Box::new(std::iter::once(other)),
            }
        })
        .collect()
}

impl Parse for BuilderRegistryInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let dir: LitStr = input.parse()?;

        let mut skip = Vec::new();
        let mut mode = None;
        let mut ext_type = None;
        let mut context_type = None;
        let mut node_type = None;
        let mut kind_type = None;
        let mut widget_metas = false;
        let mut transform = None;

        while input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                break;
            }

            let ident: Ident = input.parse()?;
            match ident.to_string().as_str() {
                "skip" => {
                    input.parse::<Token![:]>()?;
                    let content;
                    syn::bracketed!(content in input);
                    let names: Punctuated<Ident, Token![,]> =
                        content.parse_terminated(Ident::parse, Token![,])?;
                    skip = names.iter().map(|id| id.to_string()).collect();
                }
                "register" => {
                    input.parse::<Token![:]>()?;
                    mode = Some(RegistrationMode::Register(input.parse::<syn::Type>()?));
                }
                "dispatch" => {
                    input.parse::<Token![:]>()?;
                    mode = Some(RegistrationMode::Dispatch(input.parse::<syn::Type>()?));
                }
                "node_dispatch" => {
                    input.parse::<Token![:]>()?;
                    mode = Some(RegistrationMode::NodeDispatch(input.parse::<syn::Type>()?));
                }
                "ext" => {
                    input.parse::<Token![:]>()?;
                    ext_type = Some(input.parse::<syn::Type>()?);
                }
                "context" => {
                    input.parse::<Token![:]>()?;
                    context_type = Some(input.parse::<syn::Type>()?);
                }
                "node_type" => {
                    input.parse::<Token![:]>()?;
                    node_type = Some(input.parse::<syn::Path>()?);
                }
                "kind_type" => {
                    input.parse::<Token![:]>()?;
                    kind_type = Some(input.parse::<syn::Path>()?);
                }
                "widget_metas" => {
                    widget_metas = true;
                }
                "transform" => {
                    input.parse::<Token![:]>()?;
                    // Parse a transform-template expression. We consume tokens
                    // until we hit a top-level comma or end of input.
                    let mut tokens = proc_macro2::TokenStream::new();
                    while !input.is_empty() && !input.peek(Token![,]) {
                        let tt: proc_macro2::TokenTree = input.parse()?;
                        tokens.extend(std::iter::once(tt));
                    }
                    transform = Some(tokens);
                }
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "expected `skip`, `register`, `dispatch`, `node_dispatch`, `ext`, `context`, `node_type`, `kind_type`, `transform`, or `widget_metas`, found `{other}`"
                        ),
                    ));
                }
            }
        }

        Ok(BuilderRegistryInput {
            dir: dir.value(),
            skip,
            mode,
            ext_type,
            context_type,
            node_type,
            kind_type,
            widget_metas,
            transform,
        })
    }
}

pub fn builder_registry_impl(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as BuilderRegistryInput);

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let builders_dir = std::path::Path::new(&manifest_dir).join(&input.dir);

    let mut skip_set: std::collections::HashSet<&str> =
        input.skip.iter().map(|s| s.as_str()).collect();
    skip_set.insert("mod");

    let mut entries: Vec<String> = std::fs::read_dir(&builders_dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", builders_dir.display()))
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if let Some(stem) = name.strip_suffix(".rs") {
                if !skip_set.contains(stem) {
                    return Some(stem.to_string());
                }
            }
            None
        })
        .collect();

    entries.sort();

    let mod_decls: Vec<_> = entries
        .iter()
        .map(|name| {
            let ident = Ident::new(name, Span::call_site());
            quote! { mod #ident; }
        })
        .collect();

    let emit_widget_metas = input.widget_metas;

    let mode_output = match input.mode {
        Some(RegistrationMode::Register(ty)) => {
            let registrations: Vec<_> = entries
                .iter()
                .map(|name| {
                    let ident = Ident::new(name, Span::call_site());
                    let name_lit = name.as_str();
                    quote! { interp.register(#name_lit, #ident::build); }
                })
                .collect();

            quote! {
                pub fn register_all(interp: &mut RenderInterpreter<#ty>) {
                    #(#registrations)*
                }
            }
        }
        Some(RegistrationMode::Dispatch(ret_ty)) => {
            let arms: Vec<_> = entries
                .iter()
                .map(|name| {
                    let ident = Ident::new(name, Span::call_site());
                    let name_lit = name.as_str();
                    quote! { #name_lit => Some(#ident::build(args, ctx)), }
                })
                .collect();

            quote! {
                pub fn dispatch_build(
                    name: &str,
                    args: &ResolvedArgs,
                    ctx: &RenderContext,
                ) -> Option<#ret_ty> {
                    let widget = match name {
                        #(#arms)*
                        _ => None,
                    }?;
                    Some(widget)
                }

                pub fn dispatch_build_annotated(
                    name: &str,
                    args: &ResolvedArgs,
                    ctx: &RenderContext,
                    annotate: &dyn Fn(#ret_ty, &str, &RenderContext) -> #ret_ty,
                ) -> Option<#ret_ty> {
                    let widget = match name {
                        #(#arms)*
                        _ => None,
                    }?;
                    Some(annotate(widget, name, ctx))
                }
            }
        }
        Some(RegistrationMode::NodeDispatch(ret_ty)) => {
            let ctx_ty = input
                .context_type
                .expect("node_dispatch mode requires `context: Type`");
            let transform = &input.transform;
            let node_path: syn::Path = input.node_type.unwrap_or_else(|| {
                syn::parse_str("holon_frontend::reactive_view_model::ReactiveViewModel")
                    .expect("default node path parses")
            });

            // Apply the transform template (or identity, if none).
            let apply =
                |inner: proc_macro2::TokenStream, name_lit: &str| -> proc_macro2::TokenStream {
                    match transform {
                        Some(template) => splice_placeholders(template, &inner, name_lit),
                        None => inner,
                    }
                };

            // All builders are called with (node, ctx) — widget_name() dispatch.
            let arms: Vec<_> = entries
                .iter()
                .map(|name| {
                    let ident = Ident::new(name, Span::call_site());
                    let name_lit = name.as_str();
                    let inner = quote! { #ident::render(node, ctx) };
                    let applied = apply(inner, name_lit);
                    quote! {
                        Some(#name_lit) => #applied,
                    }
                })
                .collect();

            let unsupported_inner = quote! { render_unsupported(name, ctx) };
            let unsupported_applied = apply(unsupported_inner, "__unknown");

            quote! {
                pub fn render_node(
                    node: &#node_path,
                    ctx: &#ctx_ty,
                ) -> #ret_ty {
                    let name = node.widget_name();
                    match name.as_deref() {
                        #(#arms)*
                        Some("empty") | None => gpui::div().into_any_element(),
                        _ => {
                            let name = name.as_deref().unwrap_or("unknown");
                            tracing::warn!("Unsupported widget: {name}");
                            #unsupported_applied
                        }
                    }
                }
            }
        }
        None => quote! {},
    };

    let name_lits: Vec<_> = entries.iter().map(|n| n.as_str()).collect();
    let builder_names_fn = quote! {
        pub fn builder_names() -> &'static [&'static str] {
            &[#(#name_lits),*]
        }
    };

    // Generate all_widget_metas() when `widget_metas` flag is set
    let widget_metas_fn = if emit_widget_metas {
        let meta_refs: Vec<_> = entries
            .iter()
            .map(|name| {
                let ident = Ident::new(name, Span::call_site());
                quote! { &#ident::WIDGET_META }
            })
            .collect();
        quote! {
            pub fn all_widget_metas() -> Vec<&'static holon_api::WidgetMeta> {
                vec![#(#meta_refs),*]
            }
        }
    } else {
        quote! {}
    };

    let expanded = quote! {
        #(#mod_decls)*
        #mode_output
        #builder_names_fn
        #widget_metas_fn
    };

    TokenStream::from(expanded)
}
