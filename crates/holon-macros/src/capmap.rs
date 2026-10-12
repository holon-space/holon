//! `#[capmap_adapter]` — generates the capability typemap glue for a SUT read
//! trait, so a read cap is a one-line attribute instead of a hand-written
//! forwarding impl (`holon_pbt_core::composition`).
//!
//! Applied to a trait `Foo`, it emits:
//!   1. the trait itself, made object-safe via `#[async_trait(?Send)]`;
//!   2. `impl CapName for dyn Foo` (so `CapMap::{insert,get,expect}::<dyn Foo>`
//!      and `CapId::of::<dyn Foo>()` work);
//!   3. `impl Foo for CapMap`, forwarding every `&self` method to
//!      `self.expect::<dyn Foo>().method(args)[.await]`.
//!
//! A method with a DEFAULT body is forwarded too: this impl is a forwarder, so
//! inheriting a default would answer from the empty adapter instead of the
//! registered provider — a silent oracle fake, since cap defaults are the
//! "substrate without this concept" can't-fail answer.
//!
//! `&mut self` methods (drains / apply-phase side effects) can't be forwarded
//! through the shared `Arc<dyn Cap>`; they get a fail-loud `unimplemented!` so
//! a composed slice that wrongly routes a drain through the map panics clearly
//! rather than silently returning empty.
//!
//! The `#[async_trait(?Send)]` wrapper is emitted **only when the trait has an
//! async method**. A fully-sync cap (e.g. the reference-side `RefBackend`,
//! whose methods return owned values and are already object-safe) gets a plain
//! trait + plain `impl … for CapMap`, so existing sync `impl Trait for
//! ConcreteRef`s don't need the attribute retro-fitted.

use proc_macro2::TokenStream;
use quote::quote;
use syn::FnArg;
use syn::ItemTrait;
use syn::Pat;
use syn::TraitItem;

pub fn capmap_adapter_impl(trait_def: ItemTrait) -> TokenStream {
    let trait_name = &trait_def.ident;
    let mut forwards = Vec::new();

    for item in &trait_def.items {
        let TraitItem::Fn(method) = item else {
            continue;
        };
        let sig = &method.sig;
        let name = &sig.ident;
        let output = &sig.output;
        let asyncness = &sig.asyncness;

        let mut receiver_is_mut = false;
        let mut typed_args = Vec::new();
        let mut arg_idents = Vec::new();
        for input in &sig.inputs {
            match input {
                FnArg::Receiver(r) => receiver_is_mut = r.mutability.is_some(),
                FnArg::Typed(pt) => {
                    let ident = match &*pt.pat {
                        Pat::Ident(pi) => pi.ident.clone(),
                        // A defaulted method ignores an argument as `_: T`; the
                        // forwarder must still pass it on, so name it.
                        Pat::Wild(w) => syn::Ident::new(
                            &format!("arg{}", arg_idents.len()),
                            w.underscore_token.span,
                        ),
                        other => {
                            return syn::Error::new_spanned(
                                other,
                                "capmap_adapter: only identifier and `_` arguments are supported",
                            )
                            .to_compile_error();
                        }
                    };
                    let mut pt = pt.clone();
                    pt.pat = Box::new(Pat::Ident(syn::PatIdent {
                        attrs: Vec::new(),
                        by_ref: None,
                        mutability: None,
                        ident: ident.clone(),
                        subpat: None,
                    }));
                    typed_args.push(pt);
                    arg_idents.push(ident);
                }
            }
        }

        let body = if receiver_is_mut {
            let msg = format!(
                "`{trait_name}::{name}` takes &mut self — a drain/apply-phase op, not available \
                 on the shared CapMap (use the concrete SUT in apply)"
            );
            quote! { unimplemented!(#msg) }
        } else if asyncness.is_some() {
            // Async: keep the owned `Arc` alive across the await point by moving
            // it into the future (the returned future's `'_` lifetime borrows
            // `&self`, so an `expect_ref` borrow would work too, but the owned
            // form is the proven path and needs no lifetime threading).
            quote! { self.expect::<dyn #trait_name>().#name(#(#arg_idents),*).await }
        } else {
            // Sync: borrow the provider rooted in `&self` so a method returning a
            // borrow (`Option<&str>`, `&T`) doesn't dangle off a temporary `Arc`.
            quote! { self.expect_ref::<dyn #trait_name>().#name(#(#arg_idents),*) }
        };

        let receiver = if receiver_is_mut {
            quote!(&mut self)
        } else {
            quote!(&self)
        };

        forwards.push(quote! {
            #asyncness fn #name(#receiver, #(#typed_args),*) #output { #body }
        });
    }

    // Only async traits need `#[async_trait(?Send)]` to become object-safe. A
    // fully-sync cap (e.g. the reference-side `RefBackend`, whose methods return
    // owned values) is already object-safe — wrapping it would force every
    // existing plain `impl Trait for ConcreteRef` to carry the attribute too.
    // Emit the attribute conditionally so one `#[capmap_adapter]` serves both.
    let has_async = trait_def
        .items
        .iter()
        .any(|item| matches!(item, TraitItem::Fn(m) if m.sig.asyncness.is_some()));
    let async_attr = if has_async {
        quote! { #[::async_trait::async_trait(?Send)] }
    } else {
        quote! {}
    };

    quote! {
        #async_attr
        #trait_def

        impl ::holon_pbt_core::composition::CapName for dyn #trait_name {
            const NAME: &'static str = stringify!(#trait_name);
        }

        #async_attr
        impl #trait_name for ::holon_pbt_core::composition::CapMap {
            #(#forwards)*
        }
    }
}
