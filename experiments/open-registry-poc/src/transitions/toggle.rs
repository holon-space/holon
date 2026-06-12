//! MODULE B — `ToggleState`, needs `dyn SutToggleWrite` (the optional Toggle
//! axis), so under a config without Toggle it auto-drops from the alphabet.

use crate::core::SutToggleWrite;

cap_transition! {
    name: ToggleState,
    weight: 2,
    fields: { target: u64 },
    caps: { toggle: dyn SutToggleWrite },

    gen: |state| {
        if state.blocks.is_empty() {
            None
        } else {
            let ids = state.blocks.clone();
            Some(
                ::proptest::sample::select(ids)
                    .prop_map(|target| ToggleState { target })
                    .boxed(),
            )
        }
    },

    precond: |me, state| {
        if state.blocks.contains(&me.target) {
            Ok(())
        } else {
            Err(format!("target {} absent", me.target))
        }
    },

    apply_ref: |me, state| {
        if !state.toggled.insert(me.target) {
            state.toggled.remove(&me.target);
        }
    },

    apply_sut: |me| {
        toggle.toggle(me.target);
    },
}
