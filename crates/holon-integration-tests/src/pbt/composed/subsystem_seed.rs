//! **Shared subsystem-config seeding primitives.**
//!
//! The durable seeding helpers the composed wide harness (`wide_e2e.rs`,
//! `harness.rs`) and the static slices use to build a live `ReferenceState`
//! oracle for a given subsystem config:
//!
//! - [`seed_store`] — seed the `parent/c1/c2` working tree into a real store
//!   with the fixed shared ids (so the SUT store and the ref tree agree).
//! - [`seed_ref`] / [`seed_ref_with_editor`] — a started `ReferenceState` whose
//!   working tree is exactly the given blocks (all non-seed), optionally with
//!   an open editor.
//! - [`build_started_ref`] — the live `ReferenceState` oracle for a config:
//!   started, the booted default layout modeled (via
//!   `seed_booted_layout_into_ref`), the `parent/c1/c2` tree, and — only when
//!   the UI actor is wired — an open editor on `c1`.
//! - [`run_with_seeded_ref`] — run the shared catalog against a freshly-seeded
//!   ref from an async test context (handles the off-executor runtime drop).
//! - [`assert_ref_seeded`] / [`ref_non_seed_ids`] — fail-loud non-vacuity +
//!   seed-parity guards for positive slices.
//!
//! The lean proptest spike that once drove these (planted-`ReferenceState`
//! shrink-causality proofs, an in-process `build_sut`/`evaluate` seam) has been
//! superseded by the faithful convergence harness (`general_e2e_composed_pbt`);
//! only the shared seeding primitives remain.

use std::collections::BTreeSet;
use std::sync::Arc;

use holon_api::Block;
use holon_api::EntityUri;
use holon_api::Region;
use holon_orgmode::models::OrgBlockExt;
use holon_pbt_core::Actor;
use holon_pbt_core::StorageAdapter;
use holon_pbt_core::Wiring;
use holon_pbt_core::capabilities::CapCursor;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefFocusMut;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::RunReport;
use holon_pbt_core::composition::run_selected;

// Fixed-id seeding primitives (`C1`, `fixed_ids`, `seed_ref_tree`) live in
// `super::seed_primitives` so the windowed slice can share them in the `pbt` build.
use super::seed_primitives::{C1, fixed_ids, seed_ref_tree};
use crate::pbt::invariants::registry::Subsystem;
use crate::pbt::reference_capabilities::reference_state_ref_caps;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::reference_state::Resolved;
use crate::pbt::state_machine::fresh_reference_state;
use crate::pbt::state_machine::started_reference_state;
use crate::pbt::transitions::start_app::seed_booted_layout_into_ref;

// ─────────────────────────────────────────────────────────────────
// Shared SUT/oracle construction — seeding primitives used by the
// composed wide harness and the static slices.
// ─────────────────────────────────────────────────────────────────

/// Map the generated active set onto a PBT [`Wiring`]: `Loro` ⇒ the Loro
/// storage adapter, `EditorState` ⇒ the `UI` actor (the honest
/// `has_editor_buffer` source). `BlockTree` is the always-on substrate and
/// needs no manifest entry. No `validate()` — the ref only reads
/// `has_storage(Loro)` / `has_actor(UI)`.
pub(crate) fn wiring_for_subsystems(subsystems: &BTreeSet<Subsystem>) -> Wiring {
    let storage = subsystems
        .contains(&Subsystem::Loro)
        .then_some(StorageAdapter::Loro);
    let actors = subsystems
        .contains(&Subsystem::EditorState)
        .then_some(Actor::UI);
    Wiring::custom(storage, [], actors)
}

/// Build a started [`ReferenceState`] whose working tree is exactly `blocks`
/// (every block **non-seed**: no `block_documents` entry), with per-parent
/// sequences assigned in input order so the ref's `sorted_children` preserves
/// the order the blocks were given — the flat-list semantics the retired
/// `FixtureRef` had. This is the R3 keystone the static slices and catch tests
/// use in place of `ref_map`. The wiring is empty (the block invariants read no
/// wiring; the editor variant is [`seed_ref_with_editor`]).
pub(crate) fn seed_ref(blocks: Vec<Block>) -> ReferenceState {
    let mut state = started_reference_state(Wiring::custom([], [], []));
    let mut next_seq: std::collections::BTreeMap<EntityUri, i64> =
        std::collections::BTreeMap::new();
    for mut b in blocks {
        let seq = next_seq.entry(b.parent_id.clone()).or_insert(0);
        b.set_sequence(*seq);
        *seq += 1;
        state.domain.block_state.blocks.insert(b.id.clone(), b);
    }
    state
}

/// A started [`ReferenceState`] seeded with `blocks` (all non-seed) **plus** an
/// open editor on `editor_block` showing `editor_text`, caret at end-of-text —
/// the ref side of the editor invariants. Mirrors the SUT
/// [`InMemEditorComponent::open`](crate::pbt::memory_slice::components::InMemEditorComponent),
/// which seeds the caret to `text.len()`. The editor mirror is driven afterward
/// via the [`RefEditorMirrorMut`] methods (`type_chars`/`delete_backward`/
/// `move_cursor`) in lockstep with the SUT — replacing the retired
/// `EditorModel`.
pub(crate) fn seed_ref_with_editor(
    blocks: Vec<Block>,
    editor_block: EntityUri,
    editor_text: &str,
) -> ReferenceState {
    let mut state = seed_ref(blocks);
    state.open_active_editor(editor_block, editor_text.to_string(), editor_text.len());
    state
}

/// The non-seed block ids the ref currently exposes (the set the block
/// invariants compare against the SUT store). Mirrors the `is_seed` predicate
/// in [`RefBackend::non_seed_blocks`] / `all_non_seed_block_ids`.
pub(crate) fn ref_non_seed_ids(state: &ReferenceState) -> BTreeSet<EntityUri> {
    state
        .domain
        .block_state
        .blocks
        .keys()
        .filter(|uri| {
            !state
                .domain
                .block_state
                .block_documents
                .get(uri)
                .is_some_and(|d| d.is_no_parent() || d.is_sentinel())
        })
        .cloned()
        .collect()
}

/// Fail-loud non-vacuity + seed-parity guard for a positive static slice: the
/// ref must expose **exactly** `expected` non-seed block ids and the set must
/// be **non-empty**. A ref seeded empty/wrong otherwise passes the
/// ref-comparing invariants *vacuously* (empty-vs-empty → green) — the
/// CLAUDE.md "silently degrades to look fine" trap. Pair with the invariants'
/// own `ran_ids` checks (R4): together they prove the comparison ran AND had
/// real data on both sides.
pub(crate) fn assert_ref_seeded(state: &ReferenceState, expected: &[EntityUri]) {
    let expected: BTreeSet<EntityUri> = expected.iter().cloned().collect();
    assert!(
        !expected.is_empty(),
        "seed-parity: a positive slice must seed a non-empty block set (else the ref-comparison \
         invariants pass vacuously)"
    );
    assert_eq!(
        ref_non_seed_ids(state),
        expected,
        "seed-parity: the ref's non-seed block ids must match the SUT-seeded ids (id-scheme drift \
         / missing seed is the historical failure mode)"
    );
}

/// Run the shared catalog with a freshly-seeded [`ReferenceState`] ref against
/// a SUT cap map from within an **async** (`#[tokio::test]`) context.
///
/// `ReferenceState` owns an `Arc<tokio::runtime::Runtime>` (it drives its own
/// async ops). Dropping that runtime on the test's async executor panics
/// ("Cannot drop a runtime in a context where blocking is not allowed"), so the
/// final drop is moved to a fresh std thread (no tokio context) after the run.
/// The block-only [`evaluate`] path doesn't need this because it runs the whole
/// config in a sync `block_on` scope; the async slice/catch tests do.
pub(crate) async fn run_with_seeded_ref(
    registry: &[Box<dyn holon_pbt_core::composition::CapInvariant>],
    sut: &CapMap,
    ref_state: Resolved<ReferenceState>,
) -> RunReport {
    let arc: Resolved<Arc<ReferenceState>> = ref_state.map(Arc::new);
    // Keep a bare `Arc` clone for the off-executor final drop: `ref_caps` holds
    // its own `Arc` clones, so dropping it only decrements the refcount; this
    // handle carries the last ref and drops the owned tokio `Runtime` off-thread.
    let drop_handle = arc.get().clone();
    let ref_caps = reference_state_ref_caps(arc);
    let report = run_selected(registry, sut, &ref_caps).await;
    drop(ref_caps);
    std::thread::spawn(move || drop(drop_handle))
        .join()
        .expect("drop ReferenceState (owns a tokio Runtime) off the async executor");
    report
}

/// The live `ReferenceState` oracle for a config: a started state (Phase 2),
/// the seeded `parent/c1/c2` tree, and — **only when the UI actor is wired** —
/// an open editor on `c1`. The UI gate is load-bearing: `transitions()` reads
/// this state, so opening an editor for a `{Loro}`/`{}` config would let the
/// editor transitions generate and then trip the editor-less-SUT panic.
pub(crate) fn build_started_ref(subsystems: &BTreeSet<Subsystem>) -> ReferenceState {
    // Model the REAL default-asset layout the booted `full_headless` SUT carries
    // (the 9 `index.org` blocks + `journals` page shell + `__default__`), not the
    // single phantom `started-ref-layout-query` that `started_reference_state`
    // inserts — so `inv-watch-rows-match-ref` (which compares the full `from block`
    // result set) sees the same blocks on both sides. The real `*::src::0` query
    // blocks populate `layout_blocks.query_source_ids`, so `is_properly_setup()`
    // (the `SetupWatch` precondition) stays true. Nav focus/history is left to
    // `structural_ref_wired`; the SUT boots fresh (no user index.org), so
    // `fresh=true`.
    let mut state = fresh_reference_state(wiring_for_subsystems(subsystems));
    state.action.app_started = true;
    seed_booted_layout_into_ref(&mut state, true);
    // The booted SUT's ProfileResolver serves the bundled block profile; the
    // oracle must carry the same one or `render_entity` interprets to Empty
    // and ToggleState never generates (see `load_seed_profile_into_ref`).
    crate::pbt::transitions::start_app::load_seed_profile_into_ref(&mut state);
    seed_ref_tree(&mut state);
    if state.harness.wiring.has_actor(Actor::UI) {
        let c1 = fixed_ids().c1;
        let caret = C1.len();
        // Seed navigation focus on `c1` — this is what `current_focus(Main)`
        // reads (the navigation history, not `focused_entity_id`), mirroring the
        // production `NavigateFocus` apply: push a history row and advance the
        // cursor. Without this the editor preconditions' `current_focus(Main)`
        // check fails and no editor transition ever generates.
        let history = state
            .ui
            .tab
            .navigation_history
            .entry(Region::Main)
            .or_default();
        history.entries.truncate(history.cursor + 1);
        history.entries.push(Some(c1.clone()));
        history.cursor = history.entries.len() - 1;
        state.set_focus(
            CapRegion::Main,
            c1.clone(),
            CapCursor {
                line: 0,
                column: C1.chars().count(),
            },
        );
        state.open_active_editor(c1, C1.to_string(), caret);
    }
    state
}
