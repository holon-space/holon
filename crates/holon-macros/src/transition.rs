//! `#[transition]`, `#[place]` and `#[input_type]` (#167, lane M): a plain
//! function declares a user-facing transition. The expansion keeps the
//! function and adds a module of the same name with the HL-IR descriptor and
//! the adapter (bind, body, preview, `NativeParts`).

use proc_macro2::Span;
use proc_macro2::TokenStream;
use quote::ToTokens;
use quote::quote;
use quote::quote_spanned;
use syn::Data;
use syn::DeriveInput;
use syn::Error;
use syn::Fields;
use syn::FnArg;
use syn::GenericArgument;
use syn::Ident;
use syn::ItemFn;
use syn::LitInt;
use syn::Pat;
use syn::Path;
use syn::PathArguments;
use syn::ReturnType;
use syn::Type;
use syn::meta::ParseNestedMeta;
use syn::spanned::Spanned;

/// The Content key under which the lowering puts the answer value.
const INPUT_KEY: &str = "@input";
/// The Content key under which the lowering puts the work occurrence id.
const OCCURRENCE_KEY: &str = "@occurrence";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    Take,
    Read,
    Inhibit,
    Put,
}

impl Kind {
    fn variant(self) -> TokenStream {
        match self {
            Kind::Take => quote!(Take),
            Kind::Read => quote!(Read),
            Kind::Inhibit => quote!(Inhibit),
            Kind::Put => quote!(Put),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Trigger {
    Manual,
    Auto,
}

enum TimerKind {
    Retry { max_attempts: u32 },
    Run,
    Abandon,
    Delay,
}

struct Timer {
    kind: TimerKind,
    after_ms: u64,
}

#[derive(Default)]
struct Attrs {
    trigger: Option<Trigger>,
    irreversible: bool,
    persist: Option<Span>,
    occurrence: bool,
    preview: Option<Path>,
    inhibits: Vec<Type>,
    timers: Vec<Timer>,
}

struct ArcParam {
    ident: Ident,
    ty: Type,
    kind: Kind,
}

enum Arg {
    Arc(ArcParam),
    Input(Ident, Type),
    Occurrence,
}

/// A parsed `#[transition]`: everything the code generation reads.
pub(crate) struct Spec {
    name: Ident,
    trigger: Trigger,
    attrs: Attrs,
    args: Vec<Arg>,
    outputs: Vec<Type>,
}

impl Spec {
    fn arc_params(&self) -> impl Iterator<Item = &ArcParam> {
        self.args.iter().filter_map(|a| match a {
            Arg::Arc(p) => Some(p),
            _ => None,
        })
    }

    fn input(&self) -> Option<&Type> {
        self.args.iter().find_map(|a| match a {
            Arg::Input(_, ty) => Some(ty),
            _ => None,
        })
    }

    /// The arcs in descriptor order: parameters as written, inhibits, outputs.
    pub(crate) fn arcs(&self) -> Vec<(Kind, &Type)> {
        let read = if cfg!(feature = "naive-twins") {
            Kind::Take
        } else {
            Kind::Read
        };
        self.arc_params()
            .map(|p| (if p.kind == Kind::Read { read } else { p.kind }, &p.ty))
            .chain(self.attrs.inhibits.iter().map(|t| (Kind::Inhibit, t)))
            .chain(self.outputs.iter().map(|t| (Kind::Put, t)))
            .collect()
    }
}

fn parse_attrs(attr: TokenStream) -> syn::Result<Attrs> {
    let mut a = Attrs::default();
    let parser = syn::meta::parser(|meta| {
        let set_trigger = |a: &mut Attrs, t: Trigger| {
            if a.trigger.is_some() {
                return Err(meta.error("`manual` and `auto` exclude each other"));
            }
            a.trigger = Some(t);
            Ok(())
        };
        let key = meta.path.get_ident().map(Ident::to_string);
        match key.as_deref() {
            Some("manual") => set_trigger(&mut a, Trigger::Manual),
            Some("auto") => set_trigger(&mut a, Trigger::Auto),
            Some("irreversible") => {
                a.irreversible = true;
                Ok(())
            }
            Some("persist") => {
                a.persist = Some(meta.path.span());
                Ok(())
            }
            Some("occurrence") => {
                a.occurrence = true;
                Ok(())
            }
            Some("preview") => {
                a.preview = Some(meta.value()?.parse()?);
                Ok(())
            }
            Some("inhibit") => {
                a.inhibits.push(meta.value()?.parse()?);
                Ok(())
            }
            Some("retry") => parse_timer(&meta, true).map(|t| a.timers.push(t)),
            Some("run") | Some("abandon") | Some("delay") => {
                parse_timer(&meta, false).map(|t| a.timers.push(t))
            }
            _ => Err(meta.error(
                "unknown #[transition] key; expected manual, auto, irreversible, \
                 preview = <fn>, persist, inhibit = <Place>, occurrence, \
                 retry(after_ms, max_attempts), run(after_ms), abandon(after_ms) or delay(after_ms)",
            )),
        }
    });
    syn::parse::Parser::parse2(parser, attr)?;
    Ok(a)
}

fn parse_timer(meta: &ParseNestedMeta, retry: bool) -> syn::Result<Timer> {
    let mut after_ms = None;
    let mut max_attempts = None;
    meta.parse_nested_meta(|m| {
        if m.path.is_ident("after_ms") {
            after_ms = Some(m.value()?.parse::<LitInt>()?.base10_parse()?);
        } else if retry && m.path.is_ident("max_attempts") {
            max_attempts = Some(m.value()?.parse::<LitInt>()?.base10_parse()?);
        } else {
            return Err(m.error("unknown timer key"));
        }
        Ok(())
    })?;
    let after_ms = after_ms.ok_or_else(|| meta.error("a timer needs after_ms = <n>"))?;
    let kind = match meta.path.get_ident().map(Ident::to_string).as_deref() {
        Some("retry") => TimerKind::Retry {
            max_attempts: max_attempts
                .ok_or_else(|| meta.error("`retry` needs max_attempts = <n>"))?,
        },
        Some("run") => TimerKind::Run,
        Some("abandon") => TimerKind::Abandon,
        _ => TimerKind::Delay,
    };
    Ok(Timer { kind, after_ms })
}

/// Parses the attribute and the function, and strips the parameter
/// attributes from `item`.
pub(crate) fn parse(attr: TokenStream, item: &mut ItemFn) -> syn::Result<Spec> {
    let attrs = parse_attrs(attr)?;
    let name = item.sig.ident.clone();
    let trigger = attrs
        .trigger
        .ok_or_else(|| Error::new(Span::call_site(), "#[transition] needs `manual` or `auto`"))?;
    if trigger == Trigger::Auto {
        if let Some(p) = &attrs.preview {
            return Err(Error::new_spanned(
                p,
                "an auto transition asks nothing: `preview` needs `manual`",
            ));
        }
        if let Some(span) = attrs.persist {
            return Err(Error::new(
                span,
                "an auto transition asks nothing: `persist` needs `manual`",
            ));
        }
    }
    if !item.sig.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &item.sig.generics,
            "a transition has no generic parameters",
        ));
    }
    if let Some(a) = &item.sig.asyncness {
        return Err(Error::new_spanned(a, "a transition is not async"));
    }
    let count = item.sig.inputs.len();
    let mut args = Vec::new();
    for (i, input) in item.sig.inputs.iter_mut().enumerate() {
        let pt = match input {
            FnArg::Receiver(r) => {
                return Err(Error::new_spanned(
                    r,
                    "a transition is a plain function; it takes no `self`",
                ));
            }
            FnArg::Typed(pt) => pt,
        };
        let Pat::Ident(pi) = &*pt.pat else {
            return Err(Error::new_spanned(
                &pt.pat,
                "a transition parameter is a plain name",
            ));
        };
        let ident = pi.ident.clone();
        let mut marks = Vec::new();
        pt.attrs.retain(|a| {
            let kind = ["take", "read", "input"]
                .into_iter()
                .find(|k| a.path().is_ident(k));
            if let Some(k) = kind {
                marks.push((k, a.span()));
            }
            kind.is_none()
        });
        let arg = match marks.as_slice() {
            [] if attrs.occurrence && i + 1 == count => Arg::Occurrence,
            [] => {
                return Err(Error::new_spanned(
                    &pt.pat,
                    format!("parameter `{ident}` of `{name}` needs #[take], #[read] or #[input]"),
                ));
            }
            [("take", _)] => Arg::Arc(ArcParam {
                ident,
                ty: (*pt.ty).clone(),
                kind: Kind::Take,
            }),
            [("read", _)] => Arg::Arc(ArcParam {
                ident,
                ty: (*pt.ty).clone(),
                kind: Kind::Read,
            }),
            [(_, span)] => {
                if trigger == Trigger::Auto {
                    return Err(Error::new(*span, "an auto transition has no user input"));
                }
                if args.iter().any(|a| matches!(a, Arg::Input(..))) {
                    return Err(Error::new(
                        *span,
                        format!(
                            "`{name}` has a second #[input] parameter; a transition has at most one user input"
                        ),
                    ));
                }
                Arg::Input(ident, (*pt.ty).clone())
            }
            [_, (_, span), ..] => {
                return Err(Error::new(
                    *span,
                    format!(
                        "parameter `{ident}` of `{name}` has more than one of #[take], #[read], #[input]"
                    ),
                ));
            }
        };
        args.push(arg);
    }
    if attrs.occurrence && !matches!(args.last(), Some(Arg::Occurrence)) {
        return Err(Error::new_spanned(
            &item.sig,
            format!(
                "`occurrence` passes the occurrence id as the last parameter; `{name}` has no plain last parameter for it"
            ),
        ));
    }
    if trigger == Trigger::Auto && !args.iter().any(|a| matches!(a, Arg::Arc(_))) {
        return Err(Error::new_spanned(
            &item.sig,
            "an auto transition needs a #[take] or #[read] parameter to find its subjects",
        ));
    }
    let outputs = outputs(&item.sig.output, &name)?;
    Ok(Spec {
        name,
        trigger,
        attrs,
        args,
        outputs,
    })
}

/// The output places from `Result<O, Cause>`: `()` none, a tuple several.
fn outputs(ret: &ReturnType, name: &Ident) -> syn::Result<Vec<Type>> {
    let err = || {
        Error::new_spanned(
            ret,
            format!("the return type of `{name}` must be Result<Output, Cause>"),
        )
    };
    let ReturnType::Type(_, ty) = ret else {
        return Err(err());
    };
    let Type::Path(tp) = &**ty else {
        return Err(err());
    };
    let last = tp.path.segments.last().ok_or_else(err)?;
    let PathArguments::AngleBracketed(args) = &last.arguments else {
        return Err(err());
    };
    if last.ident != "Result" || args.args.len() != 2 {
        return Err(err());
    }
    let GenericArgument::Type(ok) = &args.args[0] else {
        return Err(err());
    };
    Ok(match ok {
        Type::Tuple(t) => t.elems.iter().cloned().collect(),
        other => vec![other.clone()],
    })
}

pub(crate) fn expand(attr: TokenStream, mut item: ItemFn) -> syn::Result<TokenStream> {
    let spec = parse(attr, &mut item)?;
    let module = generate(&spec, &item);
    Ok(quote! {
        #item
        #module
    })
}

fn place_of(ty: &Type) -> TokenStream {
    quote_spanned!(ty.span()=> <#ty>::PLACE)
}

fn primitive_input(ty: &Type) -> Option<TokenStream> {
    let Type::Path(tp) = ty else { return None };
    let ident = tp.path.get_ident()?.to_string();
    Some(match ident.as_str() {
        "String" => quote!(::holon_api::TypeHint::String),
        "bool" => quote!(::holon_api::TypeHint::Bool),
        "i64" => quote!(::holon_api::TypeHint::Number),
        _ => return None,
    })
}

fn generate(spec: &Spec, item: &ItemFn) -> TokenStream {
    let name = &spec.name;
    let name_str = name.to_string();
    let vis = &item.vis;
    let arcs = spec.arcs().into_iter().map(|(kind, ty)| {
        let kind = kind.variant();
        let place = place_of(ty);
        quote! {
            ::holon_transition_contract::HlArc {
                kind: ::holon_transition_contract::ArcKind::#kind,
                place: ::holon_transition_contract::PlaceName(#place.to_string()),
            }
        }
    });
    let timers = spec.attrs.timers.iter().map(|t| {
        let after = t.after_ms;
        let kind = match t.kind {
            TimerKind::Retry { max_attempts } => quote!(Retry { max_attempts: #max_attempts }),
            TimerKind::Run => quote!(Run),
            TimerKind::Abandon => quote!(Abandon),
            TimerKind::Delay => quote!(Delay),
        };
        quote! {
            ::holon_transition_contract::TimerDecl {
                kind: ::holon_transition_contract::TimerKind::#kind,
                after: ::std::time::Duration::from_millis(#after),
            }
        }
    });
    let irreversible = spec.attrs.irreversible;
    let preview_symbol = spec.attrs.preview.as_ref().map(|p| {
        let text = p.to_token_stream().to_string().replace(' ', "");
        quote!(::holon_transition_contract::NativeRef(
            format!("{}::{}", parent, #text)
        ))
    });
    let ask = match spec.trigger {
        Trigger::Auto => quote!(None),
        Trigger::Manual => {
            let input = match spec.input() {
                None => quote!(None),
                Some(ty) => {
                    let hint = primitive_input(ty)
                        .unwrap_or_else(|| quote_spanned!(ty.span()=> <#ty>::__input_hint()));
                    quote!(Some(#hint))
                }
            };
            let preview = match &preview_symbol {
                None => quote!(None),
                Some(s) => quote!(Some(#s)),
            };
            let persist = spec.attrs.persist.is_some();
            quote! {
                Some(::holon_transition_contract::AskDecl {
                    input: #input,
                    preview: #preview,
                    persist: #persist,
                })
            }
        }
    };

    let fail = |what: TokenStream| {
        quote! {
            |e| ::holon_net::exec::Cause::definite(format!(
                "transition `{}`: {}: {}", #name_str, #what, e
            ))
        }
    };
    let decode_arc = |p: &ArcParam| {
        let ident = &p.ident;
        let key = ident.to_string();
        let ty = &p.ty;
        let f = fail(quote!(concat!("parameter `", #key, "`")));
        quote_spanned! {ty.span()=>
            let #ident = <#ty>::__place_from_value(arg(#key)?).map_err(#f)?;
        }
    };
    let arc_decodes: Vec<_> = spec.arc_params().map(decode_arc).collect();
    let decodes = spec.args.iter().map(|a| match a {
        Arg::Arc(p) => decode_arc(p),
        Arg::Input(ident, ty) => {
            let f = fail(quote!(concat!("input `", stringify!(#ident), "`")));
            quote_spanned! {ty.span()=>
                let #ident = <#ty as ::std::convert::TryFrom<::holon_api::Value>>::try_from(
                    arg(#INPUT_KEY)?.clone(),
                ).map_err(#f)?;
            }
        }
        Arg::Occurrence => quote! {
            let occurrence = match arg(#OCCURRENCE_KEY)? {
                ::holon_api::Value::String(id) => {
                    ::holon_transition_contract::OccurrenceId::caller(id)
                }
                other => {
                    return Err(::holon_net::exec::Cause::definite(format!(
                        "transition `{}`: the occurrence id is not text: {:?}", #name_str, other
                    )));
                }
            };
        },
    });
    let call_args = spec.args.iter().map(|a| match a {
        Arg::Arc(p) => p.ident.clone(),
        Arg::Input(ident, _) => ident.clone(),
        Arg::Occurrence => Ident::new("occurrence", Span::call_site()),
    });
    let out_idents: Vec<_> = (0..spec.outputs.len())
        .map(|i| Ident::new(&format!("out{i}"), Span::call_site()))
        .collect();
    let out_pattern = match out_idents.as_slice() {
        [single] => quote!(#single),
        many => quote!((#(#many),*)),
    };
    let out_rows = spec.outputs.iter().zip(&out_idents).map(|(ty, o)| {
        let place = place_of(ty);
        quote!(::holon_net::exec::Output::new(#place, s, #o.__place_into_content()))
    });
    let binds = spec.arc_params().map(|p| {
        let key = p.ident.to_string();
        let ty = &p.ty;
        quote_spanned!(ty.span()=> c.insert(#key.to_string(), <#ty>::__place_read(r, s));)
    });
    let arg_fn = quote! {
        let arg = |key: &str| {
            c.get(key).ok_or_else(|| ::holon_net::exec::Cause::definite(format!(
                "transition `{}`: no `{}` in the bound content", #name_str, key
            )))
        };
    };

    let (preview_fn, preview_part) = match &spec.attrs.preview {
        None => (quote!(), quote!(None)),
        Some(path) => {
            let refs = spec.arc_params().map(|p| {
                let ident = &p.ident;
                quote!(&#ident)
            });
            (
                quote! {
                    /// The preview over the bound situation only (ruling 8). A
                    /// token that does not decode shows as a failed preview.
                    pub fn preview(
                        s: &::holon_net::exec::Subject,
                        c: &::holon_net::exec::Content,
                    ) -> String {
                        let _ = s;
                        #arg_fn
                        let bound = || -> Result<String, ::holon_net::exec::Cause> {
                            #(#arc_decodes)*
                            Ok(#path(#(#refs),*))
                        };
                        match bound() {
                            Ok(text) => text,
                            Err(cause) => format!("preview failed: {}", cause.text),
                        }
                    }
                },
                quote!(Some(::std::sync::Arc::new(preview))),
            )
        }
    };

    quote! {
        /// The HL-IR descriptor and the adapter generated by `#[transition]`.
        #[allow(unused_imports, clippy::redundant_closure)]
        #vis mod #name {
            use super::*;

            pub fn descriptor() -> ::holon_transition_contract::TransitionDescriptor {
                let parent = module_path!()
                    .rsplit_once("::")
                    .expect("the generated module has a parent")
                    .0;
                ::holon_transition_contract::TransitionDescriptor {
                    name: ::holon_transition_contract::TransitionName(#name_str.to_string()),
                    arcs: vec![#(#arcs),*],
                    body: ::holon_transition_contract::BodyDecl::Native {
                        symbol: ::holon_transition_contract::NativeRef(
                            format!("{}::{}", parent, #name_str),
                        ),
                        effectful: false,
                    },
                    irreversible: #irreversible,
                    timers: vec![#(#timers),*],
                    guard: None,
                    ask: #ask,
                    undoes: None,
                }
            }

            /// Reads every field of each #[take] and #[read] token of `s`.
            pub fn bind(
                r: &mut ::holon_net::exec::Reader<'_>,
                s: &::holon_net::exec::Subject,
            ) -> Option<::holon_net::exec::Content> {
                let mut c = ::holon_net::exec::Content::new();
                #(#binds)*
                Some(c)
            }

            /// Decodes the bound content, calls the function and puts each
            /// output for `s`; the function's `Err` is the failure as is.
            pub fn body(
                s: &::holon_net::exec::Subject,
                c: &::holon_net::exec::Content,
            ) -> Result<Vec<::holon_net::exec::Output>, ::holon_net::exec::Cause> {
                #arg_fn
                #(#decodes)*
                let #out_pattern = super::#name(#(#call_args),*)?;
                Ok(vec![#(#out_rows),*])
            }

            #preview_fn

            pub fn native_parts() -> ::holon_net::uft::NativeParts {
                ::holon_net::uft::NativeParts {
                    bind: ::std::sync::Arc::new(bind),
                    body: ::holon_net::uft::NativeBody::Pure(::std::sync::Arc::new(body)),
                    preview: #preview_part,
                }
            }
        }
    }
}

/// `#[place]`: the type is the token of the place of the same name.
pub(crate) fn expand_place(input: DeriveInput) -> syn::Result<TokenStream> {
    if !input.generics.params.is_empty() {
        return Err(Error::new_spanned(
            &input.generics,
            "a place token type has no generic parameters",
        ));
    }
    let Data::Struct(data) = &input.data else {
        return Err(Error::new_spanned(
            &input.ident,
            "#[place] goes on a struct with named fields or a unit struct",
        ));
    };
    let fields: Vec<(&Ident, &Type)> = match &data.fields {
        Fields::Named(named) => named
            .named
            .iter()
            .map(|f| (f.ident.as_ref().expect("a named field"), &f.ty))
            .collect(),
        Fields::Unit => Vec::new(),
        Fields::Unnamed(f) => {
            return Err(Error::new_spanned(
                f,
                "#[place] goes on a struct with named fields or a unit struct",
            ));
        }
    };
    let ident = &input.ident;
    let place = ident.to_string();
    let keys: Vec<String> = fields.iter().map(|(f, _)| f.to_string()).collect();
    let decodes = fields.iter().zip(&keys).map(|((f, ty), key)| {
        quote! {
            #f: <#ty as ::std::convert::TryFrom<::holon_api::Value>>::try_from(
                m.get(#key).cloned().unwrap_or(::holon_api::Value::Null),
            ).map_err(|e| format!("field `{}` of {}: {}", #key, Self::PLACE, e))?,
        }
    });
    let construct = match &data.fields {
        Fields::Unit => quote!(Self),
        _ => quote!(Self { #(#decodes)* }),
    };
    let field_idents = fields.iter().map(|(f, _)| f);
    Ok(quote! {
        #input

        impl #ident {
            /// The place this token type lives in.
            pub const PLACE: &'static str = #place;

            #[doc(hidden)]
            pub fn __place_read(
                r: &mut ::holon_net::exec::Reader<'_>,
                s: &::holon_net::exec::Subject,
            ) -> ::holon_api::Value {
                let mut m = ::std::collections::HashMap::new();
                let keys: &[&str] = &[#(#keys),*];
                for &key in keys {
                    if let Some(v) = r.field(Self::PLACE, s, key) {
                        m.insert(key.to_string(), v);
                    }
                }
                ::holon_api::Value::Object(m)
            }

            #[doc(hidden)]
            pub fn __place_from_value(v: &::holon_api::Value) -> Result<Self, String> {
                let ::holon_api::Value::Object(m) = v else {
                    return Err(format!("the token of {} is not an object: {:?}", Self::PLACE, v));
                };
                let _ = m;
                Ok(#construct)
            }

            #[doc(hidden)]
            pub fn __place_into_content(self) -> ::holon_net::exec::Content {
                let mut c = ::holon_net::exec::Content::new();
                #(c.insert(#keys.to_string(), ::holon_api::Value::from(self.#field_idents));)*
                c
            }
        }
    })
}

/// `#[input_type]`: an enum of unit variants is a labelled choice.
pub(crate) fn expand_input_type(input: DeriveInput) -> syn::Result<TokenStream> {
    let Data::Enum(data) = &input.data else {
        return Err(Error::new_spanned(
            &input.ident,
            "#[input_type] goes on an enum of unit variants",
        ));
    };
    if let Some(v) = data
        .variants
        .iter()
        .find(|v| !matches!(v.fields, Fields::Unit))
    {
        return Err(Error::new_spanned(
            v,
            "#[input_type] goes on an enum of unit variants",
        ));
    }
    let ident = &input.ident;
    let type_name = ident.to_string();
    let variants: Vec<&Ident> = data.variants.iter().map(|v| &v.ident).collect();
    let labels: Vec<String> = variants.iter().map(|v| v.to_string()).collect();
    let expected = labels.join(", ");
    Ok(quote! {
        #input

        impl #ident {
            #[doc(hidden)]
            pub fn __input_hint() -> ::holon_api::TypeHint {
                ::holon_api::TypeHint::Enum {
                    type_name: #type_name.to_string(),
                    variants: vec![#(#labels.to_string()),*],
                }
            }
        }

        impl ::std::convert::TryFrom<::holon_api::Value> for #ident {
            type Error = String;

            fn try_from(v: ::holon_api::Value) -> Result<Self, String> {
                match &v {
                    ::holon_api::Value::String(label) => match label.as_str() {
                        #(#labels => Ok(Self::#variants),)*
                        other => Err(format!(
                            "`{}` is not a {}; expected one of {}", other, #type_name, #expected
                        )),
                    },
                    other => Err(format!("a {} is a label, not {:?}", #type_name, other)),
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use quote::format_ident;
    use syn::parse_quote;

    use super::*;

    fn error_of(attr: TokenStream, item: ItemFn) -> String {
        match expand(attr, item) {
            Ok(_) => panic!("expected a compile error"),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn wrong_signatures_give_their_named_errors() {
        let cases: Vec<(TokenStream, ItemFn, &str)> = vec![
            (
                quote!(manual),
                parse_quote!(
                    fn f(a: A) -> Result<(), Cause> {
                        Ok(())
                    }
                ),
                "parameter `a` of `f` needs #[take], #[read] or #[input]",
            ),
            (
                quote!(auto),
                parse_quote!(
                    fn f(#[read] a: A, #[input] p: Pick) -> Result<(), Cause> {
                        Ok(())
                    }
                ),
                "an auto transition has no user input",
            ),
            (
                quote!(manual),
                parse_quote!(
                    fn f(#[input] p: Pick, #[input] q: Pick) -> Result<(), Cause> {
                        Ok(())
                    }
                ),
                "`f` has a second #[input] parameter; a transition has at most one user input",
            ),
            (
                quote!(manual, guard = p),
                parse_quote!(
                    fn f(#[take] a: A) -> Result<(), Cause> {
                        Ok(())
                    }
                ),
                "unknown #[transition] key",
            ),
            (
                quote!(irreversible),
                parse_quote!(
                    fn f(#[take] a: A) -> Result<(), Cause> {
                        Ok(())
                    }
                ),
                "#[transition] needs `manual` or `auto`",
            ),
            (
                quote!(manual, auto),
                parse_quote!(
                    fn f(#[take] a: A) -> Result<(), Cause> {
                        Ok(())
                    }
                ),
                "`manual` and `auto` exclude each other",
            ),
            (
                quote!(auto, preview = p),
                parse_quote!(
                    fn f(#[take] a: A) -> Result<(), Cause> {
                        Ok(())
                    }
                ),
                "an auto transition asks nothing: `preview` needs `manual`",
            ),
            (
                quote!(auto),
                parse_quote!(
                    fn f() -> Result<(), Cause> {
                        Ok(())
                    }
                ),
                "an auto transition needs a #[take] or #[read] parameter",
            ),
            (
                quote!(manual, occurrence),
                parse_quote!(
                    fn f(#[take] a: A) -> Result<(), Cause> {
                        Ok(())
                    }
                ),
                "`occurrence` passes the occurrence id as the last parameter",
            ),
            (
                quote!(manual),
                parse_quote!(
                    fn f(#[take] a: A) -> B {
                        b
                    }
                ),
                "the return type of `f` must be Result<Output, Cause>",
            ),
            (
                quote!(manual),
                parse_quote!(
                    fn f(
                        #[take]
                        #[read]
                        a: A,
                    ) -> Result<(), Cause> {
                        Ok(())
                    }
                ),
                "parameter `a` of `f` has more than one of #[take], #[read], #[input]",
            ),
            (
                quote!(manual, retry(after_ms = 5)),
                parse_quote!(
                    fn f(#[take] a: A) -> Result<(), Cause> {
                        Ok(())
                    }
                ),
                "`retry` needs max_attempts = <n>",
            ),
        ];
        for (attr, item, expected) in cases {
            let error = error_of(attr, item);
            assert!(
                error.contains(expected),
                "expected `{expected}`, got `{error}`"
            );
        }
    }

    #[test]
    fn the_function_keeps_its_signature_without_parameter_attributes() {
        let mut item: ItemFn = parse_quote!(
            pub fn f(
                #[take] a: A,
                #[read]
                #[allow(unused)]
                b: B,
            ) -> Result<C, Cause> {
                todo!()
            }
        );
        parse(quote!(manual), &mut item).expect("a valid transition");
        let expected: ItemFn = parse_quote!(
            pub fn f(a: A, #[allow(unused)] b: B) -> Result<C, Cause> {
                todo!()
            }
        );
        let text = |f: &ItemFn| -> Vec<String> {
            f.sig
                .inputs
                .iter()
                .map(|a| a.to_token_stream().to_string())
                .chain([f.sig.output.to_token_stream().to_string()])
                .chain([f.block.to_token_stream().to_string()])
                .collect()
        };
        assert_eq!(text(&item), text(&expected));
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum ParamKind {
        Take,
        Read,
        Input,
    }

    /// A random transition: parameter kinds, inhibits and outputs, each over
    /// its own place type `P<i>`.
    fn arc_set() -> impl Strategy<Value = (Vec<ParamKind>, usize, usize, bool)> {
        (
            proptest::collection::vec(
                prop_oneof![Just(ParamKind::Take), Just(ParamKind::Read)],
                1..5,
            ),
            0usize..3,
            0usize..3,
            any::<bool>(),
        )
    }

    proptest! {
        #[test]
        fn parsed_arcs_equal_the_model((params, inhibits, outputs, with_input) in arc_set(), at in 0usize..5) {
            let mut kinds = params.clone();
            if with_input {
                kinds.insert(at.min(kinds.len()), ParamKind::Input);
            }
            let mut model: Vec<(Kind, String)> = Vec::new();
            let mut fn_params = Vec::new();
            for (n, k) in kinds.iter().enumerate() {
                let p = format_ident!("p{n}");
                let ty = format_ident!("P{n}");
                fn_params.push(match k {
                    ParamKind::Take => {
                        model.push((Kind::Take, ty.to_string()));
                        quote!(#[take] #p: #ty)
                    }
                    ParamKind::Read => {
                        model.push((Kind::Read, ty.to_string()));
                        quote!(#[read] #p: #ty)
                    }
                    ParamKind::Input => quote!(#[input] #p: Pick),
                });
            }
            let inhibit_tys: Vec<_> = (0..inhibits).map(|i| format_ident!("I{i}")).collect();
            model.extend(inhibit_tys.iter().map(|t| (Kind::Inhibit, t.to_string())));
            let out_tys: Vec<_> = (0..outputs).map(|i| format_ident!("O{i}")).collect();
            model.extend(out_tys.iter().map(|t| (Kind::Put, t.to_string())));
            let ret = match out_tys.as_slice() {
                [single] => quote!(#single),
                many => quote!((#(#many),*)),
            };
            let mut item: ItemFn = parse_quote!(fn f(#(#fn_params),*) -> Result<#ret, Cause> { todo!() });
            let spec = parse(quote!(manual #(, inhibit = #inhibit_tys)*), &mut item).expect("a valid transition");
            let arcs: Vec<(Kind, String)> = spec
                .arcs()
                .into_iter()
                .map(|(k, t)| (k, t.to_token_stream().to_string()))
                .collect();
            prop_assert_eq!(arcs, model);
            prop_assert_eq!(spec.input().map(|t| t.to_token_stream().to_string()), with_input.then(|| "Pick".to_string()));
        }
    }
}
