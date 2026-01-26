//! Invariants register through the same open mechanism AND are cap-gated by the
//! same predicate as transitions. Each reads a different optional subsystem's
//! read cap, so it selects iff that subsystem is wired — which is what makes the
//! §8.7 planted-reference bugs reproduce only under the causal subsystem.

use crate::core::{cap, CapMap, Invariant, RefState, SutBlockRead, SutEditorRead, SutToggleRead};

inventory::submit! {
    Invariant {
        name: "inv-blocks-match-ref",
        required_caps: || vec![cap::<dyn SutBlockRead>()],   // always-on substrate
        check: |state: &RefState, sut: &CapMap| {
            let sut_blocks = sut.expect::<dyn SutBlockRead>().blocks();
            if sut_blocks == state.blocks {
                Ok(())
            } else {
                Err(format!("ref={:?} sut={:?}", state.blocks, sut_blocks))
            }
        },
    }
}

inventory::submit! {
    Invariant {
        name: "inv-toggle-match-ref",
        required_caps: || vec![cap::<dyn SutToggleRead>()],  // selects iff Toggle wired
        check: |state: &RefState, sut: &CapMap| {
            let read = sut.expect::<dyn SutToggleRead>();
            for id in &state.blocks {
                let (r, s) = (state.toggled.contains(id), read.is_toggled(*id));
                if r != s {
                    return Err(format!("block {id}: ref_toggled={r} sut_toggled={s}"));
                }
            }
            Ok(())
        },
    }
}

inventory::submit! {
    Invariant {
        name: "inv-editor-match-ref",
        required_caps: || vec![cap::<dyn SutEditorRead>()],  // selects iff Editor wired
        check: |state: &RefState, sut: &CapMap| {
            let sut_text = sut.expect::<dyn SutEditorRead>().text();
            if sut_text == state.editor {
                Ok(())
            } else {
                Err(format!("ref={:?} sut={:?}", state.editor, sut_text))
            }
        },
    }
}
