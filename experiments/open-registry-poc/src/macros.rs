//! `cap_transition!` — declare a transition; the macro derives everything the
//! registry needs and (the point of question 2) **injects the cap extraction**
//! so bodies are narrowly typed with no hand-written `sut.expect::<…>()`.
//!
//! It is the transition analog of γ's `cap_invariant!` (Design §4.3): the one
//! `caps: { tree: dyn SutBlockTreeWrite }` clause single-sources BOTH
//! `required_caps()` AND the `let tree = sut.expect::<dyn SutBlockTreeWrite>()`
//! binding in `apply_to_sut`, so declared needs and actual reads cannot drift.
//!
//! The uniform `apply_to_sut(&self, sut: &CapMap)` signature is unavoidable —
//! `Box<dyn Transition>` needs ONE signature across heterogeneous transitions —
//! but the *body* only ever sees the narrow cap handles the macro bound.

#[macro_export]
macro_rules! cap_transition {
    (
        name: $name:ident,
        weight: $weight:expr,
        fields: { $($fname:ident : $fty:ty),* $(,)? },
        caps: { $($cap_bind:ident : dyn $cap:path),* $(,)? },
        gen: |$gstate:ident| $genbody:block,
        precond: |$pself:ident, $pstate:ident| $precondbody:block,
        apply_ref: |$rself:ident, $rstate:ident| $applyrefbody:block,
        apply_sut: |$sself:ident| $applysutbody:block $(,)?
    ) => {
        #[derive(Clone, Debug, ::serde::Serialize, ::serde::Deserialize)]
        pub struct $name {
            $(pub $fname : $fty),*
        }

        impl $name {
            /// Single source of truth for this transition's caps: both the
            /// `TransitionGen` registration and `required_caps()` call this.
            pub fn caps() -> ::std::vec::Vec<$crate::core::CapRef> {
                ::std::vec![ $( $crate::core::cap::<dyn $cap>() ),* ]
            }

            fn __gen(
                $gstate: &$crate::core::RefState,
            ) -> ::std::option::Option<(
                u32,
                ::proptest::strategy::BoxedStrategy<::std::boxed::Box<dyn $crate::core::Transition>>,
            )> {
                use ::proptest::strategy::Strategy;
                let inner: ::std::option::Option<::proptest::strategy::BoxedStrategy<$name>> =
                    $genbody;
                inner.map(|__s| {
                    (
                        $weight,
                        __s.prop_map(|__v| {
                            ::std::boxed::Box::new(__v) as ::std::boxed::Box<dyn $crate::core::Transition>
                        })
                        .boxed(),
                    )
                })
            }
        }

        #[typetag::serde]
        #[allow(unused_braces)] // macro controls the body braces, not the author
        impl $crate::core::Transition for $name {
            fn variant_name(&self) -> &'static str {
                stringify!($name)
            }

            fn required_caps(&self) -> ::std::vec::Vec<$crate::core::CapRef> {
                Self::caps()
            }

            fn preconditions(
                &self,
                __state: &$crate::core::RefState,
            ) -> ::std::result::Result<(), ::std::string::String> {
                let $pself = self;
                let $pstate = __state;
                $precondbody
            }

            fn apply_to_ref(&self, __state: &mut $crate::core::RefState) {
                let $rself = self;
                let $rstate = __state;
                $applyrefbody
            }

            fn apply_to_sut(&self, sut: &$crate::core::CapMap) {
                let $sself = self;
                // ── cap extraction, injected from `caps: { … }` ──
                $( let $cap_bind = sut.expect::<dyn $cap>(); )*
                $applysutbody
            }
        }

        ::inventory::submit! {
            $crate::core::TransitionGen {
                name: stringify!($name),
                required_caps: $name::caps,
                gen: $name::__gen,
            }
        }
    };
}
