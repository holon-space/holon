//! `#[derive(StepVocabulary)]` — the one authoring form for a Gherkin step
//! template.
//!
//! The template is the only thing an author writes; field names, quoting, the
//! renderer and the parser are all derived from the annotated struct. Wrong
//! placeholder names and uncovered fields are COMPILE errors at the offending
//! span, never a runtime surprise:
//!
//! ```ignore
//! #[derive(StepVocabulary)]
//! #[step_template("I click block {block_id} in region {region}")]
//! pub struct ClickBlock {
//!     pub block_id: EntityUri,
//!     pub region: Region,
//! }
//! ```
//!
//! A field the template does not name must carry `#[step_default]`
//! (`Default::default()`) or `#[step_default(expr)]` (a constant the step
//! implies) — the step then cannot express any other value for it, which
//! `render_step`'s round-trip self-check reports at record time.
//!
//! Deliberately generic over "annotated struct", not over transitions: the
//! Then-step / invariant vocabulary will derive the same way.

use proc_macro2::TokenStream;
use quote::quote;
use syn::Data;
use syn::DeriveInput;
use syn::Expr;
use syn::Fields;
use syn::LitStr;
use syn::spanned::Spanned;

/// One placeholder or literal run of a template.
enum Segment {
    Literal,
    Field(String),
}

/// Mirror of `holon_pbt_core::step_vocabulary::parse_template`, run at
/// expansion time so the template's shape is checked before any code exists.
fn parse_template(template: &str) -> Result<Vec<Segment>, String> {
    let mut out = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        let close = after
            .find('}')
            .ok_or_else(|| "unterminated `{`".to_string())?;
        let name = &after[..close];
        if name.is_empty() {
            return Err("empty placeholder `{}`".to_string());
        }
        if name.contains('{') {
            return Err("nested `{`".to_string());
        }
        out.push(Segment::Literal);
        out.push(Segment::Field(name.to_string()));
        rest = &after[close + 1..];
    }
    out.push(Segment::Literal);
    Ok(out)
}

struct FieldSpec {
    ident: syn::Ident,
    ty: syn::Type,
    /// `Some(None)` = `#[step_default]`, `Some(Some(expr))` =
    /// `#[step_default(expr)]`, `None` = not defaulted.
    default: Option<Option<Expr>>,
    span: proc_macro2::Span,
}

fn field_specs(input: &DeriveInput) -> syn::Result<Vec<FieldSpec>> {
    let data = match &input.data {
        Data::Struct(s) => s,
        Data::Enum(_) | Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "StepVocabulary describes a struct with named (or no) fields; \
                 an enum has no field set a template could cover",
            ));
        }
    };
    let named = match &data.fields {
        Fields::Named(named) => named.named.iter().collect::<Vec<_>>(),
        Fields::Unit => Vec::new(),
        Fields::Unnamed(unnamed) => {
            return Err(syn::Error::new_spanned(
                unnamed,
                "StepVocabulary needs named fields — a template names its \
                 placeholders, and a tuple struct has none",
            ));
        }
    };

    let mut out = Vec::new();
    for field in named {
        let ident = field.ident.clone().expect("named field");
        let mut default = None;
        for attr in &field.attrs {
            if !attr.path().is_ident("step_default") {
                continue;
            }
            if default.is_some() {
                return Err(syn::Error::new_spanned(
                    attr,
                    "duplicate #[step_default] on one field",
                ));
            }
            default = Some(match &attr.meta {
                syn::Meta::Path(_) => None,
                syn::Meta::List(_) => Some(attr.parse_args::<Expr>()?),
                syn::Meta::NameValue(nv) => Some(nv.value.clone()),
            });
        }
        out.push(FieldSpec {
            span: field.span(),
            ident,
            ty: field.ty.clone(),
            default,
        });
    }
    Ok(out)
}

fn template_literal(input: &DeriveInput) -> syn::Result<LitStr> {
    let mut found: Option<LitStr> = None;
    for attr in &input.attrs {
        if !attr.path().is_ident("step_template") {
            continue;
        }
        let lit: LitStr = attr.parse_args().map_err(|e| {
            syn::Error::new_spanned(
                attr,
                format!("#[step_template(\"…\")] takes one string literal: {e}"),
            )
        })?;
        if found.is_some() {
            return Err(syn::Error::new_spanned(
                attr,
                "duplicate #[step_template]: a struct has exactly one step phrasing",
            ));
        }
        found = Some(lit);
    }
    found.ok_or_else(|| {
        syn::Error::new_spanned(
            &input.ident,
            "missing #[step_template(\"…\")] — every StepVocabulary declares the \
             Gherkin phrasing it renders to and parses from",
        )
    })
}

pub fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    let name = &input.ident;
    let template_lit = template_literal(&input)?;
    let template = template_lit.value();
    let segments = parse_template(&template)
        .map_err(|e| syn::Error::new_spanned(&template_lit, format!("malformed template: {e}")))?;
    let fields = field_specs(&input)?;

    let placeholders: Vec<String> = segments
        .iter()
        .filter_map(|s| match s {
            Segment::Field(n) => Some(n.clone()),
            Segment::Literal => None,
        })
        .collect();

    // (e) — a placeholder that names no field is a compile error AT THE
    // TEMPLATE's span, with the real field list in the message.
    let field_names: Vec<String> = fields.iter().map(|f| f.ident.to_string()).collect();
    for p in &placeholders {
        if !field_names.contains(p) {
            return Err(syn::Error::new_spanned(
                &template_lit,
                format!(
                    "placeholder {{{p}}} names no field of `{name}` (fields: {})",
                    if field_names.is_empty() {
                        "none".to_string()
                    } else {
                        field_names.join(", ")
                    }
                ),
            ));
        }
    }
    for (i, p) in placeholders.iter().enumerate() {
        if placeholders[..i].contains(p) {
            return Err(syn::Error::new_spanned(
                &template_lit,
                format!("placeholder {{{p}}} appears twice; each field renders once"),
            ));
        }
    }

    // (c) — total coverage: every field is templated or explicitly defaulted.
    for f in &fields {
        let templated = placeholders.contains(&f.ident.to_string());
        match (&f.default, templated) {
            (Some(_), true) => {
                return Err(syn::Error::new(
                    f.span,
                    format!(
                        "field `{}` is both named by the template and marked \
                         #[step_default] — the step already carries its value",
                        f.ident
                    ),
                ));
            }
            (None, false) => {
                return Err(syn::Error::new(
                    f.span,
                    format!(
                        "field `{}` is neither named by the step template nor marked \
                         #[step_default] — a step that cannot carry it would silently \
                         drop it on replay",
                        f.ident
                    ),
                ));
            }
            _ => {}
        }
    }

    let field_name_lits: Vec<LitStr> = fields
        .iter()
        .map(|f| LitStr::new(&f.ident.to_string(), f.span))
        .collect();

    // Template fields in TEMPLATE order, each with its type's quoting.
    let template_field_entries = placeholders.iter().map(|p| {
        let f = fields
            .iter()
            .find(|f| f.ident == p.as_str())
            .expect("checked above");
        let ty = &f.ty;
        let lit = LitStr::new(p, f.span);
        quote! { (#lit, <#ty as ::holon_pbt_core::step_vocabulary::StepField>::QUOTED) }
    });

    let render_values = placeholders.iter().map(|p| {
        let f = fields
            .iter()
            .find(|f| f.ident == p.as_str())
            .expect("checked above");
        let ident = &f.ident;
        let ty = &f.ty;
        let lit = LitStr::new(p, f.span);
        quote! {
            (
                #lit,
                <#ty as ::holon_pbt_core::step_vocabulary::StepField>::QUOTED,
                <#ty as ::holon_pbt_core::step_vocabulary::StepField>::render_step_field(&self.#ident),
            )
        }
    });

    let parse_fields = fields.iter().map(|f| {
        let ident = &f.ident;
        let ty = &f.ty;
        let lit = LitStr::new(&ident.to_string(), f.span);
        match &f.default {
            Some(Some(expr)) => quote! { #ident: #expr },
            Some(None) => quote! { #ident: ::core::default::Default::default() },
            None => quote! {
                #ident: <#ty as ::holon_pbt_core::step_vocabulary::StepField>::parse_step_field(
                    ::holon_pbt_core::step_vocabulary::captured(&__caps, #lit),
                )?
            },
        }
    });

    // Examples: cycle each templated field's example list so every example of
    // every field appears at least once; defaulted fields are pinned to their
    // declared constant (the step cannot vary them).
    let example_counts = placeholders.iter().map(|p| {
        let f = fields
            .iter()
            .find(|f| f.ident == p.as_str())
            .expect("checked above");
        let ty = &f.ty;
        quote! {
            <#ty as ::holon_pbt_core::step_vocabulary::StepField>::step_field_examples().len()
        }
    });
    let example_field_inits = fields.iter().map(|f| {
        let ident = &f.ident;
        let ty = &f.ty;
        match &f.default {
            Some(Some(expr)) => quote! { #ident: #expr },
            Some(None) => quote! { #ident: ::core::default::Default::default() },
            None => quote! {
                #ident: {
                    let __e = <#ty as ::holon_pbt_core::step_vocabulary::StepField>::step_field_examples();
                    assert!(
                        !__e.is_empty(),
                        concat!("StepField examples for field `", stringify!(#ident), "` are empty")
                    );
                    __e[__i % __e.len()].clone()
                }
            },
        }
    });

    let ctor = if fields.is_empty() {
        quote! { Self }
    } else {
        let parse_fields: Vec<_> = parse_fields.collect();
        quote! { Self { #(#parse_fields),* } }
    };
    let example_ctor = if fields.is_empty() {
        quote! { Self }
    } else {
        let example_field_inits: Vec<_> = example_field_inits.collect();
        quote! { Self { #(#example_field_inits),* } }
    };

    Ok(quote! {
        impl ::holon_pbt_core::step_vocabulary::StepVocabulary for #name {
            const TEMPLATE: &'static str = #template_lit;

            fn field_names() -> &'static [&'static str] {
                &[ #(#field_name_lits),* ]
            }

            fn template_fields() -> &'static [::holon_pbt_core::step_vocabulary::TemplateField] {
                &[ #(#template_field_entries),* ]
            }

            fn render_step(&self) -> ::holon_pbt_core::step_vocabulary::RenderedStep {
                let __values: ::std::vec::Vec<(&str, bool, ::std::string::String)> =
                    ::std::vec![ #(#render_values),* ];
                ::holon_pbt_core::step_vocabulary::RenderedStep::text(
                    ::holon_pbt_core::step_vocabulary::render_template(Self::TEMPLATE, &__values),
                )
            }

            fn parse_step(
                __text: &str,
                __docstring: ::core::option::Option<&str>,
            ) -> ::core::result::Result<::core::option::Option<Self>, ::std::string::String> {
                let ::core::option::Option::Some(__caps) =
                    ::holon_pbt_core::step_vocabulary::capture_template(
                        Self::TEMPLATE,
                        Self::template_fields(),
                        __text,
                    )
                else {
                    return ::core::result::Result::Ok(::core::option::Option::None);
                };
                // A derived step carries its whole payload in the template. A
                // docstring under it would be author intent this transition
                // cannot honour, so it is refused rather than dropped.
                if __docstring.is_some() {
                    return ::core::result::Result::Err(format!(
                        "step {:?} ({}) takes no docstring, but one was attached — \
                         remove it, or use a step whose transition reads a document",
                        __text,
                        stringify!(#name),
                    ));
                }
                ::core::result::Result::Ok(::core::option::Option::Some(#ctor))
            }

            fn step_examples() -> ::std::vec::Vec<Self> {
                let __counts: ::std::vec::Vec<usize> = ::std::vec![ #(#example_counts),* ];
                let __n = ::holon_pbt_core::step_vocabulary::example_count(&__counts);
                (0..__n).map(|__i| #example_ctor).collect()
            }

            fn step_json(&self) -> ::serde_json::Value {
                ::serde_json::to_value(self).unwrap_or_else(|e| {
                    panic!(
                        concat!(stringify!(#name), " is not serializable as a step value: {}"),
                        e
                    )
                })
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    fn err(input: DeriveInput) -> String {
        expand(input)
            .expect_err("expansion must be refused")
            .to_string()
    }

    #[test]
    fn a_placeholder_naming_no_field_is_refused() {
        let message = err(parse_quote! {
            #[step_template("I click block {blck_id}")]
            struct ClickBlock { block_id: EntityUri }
        });
        assert!(message.contains("{blck_id}"), "{message}");
        assert!(message.contains("block_id"), "{message}");
    }

    #[test]
    fn an_uncovered_field_is_refused() {
        let message = err(parse_quote! {
            #[step_template("I click block {block_id}")]
            struct ClickBlock { block_id: EntityUri, region: Region }
        });
        assert!(message.contains("region"), "{message}");
        assert!(message.contains("step_default"), "{message}");
    }

    #[test]
    fn a_templated_field_may_not_also_be_defaulted() {
        let message = err(parse_quote! {
            #[step_template("I click block {block_id}")]
            struct ClickBlock {
                #[step_default]
                block_id: EntityUri,
            }
        });
        assert!(message.contains("already carries"), "{message}");
    }

    #[test]
    fn a_missing_template_is_refused() {
        let message = err(parse_quote! {
            struct ClickBlock { block_id: EntityUri }
        });
        assert!(message.contains("step_template"), "{message}");
    }

    #[test]
    fn a_malformed_template_is_refused() {
        let message = err(parse_quote! {
            #[step_template("I click block {block_id")]
            struct ClickBlock { block_id: EntityUri }
        });
        assert!(message.contains("unterminated"), "{message}");
    }

    #[test]
    fn a_repeated_placeholder_is_refused() {
        let message = err(parse_quote! {
            #[step_template("I move {id} onto {id}")]
            struct DragDropBlock { id: EntityUri }
        });
        assert!(message.contains("twice"), "{message}");
    }

    #[test]
    fn an_enum_is_refused() {
        let message = err(parse_quote! {
            #[step_template("nothing")]
            enum Whatever { A, B }
        });
        assert!(message.contains("struct"), "{message}");
    }

    #[test]
    fn a_covered_struct_expands() {
        let expanded = expand(parse_quote! {
            #[step_template("I click block {block_id} in region {region}")]
            struct ClickBlock {
                block_id: EntityUri,
                region: Region,
                #[step_default(true)]
                navigates: bool,
            }
        })
        .expect("must expand")
        .to_string();
        assert!(expanded.contains("StepVocabulary"), "{expanded}");
        assert!(expanded.contains("navigates"), "{expanded}");
    }

    #[test]
    fn a_stray_docstring_is_refused_by_the_generated_parser() {
        let expanded = expand(parse_quote! {
            #[step_template("I type {text}")]
            struct TypeChars { text: String }
        })
        .expect("must expand")
        .to_string();
        assert!(expanded.contains("takes no docstring"), "{expanded}");
    }

    #[test]
    fn a_unit_struct_expands() {
        let expanded = expand(parse_quote! {
            #[step_template("I undo")]
            struct UndoLastMutation;
        })
        .expect("must expand")
        .to_string();
        assert!(expanded.contains("field_names"), "{expanded}");
    }
}
