//! File-per-transition pattern for the E2E PBT.
//!
//! Each transition kind owns its data (a struct), generation strategy
//! (`holon_pbt_core::TransitionFactory<ReferenceState>`), and behaviour
//! (`holon_pbt_core::TransitionImpl<ReferenceState, dyn SutHandle>`). The
//! `declare_e2e_transitions!` macro generates the dispatch enum, the
//! `From<variant>` impls, the hand-rolled trait dispatch on the enum, the
//! `SqlBudget` dispatch, and the strategy aggregator.
//!
//! See `experiments/enum-dispatch-examples/pbt-state-machine/` for the
//! standalone proof-of-concept; this module ports that pattern into
//! the real PBT.
//!
//! ## Cross-PBT trait vocabulary
//!
//! Transitions implement the generic `holon_pbt_core::{TransitionFactory,
//! TransitionImpl}` traits (shared with the layout / editor-pure PBTs)
//! rather than a PBT-local trait. The enum dispatches them by a hand-rolled
//! `match` (a foreign generic trait can't be `enum_dispatch`ed). The
//! integration-test-only SQL budget is split into a separate
//! `transition_budgets::SqlBudget` trait so the shared behaviour trait stays
//! medium-agnostic.
//!
//! - `SutHandle` (this file) is the object-safe view a transition
//!   uses to drive the SUT. `#[async_trait]` so `&mut dyn SutHandle`
//!   works under generic-V SUTs. The trait grows methods as variants
//!   are migrated; today it is empty.

use super::reference_state::ReferenceState;

/// Per-variant weight multiplier read from the `HOLON_PBT_WEIGHTS`
/// environment variable. The macro `declare_e2e_transitions!` applies
/// this automatically to every arm of `aggregate_transitions`, so
/// individual variants don't need to wire it themselves.
///
/// Format: comma-separated `pattern:multiplier` pairs.
/// `pattern` is matched case-insensitively against the variant name.
/// Patterns may contain a single `*` glob for prefix / suffix /
/// contains matching.
///
/// Examples:
///
/// ```text
/// HOLON_PBT_WEIGHTS=Indent:200            # boost a single variant
/// HOLON_PBT_WEIGHTS=Indent:200,Outdent:200,ToggleState:200,Move*:100
/// HOLON_PBT_WEIGHTS=*Edit*:0,Click*:50    # silence one family, boost another
/// ```
///
/// Multiplier `0` removes the variant from the strategy entirely
/// (`weighted_generator` still computes its base weight, but
/// `Union::new_weighted` drops zero-weight arms). Multiplier defaults
/// to `1` for any variant not matched by any pattern.
///
/// Why a single env var: previously each "interesting family"
/// (chord ops, navigation, edit) needed its own bespoke env var
/// wired into each variant's `weighted_generator`. The macro-applied
/// generic shim lets future verification work tune any subset
/// without touching transition source.
pub fn variant_weight_multiplier(variant_name: &'static str) -> u32 {
    use std::sync::OnceLock;

    static PARSED: OnceLock<Vec<(WeightPattern, u32)>> = OnceLock::new();
    let rules = PARSED.get_or_init(parse_weight_env);
    if rules.is_empty() {
        return 1;
    }
    for (pattern, mult) in rules {
        if pattern.matches(variant_name) {
            return *mult;
        }
    }
    1
}

#[derive(Debug)]
enum WeightPattern {
    Exact(String),
    Prefix(String),
    Suffix(String),
    Contains(String),
    Star, // bare `*` → matches every variant
}

impl WeightPattern {
    fn matches(&self, name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        match self {
            Self::Exact(s) => n == *s,
            Self::Prefix(s) => n.starts_with(s),
            Self::Suffix(s) => n.ends_with(s),
            Self::Contains(s) => n.contains(s),
            Self::Star => true,
        }
    }
}

fn parse_weight_env() -> Vec<(WeightPattern, u32)> {
    let raw = match std::env::var("HOLON_PBT_WEIGHTS") {
        Ok(s) if !s.is_empty() => s,
        _ => return Vec::new(),
    };
    let mut rules = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((pat_raw, mult_raw)) = entry.split_once(':') else {
            eprintln!("[HOLON_PBT_WEIGHTS] ignoring '{entry}': expected `pattern:multiplier`");
            continue;
        };
        let mult: u32 = match mult_raw.trim().parse() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("[HOLON_PBT_WEIGHTS] ignoring '{entry}': multiplier must be a u32");
                continue;
            }
        };
        let pat_lower = pat_raw.trim().to_ascii_lowercase();
        let pattern = if pat_lower == "*" {
            WeightPattern::Star
        } else if let Some(stripped) = pat_lower.strip_prefix('*') {
            if let Some(middle) = stripped.strip_suffix('*') {
                WeightPattern::Contains(middle.to_string())
            } else {
                WeightPattern::Suffix(stripped.to_string())
            }
        } else if let Some(stripped) = pat_lower.strip_suffix('*') {
            WeightPattern::Prefix(stripped.to_string())
        } else {
            WeightPattern::Exact(pat_lower)
        };
        rules.push((pattern, mult));
    }
    rules
}

/// Coarse SUT capability bundle for the E2E PBT. Transitions are
/// dispatched generically over `S: SutHandle` (concrete-`S` dispatch,
/// no `dyn`), so the trait uses **native `async fn`** rather than
/// `#[async_trait]` boxing. This is what lets a transition's
/// `TransitionImpl<R, S>` impl be narrowed to fine-grained capability
/// bounds (`S: SutEditorMirrorWrite`, …) while the enum still dispatches
/// `S: SutHandle` — `SutHandle` is (becoming) a supertrait bundle of the
/// fine caps.
///
/// Non-`Send` futures: `E2ESut<V>` holds `RefCell` fields (interior
/// mutability for the reactive engine) and can't be `Sync`. Native
/// `async fn` futures are `?Send` by default, which matches — proptest's
/// state-machine driver runs single-threaded via `runtime.block_on`.
///
/// Implemented for `E2ESut<V>` in `sut_handle.rs`. The trait grows
/// methods as variants are migrated; each migration that needs a new SUT
/// capability adds one method here and one impl in `sut_handle.rs`
/// (delegating to existing helpers).
#[allow(async_fn_in_trait)]
pub trait SutHandle:
    ::holon_pbt_core::capabilities::SutEditorMirrorWrite
    + ::holon_pbt_core::capabilities::SutBlockTreeWrite
{
    /// Shared-PBT capability: synthesize a click on a UI element by
    /// its bounds-registry id. Used by `holon_layout_testing`'s shared
    /// `apply_to_sut` bodies (`SwitchViewMode`, `ToggleDrawer`,
    /// `ToggleCollapse`) via `SutClickAdapter`. Default impl panics
    /// loud — concrete SUTs override with their click pipeline (e.g.
    /// `GpuiUserDriver::click_entity`).
    async fn apply_click_at_element(&mut self, element_id: &str) {
        let _ = element_id;
        unimplemented!(
            "SutHandle::apply_click_at_element — the concrete SUT for this PBT \
             variant hasn't been migrated to support shared layout interactions yet."
        )
    }

    /// Shared-PBT capability: push deferred `live_block` content
    /// through the SUT's data-arrival pipeline. Mirror of the layout
    /// PBT's `LiveBlockSink::deliver_block_content_loaded`. Default
    /// panics — real backend tests should override or skip
    /// `DeliverBlockContent` from their generator.
    async fn apply_deliver_block_content_loaded(&mut self, block_id: &str) {
        let _ = block_id;
        unimplemented!(
            "SutHandle::apply_deliver_block_content_loaded — the integration-tests \
             PBT runs a real backend; deferred block delivery isn't a meaningful \
             stimulus there. Skip `DeliverBlockContent` in the generator."
        )
    }

    /// Pilot-variant capability: drive the navigation back-button via
    /// `TestContext::navigate_back` and dump nav tables for tracing.
    async fn navigate_back(&mut self, region: holon_api::Region);

    /// Pre-startup: write an org file to the temp directory.
    async fn apply_write_org_file(&mut self, filename: &str, content: &str);

    /// Pre-startup: create a directory.
    async fn apply_create_directory(&mut self, path: &str);

    /// Pre-startup: initialize git repository.
    async fn apply_git_init(&mut self);

    /// Pre-startup: initialize jj repository.
    async fn apply_jj_git_init(&mut self);

    /// Pre-startup: create a stale/corrupted .loro file.
    async fn apply_create_stale_loro(
        &mut self,
        org_filename: &str,
        corruption_type: crate::LoroCorruptionType,
    );

    /// Start the application.
    async fn apply_start_app(
        &mut self,
        ref_state: &ReferenceState,
        wait_for_ready: bool,
        enable_todoist: bool,
        enable_loro: bool,
    );

    /// Navigate to focus on a specific block within a region.
    async fn apply_navigate_focus(
        &mut self,
        region: holon_api::Region,
        block_id: &holon_api::EntityUri,
    );

    /// Navigate forward in the per-region navigation history.
    async fn apply_navigate_forward(&mut self, region: holon_api::Region);

    /// Navigate home (return to root) in a region.
    async fn apply_navigate_home(&mut self, region: holon_api::Region);

    /// Simulate app restart: clears last_projection and triggers re-sync.
    async fn apply_simulate_restart(&mut self, ref_state: &ReferenceState);

    /// Post-startup: create a new document and record the UUID mapping.
    async fn apply_create_document(&mut self, file_name: &str, ref_state: &ReferenceState);

    /// Post-startup: remove an active query watch.
    async fn apply_remove_watch(&mut self, query_id: &str);

    /// Post-startup: switch the current view.
    async fn apply_switch_view(&mut self, view_name: &str);

    /// Post-startup: run sequential query_and_watch operations to test for schema-lock bugs.
    async fn apply_concurrent_schema_init(&mut self);

    /// Post-startup: set up a query watch.
    async fn apply_setup_watch(
        &mut self,
        query_id: &str,
        query: &crate::pbt::query::TestQuery,
        language: holon_api::QueryLanguage,
    );

    /// Post-startup: toggle a block's task state via the StateToggle widget path.
    async fn apply_toggle_state(&mut self, block_id: &holon_api::EntityUri, new_state: &str);

    /// Post-startup: bulk add blocks to a document by writing an org file.
    async fn apply_bulk_external_add(
        &mut self,
        doc_uri: &holon_api::EntityUri,
        blocks: &[holon_api::block::Block],
        ref_state: &ReferenceState,
    );

    /// Post-startup: concurrent UI + external mutations.
    async fn apply_concurrent_mutations(
        &mut self,
        ui_mutation: crate::pbt::types::MutationEvent,
        external_mutation: crate::pbt::types::MutationEvent,
        ref_state: &ReferenceState,
    );

    /// Post-startup: apply a single UI or external mutation.
    async fn apply_apply_mutation(
        &mut self,
        event: crate::pbt::types::MutationEvent,
        ref_state: &ReferenceState,
    );

    /// Post-startup: trigger the "/" slash-command menu on a block and select "delete".
    async fn apply_trigger_slash_command(&mut self, block_id: &holon_api::EntityUri);

    // `apply_indent` / `apply_outdent` / `apply_move_up` / `apply_move_down`
    // / `apply_split_block` / `apply_join_block` now come from the
    // `SutBlockTreeWrite` supertrait (pure ACTIONS, no `ref_state`), so the
    // block-tree transitions narrow to `S: SutBlockTreeWrite`. Their
    // `ref_state`-dependent post-action (sync barrier, block-count check,
    // synthetic-id reconciliation) lives in
    // `E2ESut::block_tree_post_action`, run by the harness after
    // `apply_to_sut`.

    /// Post-startup: drag the source block onto the target, re-parenting source as
    /// a child of the target.
    async fn apply_drag_drop_block(
        &mut self,
        source: &holon_api::EntityUri,
        target: &holon_api::EntityUri,
    );

    /// Post-startup: click on a rendered block to focus it.
    async fn apply_click_block(
        &mut self,
        region: holon_api::Region,
        block_id: &holon_api::EntityUri,
    );

    /// Post-startup: undo the last UI mutation.
    async fn apply_undo_last_mutation(&mut self, ref_state: &ReferenceState);

    /// Post-startup: redo the last undone mutation.
    async fn apply_redo(&mut self, ref_state: &ReferenceState);

    /// Post-startup: trigger IVM re-evaluation.
    async fn apply_emit_mcp_data(&mut self);

    /// Post-startup: focus an editable text block (atomic editor primitive).
    async fn apply_focus_editable_text(&mut self, block_id: &holon_api::EntityUri);

    // `apply_move_cursor` / `apply_type_chars` / `apply_delete_backward`
    // now come from the `SutEditorMirrorWrite` supertrait, so `TypeChars`
    // / `MoveCursor` / `DeleteBackward` can narrow to `S:
    // SutEditorMirrorWrite` and run on any SUT supplying that cap.

    /// Post-startup: press a structural key chord in the active editor.
    /// Takes `ref_state` so Enter (which dispatches `split_block`) can
    /// map the freshly-minted prod UUID back to the synthetic
    /// `block::split-N` slot the ref-state allocated.
    async fn apply_press_key(&mut self, chord: &holon_api::KeyChord, ref_state: &ReferenceState);

    /// Post-startup: arrow-key navigation from the focused block.
    async fn apply_arrow_navigate(
        &mut self,
        region: holon_api::Region,
        direction: holon_frontend::navigation::NavDirection,
        steps: u8,
        ref_state: &ReferenceState,
    );

    /// Post-startup: add a Loro-only peer instance sharing the primary's state.
    async fn apply_add_peer(&mut self);

    /// Post-startup: edit a block on a peer's LoroDoc directly.
    async fn apply_peer_edit(&mut self, peer_idx: usize, op: &crate::pbt::transitions::PeerEditOp);

    /// Post-startup: bidirectional sync between primary's LoroDoc and a peer.
    async fn apply_sync_with_peer(&mut self, peer_idx: usize);

    /// Post-startup: one-directional merge — peer's changes into primary.
    async fn apply_merge_from_peer(&mut self, peer_idx: usize);

    /// Post-startup: edit a block's LoroText container on a peer at character level.
    async fn apply_peer_char_edit(
        &mut self,
        peer_idx: usize,
        block_id: &str,
        op: &crate::pbt::transitions::TextOp,
    );

    /// Post-startup: pin a block to a sidebar (`focus_pin` op — LogSeq-style
    /// shift+click). No leader chord exists for this; the headless PBT
    /// dispatches the navigation op directly.
    async fn apply_pin_block(&mut self, region: holon_api::Region, block_id: &holon_api::EntityUri);

    /// Post-startup: close one open `navigation_history` row by id (`close`
    /// op — sidebar X button). Region-less.
    async fn apply_unpin_block(&mut self, history_id: i64);

    /// Post-startup: flip an `expand_toggle`'s `expanded` Mutable from
    /// false to true. Production binding is a chevron click; there is no
    /// backend operation — it's a frontend-state flip that drives
    /// `LazyReactiveSlot::materialize_if_gated` on the next render. The
    /// `E2ESut` impl walks the engine's reactive tree, finds the
    /// `expand_toggle` node whose `target_id` prop matches `block_id`,
    /// and calls `.expanded.set(true)`. Fails loud if the corpus grows a
    /// toggle render but the engine produces no matching node.
    async fn apply_expand_toggle(&mut self, block_id: &holon_api::EntityUri);

    /// Inverse of `apply_expand_toggle` — sets `.expanded` to false.
    /// Same walker; the `LazyReactiveSlot` cache is preserved by design
    /// so re-expand is instant. See
    /// `devlog/2026-05-15-lazy-expand-toggle-plan.md`.
    async fn apply_collapse_toggle(&mut self, block_id: &holon_api::EntityUri);
}

/// `declare_e2e_transitions!` — the only central code in the file-
/// per-transition pattern.
///
/// Wraps `declarative_enum_dispatch::enum_dispatch!` (which generates
/// the trait, the enum, and the trait-for-enum dispatch impl using
/// only `macro_rules!` — no proc macros) and adds the proptest-state-
/// machine strategy aggregator.
///
/// Adding a transition = create the file + add one line to the call.
#[macro_export]
macro_rules! declare_e2e_transitions {
    (
        $vis:vis enum $enum_name:ident {
            $($variant:ident($ty:ty)),* $(,)?
        }
    ) => {
        #[derive(Clone, Debug, ::serde::Serialize, ::serde::Deserialize)]
        $vis enum $enum_name {
            $( $variant($ty) ),*
        }

        $(
            impl ::core::convert::From<$ty> for $enum_name {
                fn from(v: $ty) -> Self { $enum_name::$variant(v) }
            }
        )*

        // Dispatch the FOREIGN generic behaviour trait onto the enum by a
        // hand-rolled match (replaces `declarative_enum_dispatch`, which can
        // only define+dispatch its own local trait, not a foreign generic
        // one). Concrete-`S` dispatch: generic over `S: SutHandle` rather
        // than `dyn SutHandle`, so the SUT is monomorphised (the runner
        // passes `&mut E2ESut<V>`). This is what lets each variant's
        // `TransitionImpl<R, S>` impl narrow `S` to fine-grained capability
        // bounds — `SutHandle` is a supertrait bundle of those caps, so a
        // variant bound on `S: SutEditorMirrorWrite` still satisfies this
        // enum impl's `S: SutHandle`.
        // Ref-side dispatch (S-independent): preconditions + apply_to_ref.
        impl ::holon_pbt_core::TransitionRef<$crate::pbt::reference_state::ReferenceState>
            for $enum_name {
            type Reason = $crate::pbt::validation::Reason;

            fn preconditions(
                &self,
                state: &$crate::pbt::reference_state::ReferenceState,
            ) -> ::validated::Validated<(), $crate::pbt::validation::Reason> {
                match self {
                    $( $enum_name::$variant(v) => <$ty as ::holon_pbt_core::TransitionRef<
                        $crate::pbt::reference_state::ReferenceState,
                    >>::preconditions(v, state), )*
                }
            }

            fn apply_to_ref(&self, state: &mut $crate::pbt::reference_state::ReferenceState) {
                match self {
                    $( $enum_name::$variant(v) => <$ty as ::holon_pbt_core::TransitionRef<
                        $crate::pbt::reference_state::ReferenceState,
                    >>::apply_to_ref(v, state), )*
                }
            }
        }

        // SUT-side dispatch (concrete-`S`): apply_to_sut. Generic over
        // `S: SutHandle`; variants may narrow `S` to fine-grained caps —
        // `SutHandle` is a supertrait bundle, so they still satisfy this.
        #[allow(async_fn_in_trait)]
        impl<S: $crate::pbt::transition_dispatch::SutHandle>
            ::holon_pbt_core::TransitionImpl<
                $crate::pbt::reference_state::ReferenceState,
                S,
            > for $enum_name {
            async fn apply_to_sut(
                &self,
                state: &$crate::pbt::reference_state::ReferenceState,
                sut: &mut S,
            ) {
                match self {
                    $( $enum_name::$variant(v) => <$ty as ::holon_pbt_core::TransitionImpl<
                        $crate::pbt::reference_state::ReferenceState,
                        S,
                    >>::apply_to_sut(v, state, sut).await, )*
                }
            }
        }

        #[cfg(feature = "otel-testing")]
        impl $crate::pbt::transition_budgets::SqlBudget for $enum_name {
            fn expected_sql(
                &self,
                state: &$crate::pbt::reference_state::ReferenceState,
            ) -> $crate::pbt::transition_budgets::ExpectedSql {
                match self {
                    $( $enum_name::$variant(v) =>
                        <$ty as $crate::pbt::transition_budgets::SqlBudget>::expected_sql(v, state), )*
                }
            }
        }

        impl $enum_name {
            /// Variant name for trace logging and Markov-weighting in
            /// generators that bias on the previous transition kind.
            pub fn variant_name(&self) -> &'static str {
                match self {
                    $( Self::$variant(_) => stringify!($variant), )*
                }
            }
        }

        $vis fn aggregate_transitions(
            state: &$crate::pbt::reference_state::ReferenceState,
        ) -> ::proptest::strategy::BoxedStrategy<$enum_name> {
            use ::proptest::strategy::{Strategy, Union};
            use $crate::pbt::transition_dispatch::variant_weight_multiplier;

            let mut arms: Vec<(u32, ::proptest::strategy::BoxedStrategy<$enum_name>)> = Vec::new();

            $(
                match <$ty as ::holon_pbt_core::TransitionFactory<
                    $crate::pbt::reference_state::ReferenceState,
                >>::weighted_generator(state) {
                    ::validated::Validated::Good((w, s)) => {
                        // Apply the per-variant `HOLON_PBT_WEIGHTS` multiplier
                        // here so individual transitions don't have to wire it.
                        // `0` is a legal multiplier — it removes the variant
                        // from the strategy; `Union::new_weighted` ignores
                        // zero-weight arms.
                        let multiplier = variant_weight_multiplier(stringify!($variant));
                        let final_weight = w.saturating_mul(multiplier);
                        if final_weight > 0 {
                            arms.push((final_weight, s.prop_map($enum_name::from).boxed()));
                        }
                    }
                    ::validated::Validated::Fail(reasons) => {
                        $crate::pbt::validation::record_rejection(
                            stringify!($variant),
                            &reasons,
                        );
                    }
                }
            )*

            assert!(
                !arms.is_empty(),
                "declare_e2e_transitions!: no transition applicable in state {state:?} \
                 — at least one variant must be unconditionally enabled."
            );
            Union::new_weighted(arms).boxed()
        }
    };
}
