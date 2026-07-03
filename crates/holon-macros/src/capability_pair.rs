//! `capability_pair!` — single-source the Sut*/Ref* capability duality.
//!
//! One declaration emits BOTH read traits (the SUT side and the reference
//! side), hosts each on `CapMap` (by reusing [`crate::capmap::capmap_adapter_impl`]),
//! and — for methods marked `#[compare]` — auto-derives a `BridgedInvariant`
//! constructor that asserts the two sides are equal.
//!
//! ```ignore
//! capability_pair! {
//!     #[pair(sut_name = SutViewModel, ref_name = RefRender)]
//!     /// docs shared by both generated traits
//!     pub trait ViewSelection {
//!         #[compare]                       // → both traits + auto equality invariant
//!         fn current_view(&self) -> String;
//!         #[sut_only]                      // → SUT trait only
//!         fn headless_error_node_count(&self) -> Option<usize>;
//!         #[ref_only]                      // → reference trait only
//!         fn root_render_expr_name(&self) -> Option<String>;
//!     }
//! }
//! ```
//!
//! `#[compare]` accepts four optional keys:
//!
//! - `with = path` — a custom comparator instead of `==`. Contract:
//!   `fn(&SutReturn, &RefReturn) -> Result<(), String>`, `Ok(())` = agree,
//!   `Err(msg)` = fail-loud message used verbatim as the `Fail` payload. Keep
//!   it a pure two-value function so it stays a faithful 1:1 extraction of a
//!   transformed body's compare step (`&Vec<T>` deref-coerces to `&[T]`).
//! - `sut = ident` / `ref = ident` — per-side method-name overrides, for a pair
//!   whose two existing public methods are named differently (e.g. the SUT
//!   `current_focus_rows` vs the reference `navigation_focus_rows`). The method
//!   is written once under either name; the missing side is renamed on emit.
//! - `id = "inv-…"` — an explicit invariant id, to preserve a pre-existing,
//!   externally-referenced id instead of the derived `inv-pair-<stem>-<method>`.
//!
//! Effect convention (audited 46/56 SUT reads): the SUT trait's methods are
//! `async fn … -> T` (SAME owned return type, NO `Result`); the reference
//! trait's methods stay sync and verbatim. Methods are written ONCE, sync, in
//! the declaration; the macro adds `async` for the SUT side.
//!
//! Owned returns only: a borrow-returning signature (`&str`, `Option<&str>`)
//! is a compile error — an `async` SUT method can't borrow `&self` across the
//! `.await`.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{Attribute, Ident, ItemTrait, LitStr, ReturnType, TraitItem, TraitItemFn, Type};

use crate::capmap::capmap_adapter_impl;

/// Options parsed from a `#[compare(..)]` marker.
///
/// - `with`: an optional custom comparator path (see [`build_compare_invariant`]
///   for its contract). Absent → plain `==` equality.
/// - `sut` / `ref_`: optional per-side method-name overrides, for pairs whose
///   two existing public methods are named differently (e.g. the SUT reads
///   `current_focus_rows` while the reference exposes `navigation_focus_rows`).
///   Absent → the method's written name is used on both sides.
/// - `id`: an optional explicit invariant-id string, for preserving a
///   pre-existing (externally-referenced) `inv-*` id instead of the derived
///   `inv-pair-<stem>-<method>` one.
#[derive(Default)]
struct CompareOpts {
    with: Option<syn::Path>,
    sut: Option<Ident>,
    ref_: Option<Ident>,
    id: Option<LitStr>,
}

enum Role {
    Compare(CompareOpts),
    RefOnly,
    SutOnly,
}

pub fn capability_pair_impl(mut decl: ItemTrait) -> TokenStream {
    let stem = decl.ident.clone();
    let mut sut_name: Option<Ident> = None;
    let mut ref_name: Option<Ident> = None;
    let mut trait_attrs: Vec<Attribute> = Vec::new();
    let mut errors: Vec<TokenStream> = Vec::new();

    // 1. Trait-level `#[pair(sut_name = .., ref_name = ..)]` compat attribute:
    //    keep the EXISTING public trait names so no call site renames.
    for attr in std::mem::take(&mut decl.attrs) {
        if attr.path().is_ident("pair") {
            let r = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("sut_name") {
                    sut_name = Some(meta.value()?.parse()?);
                } else if meta.path.is_ident("ref_name") {
                    ref_name = Some(meta.value()?.parse()?);
                } else {
                    return Err(meta.error(
                        "capability_pair: unknown `#[pair(..)]` key (expected `sut_name` / `ref_name`)",
                    ));
                }
                Ok(())
            });
            if let Err(e) = r {
                errors.push(e.to_compile_error());
            }
        } else {
            trait_attrs.push(attr);
        }
    }
    let sut_name = sut_name.unwrap_or_else(|| format_ident!("Sut{}", stem));
    let ref_name = ref_name.unwrap_or_else(|| format_ident!("Ref{}", stem));

    // 2. Partition methods by role.
    let mut sut_methods: Vec<TraitItemFn> = Vec::new();
    let mut ref_methods: Vec<TraitItemFn> = Vec::new();
    let mut compare_invariants: Vec<TokenStream> = Vec::new();

    for item in &decl.items {
        let TraitItem::Fn(f) = item else {
            errors.push(
                syn::Error::new_spanned(
                    item,
                    "capability_pair: only method declarations are supported inside the trait",
                )
                .to_compile_error(),
            );
            continue;
        };

        let (role, kept_attrs) = match extract_role(f) {
            Ok(v) => v,
            Err(e) => {
                errors.push(e.to_compile_error());
                continue;
            }
        };

        if let Err(e) = enforce_owned_return(&f.sig.output) {
            errors.push(e.to_compile_error());
        }

        match role {
            Role::Compare(opts) => {
                let written = f.sig.ident.clone();
                let sut_ident = opts.sut.clone().unwrap_or_else(|| written.clone());
                let ref_ident = opts.ref_.clone().unwrap_or_else(|| written.clone());

                let mut rf = f.clone();
                rf.attrs = kept_attrs.clone();
                rf.sig.ident = ref_ident.clone();
                ref_methods.push(rf);

                let mut sf = f.clone();
                sf.attrs = kept_attrs;
                sf.sig.ident = sut_ident.clone();
                sf.sig.asyncness = Some(syn::token::Async(Span::call_site()));
                sut_methods.push(sf);

                compare_invariants.push(build_compare_invariant(
                    &stem, &sut_ident, &ref_ident, &sut_name, &ref_name, opts.with, opts.id,
                ));
            }
            Role::RefOnly => {
                let mut rf = f.clone();
                rf.attrs = kept_attrs;
                ref_methods.push(rf);
            }
            Role::SutOnly => {
                let mut sf = f.clone();
                sf.attrs = kept_attrs;
                sf.sig.asyncness = Some(syn::token::Async(Span::call_site()));
                sut_methods.push(sf);
            }
        }
    }

    // 3. Emit both traits + their CapMap hosting glue (reusing the audited
    //    `#[capmap_adapter]` token generation), then the compare invariants.
    let sut_trait = make_trait(&decl, &sut_name, &trait_attrs, sut_methods);
    let ref_trait = make_trait(&decl, &ref_name, &trait_attrs, ref_methods);

    let sut_tokens = capmap_adapter_impl(sut_trait);
    let ref_tokens = capmap_adapter_impl(ref_trait);

    quote! {
        #(#errors)*
        #sut_tokens
        #ref_tokens
        #(#compare_invariants)*
    }
}

/// Find the single role marker (`#[compare]` / `#[ref_only]` / `#[sut_only]`)
/// on a method and return it plus the surviving (doc) attributes.
fn extract_role(f: &TraitItemFn) -> syn::Result<(Role, Vec<Attribute>)> {
    let mut role: Option<Role> = None;
    let mut kept: Vec<Attribute> = Vec::new();

    for attr in &f.attrs {
        let path = attr.path();
        if path.is_ident("compare") {
            let mut opts = CompareOpts::default();
            if !matches!(attr.meta, syn::Meta::Path(_)) {
                attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("with") {
                        opts.with = Some(meta.value()?.parse()?);
                    } else if meta.path.is_ident("sut") {
                        opts.sut = Some(meta.value()?.parse()?);
                    } else if meta.path.is_ident("ref") {
                        opts.ref_ = Some(meta.value()?.parse()?);
                    } else if meta.path.is_ident("id") {
                        opts.id = Some(meta.value()?.parse()?);
                    } else {
                        return Err(meta.error(
                            "capability_pair: unknown `#[compare(..)]` key \
                             (expected `with` / `sut` / `ref` / `id`)",
                        ));
                    }
                    Ok(())
                })?;
            }
            set_role(&mut role, Role::Compare(opts), attr)?;
        } else if path.is_ident("ref_only") {
            set_role(&mut role, Role::RefOnly, attr)?;
        } else if path.is_ident("sut_only") {
            set_role(&mut role, Role::SutOnly, attr)?;
        } else {
            kept.push(attr.clone());
        }
    }

    match role {
        Some(r) => Ok((r, kept)),
        None => Err(syn::Error::new_spanned(
            &f.sig,
            "capability_pair: each method needs exactly one of \
             `#[compare]` / `#[ref_only]` / `#[sut_only]`",
        )),
    }
}

fn set_role(slot: &mut Option<Role>, r: Role, attr: &Attribute) -> syn::Result<()> {
    if slot.is_some() {
        return Err(syn::Error::new_spanned(
            attr,
            "capability_pair: a method may carry only one role marker",
        ));
    }
    *slot = Some(r);
    Ok(())
}

fn enforce_owned_return(out: &ReturnType) -> syn::Result<()> {
    if let ReturnType::Type(_, ty) = out
        && returns_borrow(ty)
    {
        return Err(syn::Error::new_spanned(
            ty,
            "capability_pair: methods must return OWNED types — no borrows. The SUT trait is \
             `async`, so a return can't borrow `&self` across the `.await`. Use `String` / \
             `Option<String>` instead of `&str` / `Option<&str>`.",
        ));
    }
    Ok(())
}

/// True if `ty` contains a reference anywhere (top-level or nested in a generic
/// argument / tuple), e.g. `&str`, `Option<&str>`, `(A, &B)`.
fn returns_borrow(ty: &Type) -> bool {
    match ty {
        Type::Reference(_) => true,
        Type::Path(tp) => tp.path.segments.iter().any(|seg| match &seg.arguments {
            syn::PathArguments::AngleBracketed(a) => a
                .args
                .iter()
                .any(|arg| matches!(arg, syn::GenericArgument::Type(t) if returns_borrow(t))),
            _ => false,
        }),
        Type::Tuple(t) => t.elems.iter().any(returns_borrow),
        _ => false,
    }
}

fn make_trait(
    template: &ItemTrait,
    name: &Ident,
    attrs: &[Attribute],
    methods: Vec<TraitItemFn>,
) -> ItemTrait {
    let mut t = template.clone();
    t.ident = name.clone();
    t.attrs = attrs.to_vec();
    t.items = methods.into_iter().map(TraitItem::Fn).collect();
    t
}

/// Emit the auto-derived comparison invariant for one `#[compare]` method:
/// a unit-struct `Invariant` body + a `BridgedInvariant` constructor whose
/// `Needs` requires both the SUT and reference caps to be present.
///
/// `sut_method` / `ref_method` are the (possibly renamed, see `#[compare(sut
/// = .., ref = ..)]`) method names invoked on each side; they are equal when
/// the pair shares one name.
///
/// # Comparator contract (`#[compare(with = path)]`)
///
/// When `with` is `Some(path)`, the check calls `path(&sut_val, &ref_val)`
/// where the comparator has the signature
///
/// ```ignore
/// fn(&SutReturn, &RefReturn) -> Result<(), String>
/// ```
///
/// i.e. it borrows both read values and returns `Ok(())` when they agree or
/// `Err(message)` with a fail-loud, domain-specific divergence message. That
/// message becomes the `InvariantResult::Fail` payload verbatim. This mirrors
/// how the hand-written invariant bodies build rich `Fail` strings, and keeps
/// the comparator a pure two-value function (no auxiliary cap reads) so it is
/// a faithful 1:1 extraction of a transformed body's compare step — not a
/// re-interpretation. `&SutReturn` / `&RefReturn` deref-coerce, so a comparator
/// may take slices (`&[T]`) for `Vec<T>` returns.
///
/// When `with` is `None`, the two values are compared with plain `==` and the
/// macro synthesizes the divergence message.
fn build_compare_invariant(
    stem: &Ident,
    sut_method: &Ident,
    ref_method: &Ident,
    sut_name: &Ident,
    ref_name: &Ident,
    with: Option<syn::Path>,
    id_override: Option<LitStr>,
) -> TokenStream {
    let stem_snake = pascal_to_snake(&stem.to_string());
    let method_snake = sut_method.to_string();
    let id_str = match &id_override {
        Some(lit) => lit.value(),
        None => format!(
            "inv-pair-{}-{}",
            stem_snake.replace('_', "-"),
            method_snake.replace('_', "-")
        ),
    };

    let struct_ident = format_ident!("InvPair{}{}", stem, snake_to_pascal(&method_snake),);
    let ctor_ident = format_ident!("inv_pair_{}_{}", stem_snake, method_snake);
    let how = match &with {
        Some(path) => format!("compared via `{}`", quote!(#path)),
        None => "equal".to_string(),
    };
    let ctor_doc = format!(
        "Auto-derived `#[compare]` invariant `{id_str}`: asserts \
         `{sut_name}::{sut_method}` (SUT) is {how} `{ref_name}::{ref_method}` (reference). \
         Register with one line in `composed_invariant_catalog`."
    );

    let compare_expr = match with {
        Some(path) => quote! {
            match #path(&sut_val, &ref_val) {
                ::core::result::Result::Ok(()) => ::holon_pbt_core::invariant::InvariantResult::Ok,
                ::core::result::Result::Err(msg) => {
                    ::holon_pbt_core::invariant::InvariantResult::Fail(msg)
                }
            }
        },
        None => quote! {
            if sut_val == ref_val {
                ::holon_pbt_core::invariant::InvariantResult::Ok
            } else {
                ::holon_pbt_core::invariant::InvariantResult::Fail(format!(
                    "[{}] {} diverged: SUT={:?} ref={:?}",
                    #id_str,
                    stringify!(#sut_method),
                    sut_val,
                    ref_val,
                ))
            }
        },
    };

    quote! {
        #[doc = #ctor_doc]
        pub struct #struct_ident;

        #[allow(async_fn_in_trait)]
        impl<R, S> ::holon_pbt_core::invariant::Invariant<R, S> for #struct_ident
        where
            R: #ref_name,
            S: #sut_name,
        {
            fn id(&self) -> ::holon_pbt_core::invariant::InvariantId {
                ::holon_pbt_core::invariant::InvariantId(#id_str)
            }

            async fn check(
                &self,
                ref_: &R,
                sut: &S,
            ) -> ::holon_pbt_core::invariant::InvariantResult {
                let sut_val = sut.#sut_method().await;
                let ref_val = ref_.#ref_method();
                #compare_expr
            }
        }

        #[doc = #ctor_doc]
        pub fn #ctor_ident() -> Box<dyn ::holon_pbt_core::composition::CapInvariant> {
            Box::new(::holon_pbt_core::composition::BridgedInvariant::new(
                #struct_ident,
                ::holon_pbt_core::invariant::RunMode::Strict,
                ::holon_pbt_core::composition::Needs {
                    sut_present: vec![
                        ::holon_pbt_core::composition::CapId::of::<dyn #sut_name>(),
                    ],
                    sut_absent: Vec::new(),
                    ref_present: vec![
                        ::holon_pbt_core::composition::CapId::of::<dyn #ref_name>(),
                    ],
                },
            ))
        }
    }
}

fn pascal_to_snake(s: &str) -> String {
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() {
            if i != 0 {
                out.push('_');
            }
            out.extend(ch.to_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn snake_to_pascal(s: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for ch in s.chars() {
        if ch == '_' {
            upper = true;
        } else if upper {
            out.extend(ch.to_uppercase());
            upper = false;
        } else {
            out.push(ch);
        }
    }
    out
}
