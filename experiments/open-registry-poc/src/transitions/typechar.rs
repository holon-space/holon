//! MODULE C — `TypeChar`, needs `dyn SutEditorWrite` (the optional Editor
//! axis). Appends a char to the editor on both ref and SUT. Drives the second
//! §8.7 axis.

use crate::core::SutEditorWrite;

cap_transition! {
    name: TypeChar,
    weight: 2,
    fields: { ch: char },
    caps: { editor: dyn SutEditorWrite },

    // No precondition / block dependency — always applicable when Editor is wired.
    gen: |_state| {
        Some(
            ::proptest::char::range('a', 'e')
                .prop_map(|ch| TypeChar { ch })
                .boxed(),
        )
    },

    precond: |_me, _state| { Ok(()) },

    apply_ref: |me, state| {
        state.editor.push(me.ch);
    },

    apply_sut: |me| {
        editor.type_char(me.ch);
    },
}
