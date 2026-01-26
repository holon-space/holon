//! `declare_transitions!` — the only central code in the pattern.
//!
//! Wraps `declarative_enum_dispatch::enum_dispatch!` (which itself
//! generates the trait, the enum, and the trait-for-enum dispatch impl
//! using only `macro_rules!` — no proc macros) and adds the
//! proptest-state-machine strategy aggregator.
//!
//! Adding a transition = create the file + add one line to the call.

#[macro_export]
macro_rules! declare_transitions {
    (
        $vis:vis enum $enum_name:ident {
            $($variant:ident($ty:ty)),* $(,)?
        }
    ) => {
        // 1. Trait + enum + dispatch impl + From<Variant> impls.
        ::declarative_enum_dispatch::enum_dispatch!(
            pub trait E2ETransition: Clone + std::fmt::Debug {
                /// Re-checked by the shrinker on every shrunk seed.
                fn preconditions(&self, state: &$crate::reference::RefState) -> bool;
                fn apply_to_ref(&self, state: &mut $crate::reference::RefState);
                fn apply_to_sut(&self, sut: &mut $crate::sut::Sut);
            }

            #[derive(Clone, Debug)]
            $vis enum $enum_name {
                $( $variant($ty) ),*
            }
        );

        // 2. Strategy aggregator. Variadic over the variants.
        $vis fn transitions(
            state: &$crate::reference::RefState,
        ) -> ::proptest::strategy::BoxedStrategy<$enum_name> {
            use ::proptest::strategy::{Strategy, Union};
            use $crate::transition::E2ETransitionFactory;

            let mut arms: Vec<(u32, ::proptest::strategy::BoxedStrategy<$enum_name>)>
                = Vec::new();

            $(
                if let Some((w, s)) = <$ty as E2ETransitionFactory>::weighted_generator(state) {
                    arms.push((w, s.prop_map($enum_name::from).boxed()));
                }
            )*

            assert!(
                !arms.is_empty(),
                "declare_transitions!: no transition applicable in state {state:?} \
                 — at least one variant must be unconditionally enabled."
            );
            Union::new_weighted(arms).boxed()
        }
    };
}
