use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{Ident, LitStr, Token, parse::Parse, parse::ParseStream, punctuated::Punctuated};

enum RegistrationMode {
    /// `register: Type` — generate `register_all(interp: &mut RenderInterpreter<Type, Ext>)`
    Register(syn::Type),
    /// `dispatch: Type` — generate `dispatch_build(name, args, ctx) -> Option<Type>` with a match
    Dispatch(syn::Type),
}

struct BuilderRegistryInput {
    dir: String,
    skip: Vec<String>,
    mode: Option<RegistrationMode>,
    /// Optional extension type for `register` mode (e.g. `ext: BoundsRegistry`)
    ext_type: Option<syn::Type>,
}

impl Parse for BuilderRegistryInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let dir: LitStr = input.parse()?;

        let mut skip = Vec::new();
        let mut mode = None;
        let mut ext_type = None;

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
                "ext" => {
                    input.parse::<Token![:]>()?;
                    ext_type = Some(input.parse::<syn::Type>()?);
                }
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "expected `skip`, `register`, `dispatch`, or `ext`, found `{other}`"
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

            let ext = input.ext_type.map(|e| quote! { , #e }).unwrap_or_default();

            quote! {
                pub fn register_all(interp: &mut RenderInterpreter<#ty #ext>) {
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
        None => quote! {},
    };

    let name_lits: Vec<_> = entries.iter().map(|n| n.as_str()).collect();
    let builder_names_fn = quote! {
        pub fn builder_names() -> &'static [&'static str] {
            &[#(#name_lits),*]
        }
    };

    let expanded = quote! {
        #(#mod_decls)*
        #mode_output
        #builder_names_fn
    };

    TokenStream::from(expanded)
}
