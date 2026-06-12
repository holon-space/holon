//! Fixed-id seeding primitives shared by the `subsystem_seed` spike (test-only)
//! and the **windowed** slice (`crate::pbt::window_slice`, compiled under the
//! `pbt` feature). Lifted out of `subsystem_seed` so the windowed E4 increment can
//! seed a ref tree with the SAME fixed ids the spike uses — the ref↔SUT identity
//! mapping keystone — without un-gating the whole `#[cfg(test)]` spike body.
//!
//! Gated `any(test, feature = "pbt")`: present in the spike's own `cargo test`
//! build AND in the `pbt`-feature build the gpui windowed test depends on.

use holon_api::{Block, EntityUri};
use holon_orgmode::models::OrgBlockExt;

use crate::pbt::reference_state::ReferenceState;

/// Seed block contents. The reference expects these verbatim — except where a
/// plant deliberately diverges it.
pub(crate) const PARENT: &str = "parent";
pub(crate) const C1: &str = "c1";
pub(crate) const C2: &str = "c2";

/// The block ids of the seeded tree (`parent` with children `c1`,`c2`).
#[derive(Clone, Debug)]
pub(crate) struct Ids {
    pub(crate) parent: EntityUri,
    pub(crate) c1: EntityUri,
    pub(crate) c2: EntityUri,
}

/// **Fixed, shared** ids for the seeded tree. With `ReferenceState` as the
/// proptest state, the ref tree's ids are fixed at `init_state` time — before
/// the SUT store exists — so both sides seed the SAME ids and the ref↔SUT id
/// mapping is the identity (`MemoryBackend`/`LoroBackend::create_block` both
/// honor a provided id). No `with_resolved_doc_uris` remap needed.
pub(crate) fn fixed_ids() -> Ids {
    Ids {
        parent: EntityUri::block("parent"),
        c1: EntityUri::block("c1"),
        c2: EntityUri::block("c2"),
    }
}

/// The planted divergence applied to a *mirror* (cloned) ref state at the
/// observation boundary — the live proptest state stays correct.
// The non-`Content` variants are only constructed by the (test-only) spike; the
// windowed `pbt` build uses `Content`. Allow the rest rather than cfg-split.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Plant {
    /// No bug — every selected invariant holds. The committed default.
    None,
    /// Reference children in reversed order: Loro-causal (`{Loro}`).
    LoroOrder,
    /// Reference editor text diverges: editor-causal (`{EditorState}`).
    Editor,
    /// Reference block content diverges: optional-independent (`{}`).
    Content,
}

/// Seed `parent`/`c1`/`c2` into a [`ReferenceState`]'s working tree with the
/// **fixed shared ids** and explicit sibling order (`c1` before `c2`) and **no**
/// `block_documents` entry — so they are non-seed and the block invariants
/// compare them against the identically-seeded SUT store.
pub(crate) fn seed_ref_tree(state: &mut ReferenceState) {
    let ids = fixed_ids();
    let mut parent = Block::new_text(ids.parent.clone(), EntityUri::no_parent(), PARENT);
    parent.set_sequence(0);
    let mut c1 = Block::new_text(ids.c1.clone(), ids.parent.clone(), C1);
    c1.set_sequence(0);
    let mut c2 = Block::new_text(ids.c2.clone(), ids.parent.clone(), C2);
    c2.set_sequence(1);
    for b in [parent, c1, c2] {
        state.domain.block_state.blocks.insert(b.id.clone(), b);
    }
}

/// Inject the planted divergence into a (cloned) ref state, mirror-only. The
/// live proptest state stays correct — the wrong *reference* data is injected
/// only at the observation boundary, exactly as the old `build_ref`/snapshot
/// path did:
/// - `LoroOrder`: reverse the children's sibling order (swap `c1`/`c2` seq).
/// - `Editor`: append `-WRONG` to the active editor's in-memory text (no-op
///   when no editor is open — so it only bites `{EditorState}`).
/// - `Content`: diverge `c1`'s block content.
pub(crate) fn apply_plant(state: &mut ReferenceState, plant: Plant) {
    let ids = fixed_ids();
    match plant {
        Plant::None => {}
        Plant::LoroOrder => {
            if let Some(b) = state.domain.block_state.blocks.get_mut(&ids.c1) {
                b.set_sequence(1);
            }
            if let Some(b) = state.domain.block_state.blocks.get_mut(&ids.c2) {
                b.set_sequence(0);
            }
        }
        Plant::Editor => {
            if let Some(e) = state.ui.tab.active_editor.as_mut() {
                e.in_memory_content.push_str("-WRONG");
            }
        }
        Plant::Content => {
            if let Some(b) = state.domain.block_state.blocks.get_mut(&ids.c1) {
                b.content = "c1-WRONG".to_string();
            }
        }
    }
}
