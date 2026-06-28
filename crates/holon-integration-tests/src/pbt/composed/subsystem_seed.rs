//! **Shared subsystem-config seeding + the deterministic shrink-causality proofs.**
//!
//! The durable *composed-catalog + planted-`ReferenceState`* world for
//! subsystem-config testing (F2 Stage 2, "Plan 1"). It is self-contained so it
//! survives when the lean proptest spike (the thin `prop_state_machine!` shell
//! that consumes these helpers) is replaced by the faithful convergence harness.
//! This module owns:
//!
//! - The real SUT/oracle construction shared by every subsystem config:
//!   [`build_sut`] (a real Loro or `MemoryBackend` store + an optional real
//!   `InProcEditorSut` write target), [`build_started_ref`] (the live
//!   `ReferenceState` oracle, seeded identically), and [`evaluate`] (the seam
//!   that runs the shared catalog for one config).
//! - The planted-divergence technique ([`Plant`] / [`apply_plant`]) — wrong
//!   *reference* data injected at the observation boundary, never fake components.
//! - A deterministic lockstep driver expressed as free functions
//!   ([`apply_op_to_ref`], [`apply_op_to_sut`], [`transition_arms`],
//!   [`op_preconditions`]) so both the spike's `prop_state_machine!` AND the
//!   millisecond regression proofs below drive the SAME write path.
//!
//! The `#[cfg(test)] mod tests` here is the **deterministic shrink-causality
//! anchor**: `loro_order`/`editor`/`content` plants minimize to their causal
//! subsystem set, catalog selection follows the config, and the production
//! `InProcEditorSut` write path commits + matches the ref (with a non-vacuous
//! teeth proof). These run in milliseconds without booting a real app — the cheap
//! causality oracle the faithful harness ("Plan 2") defers to.

use std::collections::BTreeSet;
use std::sync::Arc;

use holon::api::MemoryBackend;
use holon_api::Region;
use holon_api::repository::{CoreOperations, Lifecycle};
use holon_api::types::ContentType;
use holon_loro::LoroBackend;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::composition::{CapProvider, RunReport};

// Direct imports of the prelude symbols (previously pulled via `fixtures::*`, which
// is `cfg(test)`-only — this module is now `pbt`-gated so it can back the composed
// harness from integration tests, so it must not depend on the test-double module).
use crate::pbt::composed::catalog::composed_invariant_catalog;
use crate::pbt::invariants::registry::Subsystem;
use crate::pbt::loro_slice::components::LoroBackendComponent;
use crate::pbt::memory_slice::components::{InMemEditorComponent, MemoryBackendComponent};
use holon_api::{Block, BlockContent, EntityUri};
use holon_pbt_core::composition::{CapMap, run_selected};

// Production editor transitions — the SAME structs, generators, and
// capability-bound preconditions the blessed slices use. The generic config
// drives generation through these and gates them through the production
// preconditions, which consult `RefLifecycle::has_editor_buffer`.
use holon_orgmode::models::OrgBlockExt;
use holon_pbt_core::capabilities::{CapCursor, CapRegion, RefEditorMirrorMut, RefFocusMut};
use holon_pbt_core::{Actor, StorageAdapter, Wiring};
use validated::Validated;

use crate::pbt::reference_capabilities::reference_state_ref_caps;
use crate::pbt::reference_state::{ReferenceState, Resolved};
use crate::pbt::state_machine::{fresh_reference_state, started_reference_state};
use crate::pbt::transitions::delete_backward::{DeleteBackward, delete_backward_preconditions};
use crate::pbt::transitions::start_app::seed_booted_layout_into_ref;
use crate::pbt::transitions::type_chars::{TypeChars, type_chars_preconditions};

// Fixed-id seeding primitives (`PARENT`/`C1`/`C2`, `Ids`, `fixed_ids`, `Plant`,
// `seed_ref_tree`, `apply_plant`) live in `super::seed_primitives` so the windowed
// slice can share them in the `pbt` build; re-imported here for the spike body.
use super::seed_primitives::{C1, C2, Ids, PARENT, Plant, apply_plant, fixed_ids, seed_ref_tree};

// ─────────────────────────────────────────────────────────────────
// Planted-bug + universe selection (env-driven, fail-loud on typos)
// ─────────────────────────────────────────────────────────────────

/// Parse a `HOLON_PBT_SUBSYSTEMS` spec into the optional universe. Fail-loud on an
/// unknown name (a typo must not silently shrink the universe). Pure — the env
/// read lives in the spike's `optional_universe` so this is unit-testable.
pub(crate) fn parse_universe(spec: &str) -> Vec<Subsystem> {
    spec.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|name| match name {
            "loro" => Subsystem::Loro,
            "editor" => Subsystem::EditorState,
            other => panic!(
                "unknown subsystem {other:?} in HOLON_PBT_SUBSYSTEMS; \
                 spike scope is in-process only (loro, editor)"
            ),
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────
// Shared SUT/oracle construction (used by both the proptest path and
// the deterministic regression tests)
// ─────────────────────────────────────────────────────────────────

pub(crate) fn new_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build current-thread runtime")
}

/// Seed a `parent` block with two children into a real store (Loro or memory),
/// using the **fixed shared ids** so the store's stable ids match the ref tree.
pub(crate) async fn seed_store<B: CoreOperations>(backend: &B, ids: &Ids) {
    backend
        .create_block(
            EntityUri::no_parent(),
            BlockContent::text(PARENT),
            Some(ids.parent.clone()),
        )
        .await
        .expect("seed parent");
    backend
        .create_block(
            ids.parent.clone(),
            BlockContent::text(C1),
            Some(ids.c1.clone()),
        )
        .await
        .expect("seed c1");
    backend
        .create_block(
            ids.parent.clone(),
            BlockContent::text(C2),
            Some(ids.c2.clone()),
        )
        .await
        .expect("seed c2");
}

/// Build the SUT `CapMap` from the generated config: a real Loro store when
/// `Loro` is present (else a real `MemoryBackend`), plus a real editor when
/// `EditorState` is present.
///
/// The backend is built ONCE behind an `Arc` and shared two ways: the
/// `*BackendComponent` read cap (via `new_shared`) and the [`InProcEditorSut`]
/// write target (coerced to `Arc<dyn CoreOperations>`). So an editor commit
/// lands in the SAME store the block invariants read — committed-content parity
/// falls out of the capability system (the F2 keystone). Returns the write
/// target so the apply phase drives the production `apply_to_sut` against it.
pub(crate) fn build_sut(
    rt: &tokio::runtime::Runtime,
    has_loro: bool,
    has_editor: bool,
) -> (CapMap, Option<Arc<InMemEditorComponent>>) {
    rt.block_on(async {
        let mut caps = CapMap::new();
        let ids = fixed_ids();
        let store: Arc<dyn CoreOperations> = if has_loro {
            let backend = Arc::new(
                LoroBackend::create_new("subsystem-shrink".to_string())
                    .await
                    .expect("create_new LoroBackend"),
            );
            seed_store(backend.as_ref(), &ids).await;
            Arc::new(LoroBackendComponent::new_shared(backend.clone())).register(&mut caps);
            backend
        } else {
            let backend = Arc::new(
                MemoryBackend::create_new("subsystem-shrink".to_string())
                    .await
                    .expect("create_new MemoryBackend"),
            );
            seed_store(backend.as_ref(), &ids).await;
            Arc::new(MemoryBackendComponent::new_shared(backend.clone())).register(&mut caps);
            backend
        };
        let editor_sut = if has_editor {
            // Fail-loud guard for the `ContentType::Text` hardcode in the
            // commit path: Stage 1 only ever opens the editor on a text block.
            // If the c1 seed ever becomes non-Text, break the build here instead
            // of silently diverging on block content.
            let c1_block = store
                .get_block(ids.c1.as_str())
                .await
                .expect("seeded c1 must exist");
            assert_eq!(
                c1_block.content_type,
                ContentType::Text,
                "editor commit hardcodes ContentType::Text; c1 seed is no longer Text"
            );
            // The editor IS the write target (Stage-1b collapse): commits into the
            // SAME `store` the block read cap observes.
            let editor = Arc::new(InMemEditorComponent::new(store.clone()));
            editor.open(ids.c1.clone(), C1.to_string());
            editor.clone().register(&mut caps);
            Some(editor)
        } else {
            None
        };
        (caps, editor_sut)
    })
}

/// Map the generated active set onto a PBT [`Wiring`]: `Loro` ⇒ the Loro
/// storage adapter, `EditorState` ⇒ the `UI` actor (the honest `has_editor_buffer`
/// source). `BlockTree` is the always-on substrate and needs no manifest entry.
/// No `validate()` — the ref only reads `has_storage(Loro)` / `has_actor(UI)`.
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
/// `move_cursor`) in lockstep with the SUT — replacing the retired `EditorModel`.
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
/// ref must expose **exactly** `expected` non-seed block ids and the set must be
/// **non-empty**. A ref seeded empty/wrong otherwise passes the ref-comparing
/// invariants *vacuously* (empty-vs-empty → green) — the CLAUDE.md "silently
/// degrades to look fine" trap. Pair with the invariants' own `ran_ids` checks
/// (R4): together they prove the comparison ran AND had real data on both sides.
pub(crate) fn assert_ref_seeded(state: &ReferenceState, expected: &[EntityUri]) {
    let expected: BTreeSet<EntityUri> = expected.iter().cloned().collect();
    assert!(
        !expected.is_empty(),
        "seed-parity: a positive slice must seed a non-empty block set (else the \
         ref-comparison invariants pass vacuously)"
    );
    assert_eq!(
        ref_non_seed_ids(state),
        expected,
        "seed-parity: the ref's non-seed block ids must match the SUT-seeded ids \
         (id-scheme drift / missing seed is the historical failure mode)"
    );
}

/// Run the shared catalog with a freshly-seeded [`ReferenceState`] ref against a
/// SUT cap map from within an **async** (`#[tokio::test]`) context.
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
    // `structural_ref_wired`; the SUT boots fresh (no user index.org), so `fresh=true`.
    let mut state = fresh_reference_state(wiring_for_subsystems(subsystems));
    state.action.app_started = true;
    seed_booted_layout_into_ref(&mut state, true);
    seed_ref_tree(&mut state);
    if state.wiring.has_actor(Actor::UI) {
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

/// Inject the planted divergence into a (cloned) ref state, mirror-only. The
/// live proptest state stays correct — the wrong *reference* data is injected
/// only at the observation boundary, exactly as the old `build_ref`/snapshot
/// path did:
/// - `LoroOrder`: reverse the children's sibling order (swap `c1`/`c2` seq).
/// - `Editor`: append `-WRONG` to the active editor's in-memory text (no-op
///   when no editor is open — so it only bites `{EditorState}`).
/// Run the shared catalog for an explicit config — the seam both the proptest
/// `check_invariants` and the regression tests go through. The ref is the live
/// `ReferenceState` (Phase 1 keystone), with the plant injected at observation.
pub(crate) fn evaluate(subsystems: &BTreeSet<Subsystem>, plant: Plant) -> RunReport {
    let rt = new_runtime();
    let has_loro = subsystems.contains(&Subsystem::Loro);
    let has_editor = subsystems.contains(&Subsystem::EditorState);
    let (caps, _editor) = build_sut(&rt, has_loro, has_editor);
    let mut ref_state = build_started_ref(subsystems);
    apply_plant(&mut ref_state, plant);
    let ref_caps = reference_state_ref_caps(Resolved::identity(ref_state).map(Arc::new));
    rt.block_on(run_selected(
        &composed_invariant_catalog(),
        &caps,
        &ref_caps,
    ))
}

// ─────────────────────────────────────────────────────────────────
// Deterministic lockstep driver — free functions so BOTH the proptest
// spike machine AND the regression proofs drive the SAME write path.
// ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub(crate) enum SpikeTransition {
    /// Production editor transitions — valid only when `EditorState` is wired
    /// (their production preconditions consult `has_editor_buffer`).
    TypeChars(TypeChars),
    DeleteBackward(DeleteBackward),
    /// Always valid no-op, so the alphabet is non-empty even with no editor.
    Touch,
}

/// The PRODUCTION capability-bound preconditions — the same gate
/// `proptest_state_machine` consults during shrink replay. They check
/// `has_editor_buffer`, so a transition generated while `EditorState` was wired
/// is dropped once the shrinker removes it.
pub(crate) fn op_preconditions(state: &ReferenceState, transition: &SpikeTransition) -> bool {
    match transition {
        SpikeTransition::TypeChars(_) => {
            matches!(type_chars_preconditions(state), Validated::Good(()))
        }
        SpikeTransition::DeleteBackward(_) => {
            matches!(delete_backward_preconditions(state), Validated::Good(()))
        }
        SpikeTransition::Touch => true,
    }
}

/// Apply a transition to the `ReferenceState` oracle: mutate the `ActiveEditor`
/// via the mirror methods, then commit the live text into block content with
/// `commit_active_editor_if_changed` — the SAME normalization the SUT's
/// `InProcEditorSut` applies. TEXT-ONLY: the ref uses the mirror
/// `delete_backward`, NOT the production structural join, so a backspace at
/// caret 0 does NOT trigger a block join (the SUT skips it too — the cursor-0
/// join is out of scope this stage).
pub(crate) fn apply_op_to_ref(
    mut state: ReferenceState,
    transition: &SpikeTransition,
) -> ReferenceState {
    match transition {
        SpikeTransition::TypeChars(t) => {
            state.type_chars(&t.text);
            state.commit_active_editor_if_changed();
            state.action.last_transition_kind = Some("TypeChars");
        }
        SpikeTransition::DeleteBackward(t) => {
            state.delete_backward(t.count);
            state.commit_active_editor_if_changed();
            state.action.last_transition_kind = Some("DeleteBackward");
        }
        SpikeTransition::Touch => {}
    }
    state
}

/// Apply a transition to the SUT through the PRODUCTION `apply_to_sut` impls —
/// the same write path the blessed slices use. `TypeChars`/`DeleteBackward`
/// each bind only `SutEditorMirrorWrite`, so they drive `InProcEditorSut`,
/// which mutates the real editor math AND commits into the shared store.
///
/// Criterion-4 tripwire: an editor transition reaching an editor-less SUT means
/// precondition replay failed to drop it after the config shrank `EditorState`
/// off. Fail loud instead of silently passing. `block_on` is safe here (the
/// driver is sync, no ambient runtime; a single awaited future, no spawn).
pub(crate) fn apply_op_to_sut(
    rt: &tokio::runtime::Runtime,
    editor_sut: &mut Option<Arc<InMemEditorComponent>>,
    ref_state: &ReferenceState,
    transition: SpikeTransition,
) {
    if let SpikeTransition::Touch = transition {
        return;
    }
    let target = editor_sut.as_mut().unwrap_or_else(|| {
        panic!(
            "criterion-4 VIOLATION: editor transition applied to a SUT without an editor — \
             precondition replay failed to drop the invalidated transition after the \
             config shrank EditorState off"
        )
    });
    rt.block_on(async {
        match transition {
            SpikeTransition::TypeChars(t) => t.apply_to_sut(ref_state, target).await,
            SpikeTransition::DeleteBackward(t) => t.apply_to_sut(ref_state, target).await,
            SpikeTransition::Touch => unreachable!("Touch handled above"),
        }
    });
}

#[cfg(test)]
mod tests {
    //! Deterministic, **green** proofs of the shrink semantics — the planted-bug
    //! results without relying on a manual env-gated failing run. Each drives the
    //! shared [`evaluate`] / lockstep seam directly and asserts the causal
    //! structure the shrinker exploits.
    use super::*;

    /// `{BlockTree}` + the given optional subsystems.
    fn config(optional: &[Subsystem]) -> BTreeSet<Subsystem> {
        let mut s: BTreeSet<Subsystem> = [Subsystem::BlockTree].into_iter().collect();
        s.extend(optional.iter().copied());
        s
    }

    fn fails(subsystems: &[Subsystem], plant: Plant) -> bool {
        !evaluate(&config(subsystems), plant).failures().is_empty()
    }

    /// Criterion 2/3 (Loro): the reversed-children bug is causally necessary on
    /// `Loro` and irrelevant to `EditorState` — so it minimizes to `{Loro}`.
    #[test]
    fn loro_order_bug_is_caused_by_loro_only() {
        assert!(
            fails(&[Subsystem::Loro], Plant::LoroOrder),
            "must fail with Loro"
        );
        assert!(!fails(&[], Plant::LoroOrder), "must pass on a memory store");
        assert!(
            !fails(&[Subsystem::EditorState], Plant::LoroOrder),
            "the editor is irrelevant to a Loro-order bug"
        );
        assert!(
            fails(&[Subsystem::Loro, Subsystem::EditorState], Plant::LoroOrder),
            "still fails with the irrelevant editor also present"
        );
    }

    /// Criterion 2/3 (editor): the editor-text bug is causally necessary on
    /// `EditorState` and irrelevant to `Loro` — so it minimizes to `{EditorState}`.
    #[test]
    fn editor_bug_is_caused_by_editor_only() {
        assert!(
            fails(&[Subsystem::EditorState], Plant::Editor),
            "must fail with editor"
        );
        assert!(
            !fails(&[Subsystem::Loro], Plant::Editor),
            "Loro is irrelevant to an editor bug"
        );
        assert!(
            !fails(&[], Plant::Editor),
            "no editor ⇒ editor invariants deselect"
        );
    }

    /// Criterion 2 (lower) / 3: the content bug reproduces for every config, so
    /// the shrinker removes *both* optionals → `{}`.
    #[test]
    fn content_bug_is_independent_of_the_optionals() {
        for optional in [
            vec![],
            vec![Subsystem::Loro],
            vec![Subsystem::EditorState],
            vec![Subsystem::Loro, Subsystem::EditorState],
        ] {
            assert!(
                fails(&optional, Plant::Content),
                "content bug must reproduce for optional={optional:?}"
            );
        }
    }

    /// The committed default: no plant ⇒ every selected invariant holds, for the
    /// whole powerset of optionals.
    #[test]
    fn none_plant_is_green_for_every_config() {
        for optional in [
            vec![],
            vec![Subsystem::Loro],
            vec![Subsystem::EditorState],
            vec![Subsystem::Loro, Subsystem::EditorState],
        ] {
            let report = evaluate(&config(&optional), Plant::None);
            assert!(
                report.failures().is_empty(),
                "none plant must be green for optional={optional:?}: {:?}",
                report.failures(),
            );
        }
    }

    /// Criterion 5 / honesty: the generated config drives catalog *selection* —
    /// the subsystem-specific invariants run iff their subsystem is wired, and are
    /// disclosed-deselected otherwise (never a vacuous pass).
    #[test]
    fn selection_follows_config() {
        let loro = evaluate(&config(&[Subsystem::Loro]), Plant::None);
        assert!(
            loro.ran_ids().contains(&"inv-loro-children-match-ref"),
            "Loro wired ⇒ the Loro invariant runs; ran={:?}",
            loro.ran_ids(),
        );
        let editor = evaluate(&config(&[Subsystem::EditorState]), Plant::None);
        assert!(
            editor.ran_ids().contains(&"inv-editor-text-matches-ref"),
            "editor wired ⇒ the editor invariant runs; ran={:?}",
            editor.ran_ids(),
        );
        let memory = evaluate(&config(&[]), Plant::None);
        assert!(
            !memory.ran_ids().contains(&"inv-loro-children-match-ref")
                && !memory.ran_ids().contains(&"inv-editor-text-matches-ref"),
            "neither optional wired ⇒ both subsystem invariants deselected; ran={:?}",
            memory.ran_ids(),
        );
    }

    /// Criterion 4 at the precondition level: editor transitions are valid iff
    /// `EditorState` is wired. This is exactly the gate `proptest_state_machine`
    /// consults during replay to drop transitions invalidated by a shrunk config.
    #[test]
    fn editor_transitions_gated_on_editor_state() {
        let with = build_started_ref(&config(&[Subsystem::EditorState]));
        let without = build_started_ref(&config(&[]));
        let typ = SpikeTransition::TypeChars(TypeChars {
            text: "x".to_string(),
        });
        assert!(
            op_preconditions(&with, &typ),
            "TypeChars valid with editor (production precondition: has_editor_buffer)"
        );
        assert!(
            !op_preconditions(&without, &typ),
            "TypeChars invalid without editor (has_editor_buffer false)"
        );
        assert!(
            op_preconditions(&without, &SpikeTransition::Touch),
            "Touch is always valid (keeps the editor-less sequence non-empty)"
        );
    }

    // ── F2 Stage 1: production write-path / committed-content parity ──────────
    //
    // `evaluate` above checks only the SEEDED state. These drive a real
    // transition sequence through BOTH the production SUT write path
    // (`apply_op_to_sut` → `apply_to_sut` → `InProcEditorSut` commit) and the
    // reference (`apply_op_to_ref` → `commit_active_editor_if_changed`), so they
    // exercise the NEW committed-content write path.

    /// Drive `transitions` through the ref and the SUT in lockstep, then run the
    /// catalog with the live ref as the oracle (no plant).
    fn drive_and_check(
        subsystems: &BTreeSet<Subsystem>,
        transitions: &[SpikeTransition],
    ) -> RunReport {
        let rt = new_runtime();
        let has_loro = subsystems.contains(&Subsystem::Loro);
        let has_editor = subsystems.contains(&Subsystem::EditorState);
        let (caps, mut editor_sut) = build_sut(&rt, has_loro, has_editor);
        let mut ref_state = build_started_ref(subsystems);
        for t in transitions {
            ref_state = apply_op_to_ref(ref_state, t);
            apply_op_to_sut(&rt, &mut editor_sut, &ref_state, t.clone());
        }
        let ref_caps = reference_state_ref_caps(Resolved::identity(ref_state).map(Arc::new));
        rt.block_on(run_selected(
            &composed_invariant_catalog(),
            &caps,
            &ref_caps,
        ))
    }

    fn type_chars(s: &str) -> SpikeTransition {
        SpikeTransition::TypeChars(TypeChars {
            text: s.to_string(),
        })
    }

    /// H3 (positive): a real `TypeChars` commits typed text to `c1` on BOTH sides
    /// and `inv-block-content-matches-ref/block_raw` stays green — the committed-
    /// content payoff. The green is meaningful ONLY because a write actually ran
    /// (asserted non-vacuous) and the read cap + commit target share one backend.
    #[test]
    fn typed_content_commits_and_matches_ref() {
        let report = drive_and_check(&config(&[Subsystem::EditorState]), &[type_chars("Z")]);
        assert!(
            report.failures().is_empty(),
            "typed content must commit and match the ref: {:?}",
            report.failures(),
        );
        assert!(
            report
                .ran_ids()
                .contains(&"inv-block-content-matches-ref/block_raw"),
            "block-content invariant must actually run (non-vacuous); ran={:?}",
            report.ran_ids(),
        );
    }

    /// H3 (teeth) / Task #2: hand `InProcEditorSut` a SEPARATE backend `Arc` from
    /// the one the read cap observes. The commit lands where the invariant can't
    /// see it, so `inv-block-content-matches-ref/block_raw` MUST go RED. Proves
    /// the positive test is not vacuous — `Arc`-sharing is load-bearing, and the
    /// block-content invariant genuinely observes the new commit write path.
    #[test]
    fn broken_sharing_turns_block_content_red() {
        let rt = new_runtime();
        let ids = fixed_ids();
        let (caps, mut editor_sut) = rt.block_on(async {
            let mut caps = CapMap::new();
            // Read cap observes the `observed` backend (stays at the seeded "c1").
            let observed = Arc::new(
                MemoryBackend::create_new("observed".to_string())
                    .await
                    .expect("create observed backend"),
            );
            seed_store(observed.as_ref(), &ids).await;
            Arc::new(MemoryBackendComponent::new_shared(observed.clone())).register(&mut caps);
            // Editor commits into a DIFFERENT backend — deliberately unshared, so
            // the read cap never sees the write.
            let unobserved = Arc::new(
                MemoryBackend::create_new("unobserved".to_string())
                    .await
                    .expect("create unobserved backend"),
            );
            seed_store(unobserved.as_ref(), &ids).await;
            // The editor commits into the UNOBSERVED backend, while the read cap
            // registered above reads a different store — so the write is never seen.
            let editor = Arc::new(InMemEditorComponent::new(
                unobserved as Arc<dyn CoreOperations>,
            ));
            editor.open(ids.c1.clone(), C1.to_string());
            editor.clone().register(&mut caps);
            (caps, Some(editor))
        });
        let subsystems = config(&[Subsystem::EditorState]);
        let mut ref_state = build_started_ref(&subsystems);
        let t = type_chars("Z");
        ref_state = apply_op_to_ref(ref_state, &t);
        apply_op_to_sut(&rt, &mut editor_sut, &ref_state, t);
        let ref_caps = reference_state_ref_caps(Resolved::identity(ref_state).map(Arc::new));
        let report = rt.block_on(run_selected(
            &composed_invariant_catalog(),
            &caps,
            &ref_caps,
        ));
        assert!(
            report
                .failures()
                .iter()
                .any(|(id, _)| *id == "inv-block-content-matches-ref/block_raw"),
            "broken sharing must make block-content RED; failures={:?} ran={:?}",
            report.failures(),
            report.ran_ids(),
        );
    }

    /// H2 / Task #1: the FIRST time typed editor content is committed into a REAL
    /// `LoroBackend` and read back via `block_raw_snapshot` — a path with a
    /// documented history of content divergence (scheme parsing, `content_raw`
    /// cell fork). Verify the `{Loro, EditorState}` config explicitly with a
    /// non-empty type/delete/type sequence rather than trusting proptest to hit it.
    #[test]
    fn loro_write_path_commits_and_matches_ref() {
        let report = drive_and_check(
            &config(&[Subsystem::Loro, Subsystem::EditorState]),
            &[
                type_chars("Z"),
                SpikeTransition::DeleteBackward(DeleteBackward { count: 1 }),
                type_chars("Y"),
            ],
        );
        assert!(
            report.failures().is_empty(),
            "Loro-committed typed content must match the ref: {:?}",
            report.failures(),
        );
        assert!(
            report
                .ran_ids()
                .contains(&"inv-block-content-matches-ref/block_raw"),
            "block-content must run over the Loro store; ran={:?}",
            report.ran_ids(),
        );
        assert!(
            report.ran_ids().contains(&"inv-loro-children-match-ref"),
            "the Loro-children invariant must also run; ran={:?}",
            report.ran_ids(),
        );
    }

    #[test]
    fn parses_known_subsystems() {
        assert_eq!(
            parse_universe("loro, editor"),
            vec![Subsystem::Loro, Subsystem::EditorState]
        );
    }

    #[test]
    fn empty_spec_yields_empty_universe() {
        assert!(parse_universe("").is_empty());
        assert!(parse_universe("  ,  ").is_empty());
    }

    #[test]
    #[should_panic(expected = "unknown subsystem")]
    fn unknown_subsystem_fails_loud() {
        // `turso` is a real subsystem but out of the in-process spike scope.
        let _ = parse_universe("turso");
    }
}
