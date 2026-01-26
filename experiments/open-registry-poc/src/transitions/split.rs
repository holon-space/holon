//! MODULE A — `SplitBlock`, declared with `cap_transition!`. Note the body of
//! `apply_sut` uses `tree` directly: the macro injected
//! `let tree = sut.expect::<dyn SutBlockTreeWrite>()` from the `caps:` clause,
//! and that same clause produced `required_caps()`. No central edit, no
//! hand-written `expect`, no possible drift between declared and used caps.

use crate::core::SutBlockTreeWrite;

cap_transition! {
    name: SplitBlock,
    weight: 3,
    fields: { target: u64, new_id: u64 },
    caps: { tree: dyn SutBlockTreeWrite },

    // Generator reads the ref to bake a SELF-CONTAINED transition (the only
    // place ref is read for SUT purposes; apply_to_sut never sees it).
    gen: |state| {
        if state.blocks.is_empty() {
            None
        } else {
            let ids = state.blocks.clone();
            let mint = state.next_id;
            Some(
                ::proptest::sample::select(ids)
                    .prop_map(move |target| SplitBlock { target, new_id: mint })
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
        let pos = state.blocks.iter().position(|b| *b == me.target).unwrap();
        state.blocks.insert(pos + 1, me.new_id);
    },

    // `tree: Arc<dyn SutBlockTreeWrite>` is in scope — narrowly typed, no ref.
    apply_sut: |me| {
        tree.split(me.target, me.new_id);
    },
}
