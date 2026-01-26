//! Reference state machine: `VariantRef` wrapper and `ReferenceStateMachine` impl.
//!
//! This contains the transition generation, preconditions, and reference model
//! application logic for the property-based test.

use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::Arc;

use fluxdi::{Injector, Provider, Shared};
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::{ContentType, Region, SourceLanguage, TaskState, Value};

use holon_orgmode::models::OrgBlockExt;

use crate::assign_reference_sequences_canonical;

use super::generators::*;
use super::query::WatchSpec;
use super::reference_state::{
    CursorPosition, NavigationHistory, ReferenceState, ShadowInterpreter,
};
use super::transitions::E2ETransition;
use super::types::*;
use crate::LoroCorruptionType;

use loro::{ExportMode, LoroDoc};

/// Whether the PBT generator may produce mutations that overwrite the root
/// layout (render-source content, layout headline content). The original
/// disable was to keep the `set_data`-doesn't-propagate-to-children bug
/// reproducible (layout mutations could swap out `state_toggle`, hiding it).
/// That bug is now fixed (ReactiveRowSet single-writer + ReadOnlyMutable
/// downstream + leaf signal subscriptions; see
/// `reactive_view_model::tests::shared_data_cell_updates_propagate_to_state_toggle_child`).
///
/// Custom `index.org` layouts that drop a sidebar (and layout-mutated panel
/// render sources) are handled in the generator: `ref_state.region_predictable(region)`
/// short-circuits `focusable_rendered_block_ids` so ClickBlock candidates only
/// arise for regions where ref_state can predict production's rendering.
const LAYOUT_MUTATIONS_ENABLED: bool = true;

/// Block-tree manipulation transitions (Indent, Outdent, MoveUp, MoveDown,
/// SplitBlock) drive `BlockOperations` via key chords (Tab/Shift+Tab/...).
///
/// Production wiring is in place (Apr 2026):
/// - `SqlBlockOperations` is registered as an `OperationProvider` for
///   `("block", "indent" | "outdent" | "move_up" | "move_down" | "split_block")`
///   in `event_infra_module.rs`.
/// - `render_entity.rs` attaches `ctx.operations` onto the produced
///   `ViewModel` so descendants' `bubble_input` walks find the chord on
///   their way up the focus path.
///
/// Enabled so the PBT actually generates these transitions — the failures
/// they shrink to are real production bugs (e.g. `outdent` raising "Parent
/// not found" because `BlockEntity::parent_id()` strips the `block:` scheme
/// prefix while the SQL `id` column stores the prefixed form).
const BLOCK_TREE_KEYCHORD_OPS_ENABLED: bool = true;

/// Gate for `DragDropBlock` transition generation.
///
/// Wiring:
/// - `assets/default/types/block_profile.yaml` `editing` and `default`
///   variants wrap each block in `column(row(draggable(icon), …), drop_zone())`.
/// - `ViewKind::DropZone { op_name }` carries the dispatched op declaratively.
/// - `UserDriver::drop_entity` overrides drive headless (shadow tree walk
///   via `HeadlessInputRouter::block_contents`) and GPUI (real
///   `MouseDown` → `MouseMove(pressed=Left)` → `MouseUp` events).
///
/// Wiring complete (Apr 2026): block_profile draggable/drop_zone widgets
/// now bind their `data` to the current row so `row_id()` returns the
/// block's id. Headless `drop_entity` polls `block_contents` for both
/// widgets to appear and bootstraps the router on first call. inv16 is
/// a hard panic if any focus-tree text block lacks a Draggable wrapper.
const DRAG_DROP_ENABLED: bool = true;

/// Builder for weighted proptest strategies with env-var overrides.
///
/// Each strategy gets a label. The default weight is 1. At build time,
/// `PBT_WEIGHT_<LABEL>` env vars override weights (0 disables the strategy).
/// This lets you focus a test run on specific transitions:
///
/// ```sh
/// # Only generate ToggleState and render source mutations:
/// PBT_WEIGHT_TOGGLE_STATE=10 PBT_WEIGHT_RENDER_SOURCE_MUTATION=10 \
///   PBT_WEIGHT_DEFAULT=0 cargo test general_e2e_pbt
///
/// # Boost ToggleState without disabling others:
/// PBT_WEIGHT_TOGGLE_STATE=20 cargo test general_e2e_pbt
/// ```
///
/// `PBT_WEIGHT_DEFAULT` overrides the baseline for all strategies that don't
/// have a specific `PBT_WEIGHT_<LABEL>` set. Defaults to 1 if unset.
struct WeightedStrategies<T: std::fmt::Debug> {
    entries: Vec<(String, u32, BoxedStrategy<T>)>,
}

impl<T: std::fmt::Debug + 'static> WeightedStrategies<T> {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add a strategy with default weight (1, or PBT_WEIGHT_DEFAULT).
    fn add(&mut self, label: &str, strategy: BoxedStrategy<T>) {
        self.entries.push((label.to_string(), 1, strategy));
    }

    /// Add a strategy with a specific default weight.
    fn add_weighted(&mut self, label: &str, weight: u32, strategy: BoxedStrategy<T>) {
        self.entries.push((label.to_string(), weight, strategy));
    }

    /// Resolve env-var overrides and build a weighted union.
    fn build(self) -> BoxedStrategy<T> {
        let default_weight: Option<u32> = std::env::var("PBT_WEIGHT_DEFAULT")
            .ok()
            .and_then(|v| v.parse().ok());

        let weighted: Vec<(u32, BoxedStrategy<T>)> = self
            .entries
            .into_iter()
            .filter_map(|(label, base_weight, strategy)| {
                let env_key = format!("PBT_WEIGHT_{}", label.to_uppercase());
                let weight = std::env::var(&env_key)
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or_else(|| default_weight.unwrap_or(base_weight));
                if weight == 0 {
                    None
                } else {
                    Some((weight, strategy))
                }
            })
            .collect();

        assert!(
            !weighted.is_empty(),
            "All PBT strategies have weight 0 — check PBT_WEIGHT_* env vars"
        );
        prop::strategy::Union::new_weighted(weighted).boxed()
    }
}

/// Simulate Loro CRDT merge for concurrent text updates.
///
/// Creates two Loro peers from a common ancestor, applies one update on each,
/// then merges them. Returns the CRDT-merged content.
fn loro_merge_text(original: &str, update_a: &str, update_b: &str) -> String {
    let ancestor = LoroDoc::new();
    ancestor.set_peer_id(0).unwrap();
    let text = ancestor.get_text("content");
    text.update(original, Default::default()).unwrap();
    ancestor.commit();
    let snapshot = ancestor.export(ExportMode::Snapshot).unwrap();

    let peer_a = LoroDoc::new();
    peer_a.set_peer_id(1).unwrap();
    peer_a.import(&snapshot).unwrap();
    peer_a
        .get_text("content")
        .update(update_a, Default::default())
        .unwrap();
    peer_a.commit();

    let peer_b = LoroDoc::new();
    peer_b.set_peer_id(2).unwrap();
    peer_b.import(&snapshot).unwrap();
    peer_b
        .get_text("content")
        .update(update_b, Default::default())
        .unwrap();
    peer_b.commit();

    let b_updates = peer_b.export(ExportMode::all_updates()).unwrap();
    peer_a.import(&b_updates).unwrap();

    peer_a.get_text("content").to_string()
}

#[derive(Debug, Clone)]
pub struct VariantRef<V: VariantMarker>(pub ReferenceState, PhantomData<V>);

impl<V: VariantMarker> std::ops::Deref for VariantRef<V> {
    type Target = ReferenceState;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<V: VariantMarker> std::ops::DerefMut for VariantRef<V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Reset a peer's baseline tracking to its current `blocks` snapshot.
///
/// The baseline drives `merge_peer_blocks_into_primary`'s concurrent-edit
/// detection: a primary block whose content differs from the baseline (and
/// whose stable ID is in the peer's `modified_stable_ids`) signals that
/// both sides edited the same text, so we run a Loro CRDT merge instead of
/// naive last-writer-wins.
fn refresh_peer_baseline(peer: &mut super::reference_state::PeerRefState) {
    peer.baseline_contents = peer
        .blocks
        .values()
        .map(|pb| (pb.stable_id.clone(), pb.content.clone()))
        .collect();
}

/// Merge peer blocks into the primary's block state.
///
/// - Blocks the peer created since the last sync (tracked in
///   `created_stable_ids`) are added to the primary.
/// - Existing blocks that the peer explicitly modified (tracked in
///   `modified_stable_ids`) get their content updated. If the primary
///   *also* edited that content since the baseline (`AddPeer` / last sync),
///   we run Loro's text CRDT merge instead of naive LWW — RGA keeps both
///   concurrent insertions.
/// - Inherited-at-AddPeer blocks the primary may have since deleted are
///   NOT re-added — Loro's CRDT keeps primary-side deletes.
fn merge_peer_blocks_into_primary(
    block_state: &mut super::reference_state::BlockState,
    peer_blocks: &[super::peer_ops::PeerBlock],
    modified_stable_ids: &std::collections::HashSet<String>,
    created_stable_ids: &std::collections::HashSet<String>,
    baseline_contents: &HashMap<String, String>,
) {
    for pb in peer_blocks {
        let block_uri = EntityUri::block(&pb.stable_id);
        if let Some(existing) = block_state.blocks.get_mut(&block_uri) {
            if modified_stable_ids.contains(&pb.stable_id) {
                let baseline = baseline_contents
                    .get(&pb.stable_id)
                    .cloned()
                    .unwrap_or_default();
                existing.content = if existing.content != baseline {
                    // Primary diverged too — Loro's text CRDT keeps both
                    // concurrent insertions. Compute the merged result.
                    loro_merge_text(&baseline, &existing.content, &pb.content)
                } else {
                    pb.content.clone()
                };
            }
            continue;
        }
        // Only re-add blocks the peer explicitly created since the last
        // sync; inherited blocks the primary deleted stay deleted.
        if !created_stable_ids.contains(&pb.stable_id) {
            continue;
        }
        let parent_uri = pb
            .parent_stable_id
            .as_deref()
            .map(EntityUri::block)
            .unwrap_or_else(EntityUri::no_parent);
        let mut block = Block::from_block_content(
            block_uri.clone(),
            parent_uri.clone(),
            holon_api::BlockContent::text(pb.content.clone()),
        );
        block.created_at = 0;
        block.updated_at = 0;
        block_state.blocks.insert(block_uri.clone(), block);
        block_state.block_documents.insert(block_uri, parent_uri);
    }
}

impl<V: VariantMarker> ReferenceStateMachine for VariantRef<V> {
    type State = Self;
    type Transition = E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        let injector = Injector::root();
        let interp = Shared::new(holon_frontend::shadow_builders::build_shadow_interpreter());
        injector.provide::<ShadowInterpreter>(Provider::root({
            let s = interp;
            move |_| s.clone()
        }));
        let interpreter: Arc<ShadowInterpreter> = injector.resolve::<ShadowInterpreter>();

        prop_oneof![
            // ~50% with keyword set (exercises task_state mutations)
            1 => {
                let interp = interpreter.clone();
                todo_keyword_set_strategy().prop_map(move |ks| {
                    let mut state = ReferenceState::new(V::variant(), interp.clone());
                    state.keyword_set = Some(ks);
                    VariantRef(state, PhantomData)
                })
            },
            // ~50% without (exercises no-keyword path)
            1 => {
                let interp = interpreter.clone();
                Just(VariantRef(
                    ReferenceState::new(V::variant(), interp),
                    PhantomData,
                ))
            },
        ]
        .boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        // Pre-startup phase: generate various pre-startup transitions
        if !state.app_started {
            let pre_startup_file_count = state.documents.len();
            let dir_count = state.pre_startup_directories.len();

            // Weight towards starting app after some setup
            let file_weight = if pre_startup_file_count < 3 { 3 } else { 1 };
            let dir_weight = if dir_count < 10 { 2 } else { 0 }; // Create up to ~10 directories
            let vcs_weight = if !state.git_initialized && !state.jj_initialized {
                1
            } else {
                0
            };

            // Build strategy list dynamically based on state
            let mut strategies: Vec<(u32, BoxedStrategy<E2ETransition>)> = vec![
                (
                    file_weight,
                    generate_org_file_content_with_keywords(
                        state.keyword_set.clone(),
                        LAYOUT_MUTATIONS_ENABLED,
                    )
                    .prop_map(|(filename, content)| E2ETransition::WriteOrgFile {
                        filename,
                        content,
                    })
                    .boxed(),
                ),
                (
                    2,
                    // Always wait for OrgSyncController to finish initial file processing.
                    // Without this, UI mutations race with OrgSyncController's initial processing,
                    // creating duplicate Loro documents for the same file path (one from
                    // the mutation, one from OrgSyncController), causing block deletion on sync.
                    Just(E2ETransition::StartApp {
                        wait_for_ready: true,
                        enable_todoist: true,
                        enable_loro: state.variant.enable_loro,
                    })
                    .boxed(),
                ),
            ];

            if dir_weight > 0 {
                strategies.push((
                    dir_weight,
                    generate_directory_path()
                        .prop_map(|path| E2ETransition::CreateDirectory { path })
                        .boxed(),
                ));
            }

            if vcs_weight > 0 && !state.git_initialized {
                strategies.push((vcs_weight, Just(E2ETransition::GitInit).boxed()));
            }

            if vcs_weight > 0 && !state.jj_initialized {
                strategies.push((vcs_weight, Just(E2ETransition::JjGitInit).boxed()));
            }

            // Add CreateStaleLoro transition if there are org files to corrupt (Loro only)
            let org_filenames: Vec<String> = state.documents.values().cloned().collect();
            if state.variant.enable_loro && !org_filenames.is_empty() {
                strategies.push((
                    1, // Lower weight - only occasionally create stale loro files
                    (
                        prop::sample::select(org_filenames),
                        prop::sample::select(vec![
                            LoroCorruptionType::Empty,
                            LoroCorruptionType::Truncated,
                            LoroCorruptionType::InvalidHeader,
                        ]),
                    )
                        .prop_map(|(org_filename, corruption_type)| {
                            E2ETransition::CreateStaleLoro {
                                org_filename,
                                corruption_type,
                            }
                        })
                        .boxed(),
                ));
            }

            return prop::strategy::Union::new_weighted(strategies).boxed();
        }

        // Post-startup phase: generate normal transitions
        // Exclude seeded default layout blocks (doc:__default__) from mutation targets —
        // these blocks have no org file on disk so external mutations can't write to them.
        let default_doc = EntityUri::no_parent();
        // Blocks that any peer has modified since the last sync. Loro merges
        // concurrent text edits character-wise, but the reference model uses
        // overwrite semantics; excluding peer-modified blocks from primary
        // mutation targets avoids unrepresentable concurrent-edit divergence.
        let peer_modified: HashSet<String> = state
            .peers
            .iter()
            .flat_map(|p| p.modified_stable_ids.iter().cloned())
            .collect();
        let is_peer_modified = |id: &EntityUri| peer_modified.contains(id.id());
        let block_ids: Vec<EntityUri> = state
            .block_state
            .blocks
            .iter()
            .filter(|(_, b)| {
                // Exclude seed blocks (belong to default doc) and document blocks
                !b.is_page()
                    && !is_peer_modified(&b.id)
                    && state
                        .block_state
                        .block_documents
                        .get(&b.id)
                        .is_none_or(|doc| *doc != default_doc)
            })
            .map(|(id, _)| id.clone())
            .collect();
        let text_block_ids: Vec<EntityUri> = state
            .block_state
            .blocks
            .iter()
            .filter(|(_, b)| {
                b.content_type == ContentType::Text
                    && !b.is_page()
                    && !is_peer_modified(&b.id)
                    && state
                        .block_state
                        .block_documents
                        .get(&b.id)
                        .is_none_or(|doc| *doc != default_doc)
            })
            .map(|(id, _)| id.clone())
            .collect();
        let doc_uris: Vec<EntityUri> = state.documents.keys().cloned().collect();
        let next_id = state.block_state.next_id;
        let next_doc_id = state.next_doc_id;

        let mut strategies = WeightedStrategies::new();

        strategies.add(
            "create_document",
            Just(E2ETransition::CreateDocument {
                file_name: format!("doc_{}.org", next_doc_id),
            })
            .boxed(),
        );

        // Render + query source blocks have dedicated mutation strategies;
        // exclude them from the generic content-update path so random text
        // doesn't corrupt render DSL or break initial_widget().
        let no_content_update: HashSet<EntityUri> = state
            .layout_blocks
            .render_source_ids
            .iter()
            .chain(state.layout_blocks.query_source_ids.iter())
            .chain(state.profile_block_ids.iter())
            .cloned()
            .collect();

        if !doc_uris.is_empty() {
            // ui_mutation is a synthetic-fuzz transition: it dispatches
            // backend ops (create with arbitrary id, delete, custom-prop
            // updates) that real users can't trigger via the GPUI editor
            // today (no delete affordance, no pref_field outside the
            // Preferences panel, no direct-create gesture). Useful for
            // exhibiting backend bugs, but mislabelled as UI-sourced.
            // Default weight 0 so the PBT doesn't exercise faux-UI
            // gestures by accident; opt in with PBT_WEIGHT_UI_MUTATION=N.
            // Eventual cleanup: rename the source to MutationSource::Backend
            // (or split layout_headline / render_source / profile genuine
            // content-edit UI from arbitrary-shape mutations).
            strategies.add_weighted(
                "ui_mutation",
                0,
                generate_mutation(
                    next_id,
                    block_ids.clone(),
                    text_block_ids.clone(),
                    doc_uris.clone(),
                    no_content_update.clone(),
                )
                .prop_map(|mutation| {
                    E2ETransition::ApplyMutation(MutationEvent {
                        source: MutationSource::UI,
                        mutation,
                    })
                })
                .boxed(),
            );

            strategies.add(
                "external_mutation",
                generate_mutation(
                    next_id,
                    block_ids.clone(),
                    text_block_ids.clone(),
                    doc_uris.clone(),
                    no_content_update.clone(),
                )
                .prop_map(|mutation| {
                    E2ETransition::ApplyMutation(MutationEvent {
                        source: MutationSource::External,
                        mutation,
                    })
                })
                .boxed(),
            );
        }

        strategies.add(
            "setup_watch",
            (
                generate_test_query(),
                generate_query_language(),
                "[a-z]{1,10}",
            )
                .prop_map(|(query, language, id)| E2ETransition::SetupWatch {
                    query_id: format!("query-{}", id),
                    query,
                    language,
                })
                .boxed(),
        );

        if !state.active_watches.is_empty() {
            let watch_ids: Vec<String> = state.active_watches.keys().cloned().collect();
            strategies.add(
                "remove_watch",
                prop::sample::select(watch_ids)
                    .prop_map(|query_id| E2ETransition::RemoveWatch { query_id })
                    .boxed(),
            );
        }

        strategies.add(
            "switch_view",
            prop::sample::select(vec![
                "all".to_string(),
                "sidebar".to_string(),
                "main".to_string(),
            ])
            .prop_map(|view_name| E2ETransition::SwitchView { view_name })
            .boxed(),
        );

        let regions = Region::ALL.to_vec();

        // NavigateFocus should target blocks that have focusable children,
        // otherwise the region becomes empty after navigation and ClickBlock
        // cannot fire. Includes layout headline blocks (e.g. default-main-panel)
        // because those are the typical navigation targets, even though they're
        // excluded from the mutation pool.
        //
        // EXCLUDES `root-layout`: focusing region='main' on root-layout makes
        // `focus_roots` resolve to {default-main-panel, default-left-sidebar,
        // default-right-sidebar}. The main panel's own GQL query
        // (`CHILD_OF*0..20` from focus_root) then returns `default-main-panel`
        // itself among the descendants, and the renderer's
        // block_ref(default-main-panel) recurses inside default-main-panel's
        // own snapshot — `snapshot()` detects the cycle and emits an Error
        // widget that trips inv14b. See HANDOFF_TURSO_IVM_FOCUS_ROOTS_CHURN.md.
        let parents_with_focusable_children: std::collections::HashSet<EntityUri> = state
            .block_state
            .blocks
            .values()
            .filter(|b| {
                b.content_type == ContentType::Text && state.layout_blocks.is_focusable(&b.id)
            })
            .map(|b| b.parent_id.clone())
            .collect();
        let navigable_block_ids: Vec<EntityUri> = parents_with_focusable_children
            .into_iter()
            .filter(|id| {
                !id.is_no_parent() && !id.is_sentinel() && state.block_state.blocks.contains_key(id)
            })
            .filter(|id| id.as_str() != "block:root-layout")
            .collect();
        if !navigable_block_ids.is_empty() {
            strategies.add_weighted(
                "navigate_focus",
                3,
                (
                    prop::sample::select(regions.clone()),
                    prop::sample::select(navigable_block_ids),
                )
                    .prop_map(|(region, block_id)| E2ETransition::NavigateFocus {
                        region,
                        block_id,
                    })
                    .boxed(),
            );
        }

        for region in &regions {
            if state.can_go_back(*region) {
                strategies.add(
                    "navigate_back",
                    Just(E2ETransition::NavigateBack { region: *region }).boxed(),
                );
            }
        }

        for region in &regions {
            if state.can_go_forward(*region) {
                strategies.add(
                    "navigate_forward",
                    Just(E2ETransition::NavigateForward { region: *region }).boxed(),
                );
            }
        }

        strategies.add(
            "navigate_home",
            prop::sample::select(regions.clone())
                .prop_map(|region| E2ETransition::NavigateHome { region })
                .boxed(),
        );

        // ClickBlock — click on a focusable rendered block to focus it.
        //
        // When the Main region has no current focus, bias heavily toward
        // LeftSidebar clicks. The default index.org sidebar wraps each doc in
        // a `selectable` whose bound action is `navigation.focus(region:
        // "main", block_id: col("id"))`, so clicking one populates the
        // `navigation_cursor` → `current_focus` → `focus_roots` matview chain
        // that the Main panel's GQL query depends on. Without this nudge, a
        // freshly-started app rarely produces the sidebar click that unlocks
        // every Main-region transition (ToggleState, edits, navigation), so
        // the rest of the search budget is wasted on Main-region transitions
        // that have no rendered blocks to act on.
        let main_unfocused = state.current_focus(holon_api::Region::Main).is_none();
        for region in &regions {
            // Skip RightSidebar while we stabilize the bug reproduction —
            // its default PRQL is `from children`, which depends on a focus
            // that the PBT's nav-state doesn't fully mirror in the production
            // matview chain. Clicking ends up timing out waiting for content
            // that never resolves. Re-enable once we either teach the ref
            // model to seed RightSidebar focus correctly or extend the click
            // path to handle non-clickable targets gracefully.
            if *region == holon_api::Region::RightSidebar {
                continue;
            }
            let focusable = state.focusable_rendered_block_ids(*region);
            if !focusable.is_empty() {
                let r = *region;
                let weight = if main_unfocused && *region == holon_api::Region::LeftSidebar {
                    12
                } else {
                    3
                };
                strategies.add_weighted(
                    "click_block",
                    weight,
                    prop::sample::select(focusable)
                        .prop_map(move |block_id| E2ETransition::ClickBlock {
                            region: r,
                            block_id,
                        })
                        .boxed(),
                );
            }
        }

        // ArrowNavigate — arrow-key navigation from the currently focused block
        for region in &regions {
            if state.focused_entity_id.contains_key(region) {
                use holon_frontend::navigation::NavDirection;

                // Determine available directions from navigator type
                let render_name = state.active_render_expr_name(*region);
                let directions: Vec<NavDirection> = match render_name.as_deref() {
                    Some("tree") | Some("outline") => {
                        vec![
                            NavDirection::Up,
                            NavDirection::Down,
                            NavDirection::Left,
                            NavDirection::Right,
                        ]
                    }
                    _ => vec![NavDirection::Up, NavDirection::Down],
                };

                let r = *region;
                strategies.add(
                    "arrow_navigate",
                    (prop::sample::select(directions), 1u8..=3u8)
                        .prop_map(move |(direction, steps)| E2ETransition::ArrowNavigate {
                            region: r,
                            direction,
                            steps,
                        })
                        .boxed(),
                );
            }
        }

        // Layout headline mutations (content, task_state, priority, tags).
        //
        // Gated behind `LAYOUT_MUTATIONS_ENABLED` while we stabilize the base
        // reproduction scenario — see the constant's doc comment.
        if LAYOUT_MUTATIONS_ENABLED {
            // Exclude seed layout blocks (root-layout, default-main-panel, etc).
            // These are created by seed code and never fully registered in Loro,
            // so field updates trigger "not in Loro during fields_changed" warnings
            // and the CDC watch's cached content diverges from the reference model
            // (pre-existing Loro seed-block sync bug).
            let seed_layout_block_ids: std::collections::HashSet<&str> = [
                "block:root-layout",
                "block:default-main-panel",
                "block:default-left-sidebar",
                "block:default-right-sidebar",
            ]
            .into_iter()
            .collect();
            let headline_ids: Vec<EntityUri> = state
                .layout_blocks
                .headline_ids
                .iter()
                .filter(|id| !is_peer_modified(id))
                .filter(|id| !seed_layout_block_ids.contains(id.as_str()))
                .cloned()
                .collect();
            if !headline_ids.is_empty() {
                strategies.add(
                    "layout_headline_mutation",
                    generate_layout_headline_mutation(headline_ids, state.keyword_set.clone())
                        .prop_map(|mutation| {
                            E2ETransition::ApplyMutation(MutationEvent {
                                source: MutationSource::UI,
                                mutation,
                            })
                        })
                        .boxed(),
                );
            }
        }

        // Render source mutations (change render DSL expression).
        //
        // Gated behind `LAYOUT_MUTATIONS_ENABLED` — these directly rewrite the
        // active layout's render expression and can swap `state_toggle` /
        // `editable_text` out of the main panel, hiding the bug we're trying
        // to reproduce.
        if LAYOUT_MUTATIONS_ENABLED {
            // Exclude seed render sources (e.g. `holon-app-layout::render::0`)
            // for two reasons: (1) Loro seed-block sync issue (same as seed
            // headline blocks), and (2) the sidebar/main-panel render exprs
            // are load-bearing UI infrastructure — the default sidebar render
            // is a `list()` of all documents; mutating it to e.g. a
            // `focus_chain()`-driven `columns()` empties the sidebar
            // permanently (focus_chain is empty when nothing's focused),
            // hiding all documents and trapping the test in a state where
            // subsequent transitions can't exercise anything meaningful.
            let seed_render_source_ids: std::collections::HashSet<&str> = [
                "block:holon-app-layout::render::0",
                "block:holon-app-layout::src::0",
                "block:root-layout::src::0",
                // Test-env seed (uses `block:left_sidebar::...` raw IDs,
                // which become `block:block:...` once wrapped as EntityUri).
                "block:block:left_sidebar::render::0",
                "block:block:left_sidebar::src::0",
                "block:block:right_sidebar::render::0",
                "block:block:right_sidebar::src::0",
                "block:block:main_panel::render::0",
                "block:block:main_panel::src::0",
                // Worker-env seed (`block:default-*` raw IDs).
                "block:default-left-sidebar::render::0",
                "block:default-left-sidebar::src::0",
                "block:default-right-sidebar::render::0",
                "block:default-right-sidebar::src::0",
                "block:default-main-panel::render::0",
                "block:default-main-panel::src::0",
            ]
            .into_iter()
            .collect();
            let render_ids: Vec<EntityUri> = state
                .layout_blocks
                .render_source_ids
                .iter()
                .filter(|id| !seed_render_source_ids.contains(id.as_str()))
                .cloned()
                .collect();
            if !render_ids.is_empty() {
                strategies.add(
                    "render_source_mutation",
                    generate_render_source_mutation(render_ids)
                        .prop_map(|mutation| {
                            E2ETransition::ApplyMutation(MutationEvent {
                                source: MutationSource::UI,
                                mutation,
                            })
                        })
                        .boxed(),
                );
            }
        }

        // Profile content mutations (change entity profile YAML)
        {
            let profile_ids: Vec<EntityUri> = state.profile_block_ids.iter().cloned().collect();
            if !profile_ids.is_empty() {
                strategies.add(
                    "profile_mutation",
                    generate_profile_content_mutation(profile_ids)
                        .prop_map(|mutation| {
                            E2ETransition::ApplyMutation(MutationEvent {
                                source: MutationSource::UI,
                                mutation,
                            })
                        })
                        .boxed(),
                );
            }
        }

        if !block_ids.is_empty() {
            strategies.add(
                "simulate_restart",
                Just(E2ETransition::SimulateRestart).boxed(),
            );
        }

        // BulkExternalAdd - tests sync loop by adding multiple blocks via external file
        if !doc_uris.is_empty() {
            let doc_uris_clone = doc_uris.clone();
            strategies.add(
                "bulk_external_add",
                (
                    prop::sample::select(doc_uris_clone),
                    prop::collection::vec("[a-zA-Z][a-zA-Z0-9 ]{0,20}", 3..=10),
                )
                    .prop_map(move |(doc_entity_uri, contents)| {
                        let blocks: Vec<Block> = contents
                            .into_iter()
                            .enumerate()
                            .map(|(i, content)| {
                                Block::new_text(
                                    EntityUri::block(&format!("bulk-{}-{}", next_id, i)),
                                    doc_entity_uri.clone(),
                                    content,
                                )
                            })
                            .collect();
                        E2ETransition::BulkExternalAdd {
                            doc_uri: doc_entity_uri,
                            blocks,
                        }
                    })
                    .boxed(),
            );
        }

        // ConcurrentSchemaInit - tests database lock bug from concurrent schema initialization
        if !block_ids.is_empty() && !state.active_watches.is_empty() {
            strategies.add(
                "concurrent_schema_init",
                Just(E2ETransition::ConcurrentSchemaInit).boxed(),
            );
        }

        // Edit/hierarchy/split/slash/doclink transitions require:
        // - is_properly_setup(): root layout has query sources or user index.org
        // - a focused entity (via ClickBlock/ArrowNavigate): in real UI you can only
        //   edit the block with active focus. The target for edit transitions is
        //   *always* the focused block — not an arbitrary pick. This mirrors the
        //   user flow click → type → blur.
        //
        // Note: we check focused block validity directly (text, non-source,
        // descendant-of-focus) rather than intersecting with `text_block_ids`,
        // because `text_block_ids` excludes seed layout blocks (belonging to
        // __default__) which ARE valid click/edit targets when they appear
        // as focusable_rendered children of the navigation cursor.
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let focused_in_main = state.focused_entity(holon_api::Region::Main).cloned();
        let editable_block_ids: Vec<EntityUri> =
            if state.is_properly_setup() && focused_in_main.is_some() {
                let focused = focused_in_main.as_ref().unwrap();
                let valid = state
                    .block_state
                    .blocks
                    .get(focused)
                    .is_some_and(|b| b.content_type == ContentType::Text && !b.is_page())
                    && state.layout_blocks.is_focusable(focused)
                    && !no_content_update.contains(focused)
                    && state.is_descendant_of_any(focused, &focus_roots);
                if valid { vec![focused.clone()] } else { vec![] }
            } else {
                vec![]
            };
        if !editable_block_ids.is_empty() {
            let ids = editable_block_ids.clone();
            strategies.add_weighted(
                "edit_via_display_tree",
                5,
                (prop::sample::select(ids), "[a-z ]{3,20}")
                    .prop_map(
                        |(block_id, new_content)| E2ETransition::EditViaDisplayTree {
                            block_id,
                            new_content,
                        },
                    )
                    .boxed(),
            );
        }

        if !editable_block_ids.is_empty() {
            let ids = editable_block_ids.clone();
            strategies.add_weighted(
                "edit_via_view_model",
                5,
                (prop::sample::select(ids), "[a-z ]{3,20}")
                    .prop_map(|(block_id, new_content)| E2ETransition::EditViaViewModel {
                        block_id,
                        new_content,
                    })
                    .boxed(),
            );
        }

        // ToggleState: set task_state via the StateToggle widget click path.
        // Targets any text block currently rendered in the Main panel — i.e.
        // a child of the current navigation focus root. The user can
        // Cmd+Enter on any such block, not just the focused one.
        //
        // Requires:
        // - `app_started`
        // - `current_focus(Main).is_some()` — without a navigation target
        //   the main panel is empty (no rows from `MATCH (fr:focus_root) ...`),
        //   so no state_toggle widgets render. Sidebar `ClickBlock` is the
        //   transition that establishes this focus by dispatching
        //   `navigation.focus(region: "main", block_id: <doc>)`.
        if state.app_started && state.current_focus(holon_api::Region::Main).is_some() {
            // With `LAYOUT_MUTATIONS_ENABLED = false`, no render-source
            // mutations have run, so `render_expressions` is empty and both
            // `main_panel_render_expr()` and `root_render_expr()` return None.
            // Fall back to the default render expression
            // (`columns(#{gap: 4, item_template: render_entity()})`) — the
            // shadow interpreter resolves `render_entity()` per row through
            // the entity-profile system; the default block-profile variant
            // produces `state_toggle` for blocks with task_state.
            let owned_render_expr = state
                .main_panel_render_expr()
                .or_else(|| state.root_render_expr())
                .cloned()
                .unwrap_or_else(super::reference_state::default_root_render_expr);

            // Restrict candidates to blocks actually visible in the Main
            // panel — direct children of the current focus root. Ranges over
            // text blocks only.
            let main_focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
            let visible_text_block_ids: Vec<EntityUri> = text_block_ids
                .iter()
                .filter(|id| main_focus_roots.contains(*id))
                .cloned()
                .collect();

            let rows: Vec<holon_api::widget_spec::DataRow> = visible_text_block_ids
                .iter()
                .filter_map(|id| state.block_state.blocks.get(id))
                .map(super::reference_state::block_to_data_row)
                .collect();
            let ref_state: &ReferenceState = state;
            let arc_rows: Vec<std::sync::Arc<_>> =
                rows.into_iter().map(std::sync::Arc::new).collect();
            let vm = holon_frontend::interpret_pure(&owned_render_expr, &arc_rows, ref_state);
            let toggle_block_ids: Vec<EntityUri> = vm
                .snapshot()
                .state_toggle_block_ids()
                .into_iter()
                .filter_map(|id| holon_api::EntityUri::parse(&id).ok())
                .collect();

            // Build (block_id, valid_state) pairs from the *actually rendered*
            // state_toggle states. The default block-profile render passes
            // `#{states: col("todo_states")}`, where `todo_states` is a Rhai
            // computed field that walks `document(document_id).todo_keywords`.
            // Today both `document_id` and the `document()` Rhai function are
            // missing — `todo_states` always evaluates to null, and
            // `resolve_states` (crates/holon-api/src/render_eval.rs:72) falls
            // back to the hardcoded default list. The PBT must predict what
            // the widget actually shows, not what the doc would dictate if
            // the keyword-set wiring worked. Match the production fallback
            // exactly.
            //
            // *However*: when the doc declares a custom `#+TODO:` set, the
            // org parser only recognizes those keywords. Setting `task_state`
            // to a default-list value like "DOING" then writing the org file
            // produces `* DOING ...`; on the next parse "DOING" stays as
            // content (not a recognized keyword) — divergence. So when a
            // keyword set is active we restrict the candidate states to the
            // intersection of (rendered defaults) and (doc keyword set);
            // when no custom set is active we use the full default list.
            //
            // Filter out the block's *current* task_state — picking the same
            // value is a no-op (set_field doesn't fire CDC if value is
            // unchanged), so the matview never emits UpdateAt and the live
            // tree never exercises the set_data path. Toggling to a different
            // state guarantees CDC propagation, which is what inv10h_live
            // needs to detect set_data → child propagation bugs.
            const RENDERED_DEFAULT_STATES: [&str; 4] = ["", "TODO", "DOING", "DONE"];
            let candidate_states: Vec<String> = match &state.keyword_set {
                Some(ks) => {
                    let allowed: std::collections::HashSet<String> =
                        ks.all_keywords().into_iter().collect();
                    RENDERED_DEFAULT_STATES
                        .iter()
                        .filter(|s| s.is_empty() || allowed.contains(**s))
                        .map(|s| s.to_string())
                        .collect()
                }
                None => RENDERED_DEFAULT_STATES
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
            };
            let pairs: Vec<(EntityUri, String)> = toggle_block_ids
                .iter()
                .flat_map(|id| {
                    let current_state = state
                        .block_state
                        .blocks
                        .get(id)
                        .and_then(|b| b.task_state())
                        .map(|ts| ts.keyword.to_string())
                        .unwrap_or_default();
                    let bid = id.clone();
                    candidate_states
                        .iter()
                        .filter(move |&s| s != &current_state)
                        .cloned()
                        .map(move |s| (bid.clone(), s))
                })
                .collect();

            if !pairs.is_empty() {
                strategies.add(
                    "toggle_state",
                    prop::sample::select(pairs)
                        .prop_map(|(block_id, new_state)| E2ETransition::ToggleState {
                            block_id,
                            new_state,
                        })
                        .boxed(),
                );
            }
        }

        // TriggerSlashCommand: delete the currently focused block via slash command.
        // Like EditViaViewModel, this only targets the focused block (mirrors real UI
        // flow: click → type "/delete" → execute).
        let deletable_block_ids: Vec<EntityUri> =
            if state.is_properly_setup() && focused_in_main.is_some() {
                let focused = focused_in_main.as_ref().unwrap();
                let valid = state.block_state.blocks.get(focused).is_some_and(|b| {
                    b.content_type == ContentType::Text
                        && !state.layout_blocks.contains(&b.id)
                        && !b.id.as_str().contains("default-")
                        && state.block_state.blocks.len() > 2
                        && state.is_descendant_of_any(&b.id, &focus_roots)
                });
                if valid { vec![focused.clone()] } else { vec![] }
            } else {
                vec![]
            };
        if !deletable_block_ids.is_empty() {
            strategies.add(
                "trigger_slash_command",
                prop::sample::select(deletable_block_ids)
                    .prop_map(|block_id| E2ETransition::TriggerSlashCommand { block_id })
                    .boxed(),
            );
        }

        // TriggerDocLink: pick a text block and a different block as "link target".
        // Exercises the [[ trigger → EditorController → PopupMenu → InsertText pipeline.
        if editable_block_ids.len() >= 2 {
            let ids = editable_block_ids.clone();
            let target_ids = editable_block_ids.clone();
            strategies.add(
                "trigger_doc_link",
                (prop::sample::select(ids), prop::sample::select(target_ids))
                    .prop_filter("block and target must differ", |(a, b)| a != b)
                    .prop_map(
                        |(block_id, target_block_id)| E2ETransition::TriggerDocLink {
                            block_id,
                            target_block_id,
                        },
                    )
                    .boxed(),
            );
        }

        // Block hierarchy operations via keybindings.
        // Gated by `BLOCK_TREE_KEYCHORD_OPS_ENABLED` — see the constant
        // for the rationale.
        if BLOCK_TREE_KEYCHORD_OPS_ENABLED {
            // Indent: block must have a previous sibling to become its child.
            {
                let indentable: Vec<EntityUri> = editable_block_ids
                    .iter()
                    .filter(|id| state.previous_sibling(id).is_some())
                    .cloned()
                    .collect();
                if !indentable.is_empty() {
                    strategies.add(
                        "indent",
                        prop::sample::select(indentable)
                            .prop_map(|block_id| E2ETransition::Indent { block_id })
                            .boxed(),
                    );
                }
            }
            // Outdent: block must have a grandparent (parent is not root-level).
            {
                let outdentable: Vec<EntityUri> = editable_block_ids
                    .iter()
                    .filter(|id| state.grandparent(id).is_some())
                    .cloned()
                    .collect();
                if !outdentable.is_empty() {
                    strategies.add(
                        "outdent",
                        prop::sample::select(outdentable)
                            .prop_map(|block_id| E2ETransition::Outdent { block_id })
                            .boxed(),
                    );
                }
            }
            // MoveUp: block must have a previous sibling to swap with.
            {
                let movable_up: Vec<EntityUri> = editable_block_ids
                    .iter()
                    .filter(|id| state.previous_sibling(id).is_some())
                    .cloned()
                    .collect();
                if !movable_up.is_empty() {
                    strategies.add(
                        "move_up",
                        prop::sample::select(movable_up)
                            .prop_map(|block_id| E2ETransition::MoveUp { block_id })
                            .boxed(),
                    );
                }
            }
            // MoveDown: block must have a next sibling to swap with.
            {
                let movable_down: Vec<EntityUri> = editable_block_ids
                    .iter()
                    .filter(|id| state.next_sibling(id).is_some())
                    .cloned()
                    .collect();
                if !movable_down.is_empty() {
                    strategies.add(
                        "move_down",
                        prop::sample::select(movable_down)
                            .prop_map(|block_id| E2ETransition::MoveDown { block_id })
                            .boxed(),
                    );
                }
            }

            // SplitBlock: any editable block can be split at any byte position.
            // Content is ASCII-only in the PBT (generators use [a-z ]{3,20}), so byte == char.
            if !editable_block_ids.is_empty() {
                let blocks = state.block_state.blocks.clone();
                let ids = editable_block_ids.clone();
                strategies.add(
                    "split_block",
                    prop::sample::select(ids)
                        .prop_flat_map(move |block_id| {
                            let content_len = blocks
                                .get(&block_id)
                                .map(|b| b.content_text().len())
                                .unwrap_or(0);
                            (Just(block_id), 0..=content_len)
                        })
                        .prop_map(|(block_id, position)| E2ETransition::SplitBlock {
                            block_id,
                            position,
                        })
                        .boxed(),
                );
            }

            // JoinBlock: two cases that both fire on Backspace at position 0.
            //   1. Block has a previous text sibling → merge into prev sibling.
            //   2. Block is the first child of a text parent → merge into parent.
            // Either case requires the merge target to be a text block (joining
            // into a headline / source / document has different semantics we
            // don't model). The parent target also must not be a layout
            // headline, since those host their own render expression and
            // mutating their content would corrupt the active layout.
            {
                let joinable: Vec<EntityUri> = editable_block_ids
                    .iter()
                    .filter(|id| {
                        // Case 1: prev sibling is text
                        let prev_text = state.previous_sibling(id).is_some_and(|prev| {
                            state
                                .block_state
                                .blocks
                                .get(&prev)
                                .is_some_and(|b| b.content_type == ContentType::Text)
                        });
                        if prev_text {
                            return true;
                        }
                        // Case 2: no prev sibling (first child) and parent is
                        // a non-layout text block.
                        if state.previous_sibling(id).is_some() {
                            return false;
                        }
                        let parent_id = match state.block_state.blocks.get(*id) {
                            Some(b) => b.parent_id.clone(),
                            None => return false,
                        };
                        if parent_id.is_no_parent() || parent_id.is_sentinel() {
                            return false;
                        }
                        let parent_is_text = state
                            .block_state
                            .blocks
                            .get(&parent_id)
                            .is_some_and(|b| b.content_type == ContentType::Text);
                        parent_is_text && !state.layout_blocks.contains(&parent_id)
                    })
                    .cloned()
                    .collect();
                if !joinable.is_empty() {
                    strategies.add(
                        "join_block",
                        prop::sample::select(joinable)
                            .prop_map(|block_id| E2ETransition::JoinBlock { block_id })
                            .boxed(),
                    );
                }
            }

            // DragDropBlock: drag the currently-focused block onto another
            // block so the source becomes a child of the target.
            //
            // Source must be the focused block (mirrors MoveUp/Indent
            // preconditions) — this guarantees the source is currently
            // rendered with a `Draggable` wrapper. A real user typically
            // clicks the block first to focus it, then drags. Targets are
            // drawn from text blocks elsewhere in the focus tree so a
            // `DropZone` widget definitely renders for them too.
            let drag_source: Option<EntityUri> = if !editable_block_ids.is_empty() {
                Some(editable_block_ids[0].clone())
            } else {
                None
            };
            let drag_targets: Vec<EntityUri> = drag_source
                .as_ref()
                .map(|source| {
                    text_block_ids
                        .iter()
                        .filter(|id| {
                            id != &source
                                && !state.layout_blocks.contains(id)
                                && state.is_descendant_of_any(id, &focus_roots)
                        })
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            if DRAG_DROP_ENABLED && drag_source.is_some() && !drag_targets.is_empty() {
                let block_state = state.block_state.clone();
                let source = drag_source.unwrap();
                let valid_targets: Vec<EntityUri> = drag_targets
                    .into_iter()
                    .filter(|t| {
                        // Reject cycle: target descendant of source.
                        let mut current = t.clone();
                        for _ in 0..50 {
                            let Some(b) = block_state.blocks.get(&current) else {
                                return true;
                            };
                            if b.parent_id == source {
                                return false;
                            }
                            if b.parent_id.is_no_parent() || b.parent_id.is_sentinel() {
                                return true;
                            }
                            current = b.parent_id.clone();
                        }
                        true
                    })
                    // Reject no-op: target already source's parent.
                    .filter(|t| {
                        block_state
                            .blocks
                            .get(&source)
                            .map(|b| &b.parent_id != t)
                            .unwrap_or(false)
                    })
                    .collect();
                if !valid_targets.is_empty() {
                    let source = source.clone();
                    strategies.add(
                        "drag_drop_block",
                        prop::sample::select(valid_targets)
                            .prop_map(move |target| E2ETransition::DragDropBlock {
                                source: source.clone(),
                                target,
                            })
                            .boxed(),
                    );
                }
            }
        }

        // DISABLED: ConcurrentMutations — reference model assumes External always wins (LWW),
        // but actual CRDT resolution is timing-dependent.
        if false && !doc_uris.is_empty() {
            let ui_next_id = next_id;
            let ext_next_id = next_id + 1;
            strategies.add(
                "concurrent_mutations",
                (
                    generate_mutation(
                        ui_next_id,
                        block_ids.clone(),
                        text_block_ids.clone(),
                        doc_uris.clone(),
                        no_content_update.clone(),
                    ),
                    generate_mutation(
                        ext_next_id,
                        block_ids.clone(),
                        text_block_ids.clone(),
                        doc_uris.clone(),
                        no_content_update.clone(),
                    ),
                )
                    .prop_map(|(ui_mut, ext_mut)| E2ETransition::ConcurrentMutations {
                        ui_mutation: MutationEvent {
                            source: MutationSource::UI,
                            mutation: ui_mut,
                        },
                        external_mutation: MutationEvent {
                            source: MutationSource::External,
                            mutation: ext_mut,
                        },
                    })
                    .boxed(),
            );

            if !block_ids.is_empty() {
                let blocks_snapshot: Vec<(EntityUri, String)> = block_ids
                    .iter()
                    .filter_map(|id| {
                        let block = state.block_state.blocks.get(id)?;
                        if block.content_type == ContentType::Text {
                            Some((id.clone(), block.content.clone()))
                        } else {
                            None
                        }
                    })
                    .collect();

                if !blocks_snapshot.is_empty() {
                    strategies.add(
                        "concurrent_same_block",
                        (
                            prop::sample::select(blocks_snapshot),
                            "[A-Z][a-z]{2,8}",
                            "[A-Z][a-z]{2,8}",
                        )
                            .prop_map(|((id, original), suffix, prefix)| {
                                let ui_content = format!("{} {}", original, suffix);
                                let ext_content = format!("{} {}", prefix, original);
                                E2ETransition::ConcurrentMutations {
                                    ui_mutation: MutationEvent {
                                        source: MutationSource::UI,
                                        mutation: Mutation::Update {
                                            entity: "block".to_string(),
                                            id: id.clone(),
                                            fields: [(
                                                "content".to_string(),
                                                Value::String(ui_content),
                                            )]
                                            .into_iter()
                                            .collect(),
                                        },
                                    },
                                    external_mutation: MutationEvent {
                                        source: MutationSource::External,
                                        mutation: Mutation::Update {
                                            entity: "block".to_string(),
                                            id,
                                            fields: [(
                                                "content".to_string(),
                                                Value::String(ext_content),
                                            )]
                                            .into_iter()
                                            .collect(),
                                        },
                                    },
                                }
                            })
                            .boxed(),
                    );
                }
            }
        }

        if !state.undo_stack.is_empty() {
            strategies.add_weighted("undo", 2, Just(E2ETransition::UndoLastMutation).boxed());
        }
        if !state.redo_stack.is_empty() {
            strategies.add_weighted("redo", 2, Just(E2ETransition::Redo).boxed());
        }

        // EmitMcpData: trigger IVM re-evaluation to detect CDC re-emission bugs.
        // Generated with low weight — useful after navigation transitions.
        if state.app_started {
            strategies.add_weighted("emit_mcp_data", 3, Just(E2ETransition::EmitMcpData).boxed());
        }

        // Multi-instance sync transitions (post-startup, Loro enabled).
        if state.app_started && state.variant.enable_loro {
            if state.peers.len() < 3 {
                strategies.add_weighted("add_peer", 1, Just(E2ETransition::AddPeer).boxed());
            }

            // Peer edit / sync transitions — only when peers exist
            if !state.peers.is_empty() {
                let peer_count = state.peers.len();

                // Source blocks cannot have child blocks in org syntax — the
                // OrgRenderer would re-parent any children to the source's
                // parent on the next sync, diverging from the reference model.
                // Exclude source blocks from valid create parents.
                let source_block_ids: std::collections::HashSet<String> = state
                    .block_state
                    .blocks
                    .values()
                    .filter(|b| b.content_type == ContentType::Source)
                    .map(|b| b.id.id().to_string())
                    .collect();

                // Collect stable IDs from all peers for update/delete/nested-create targets
                let all_peer_block_ids: Vec<(usize, Vec<String>)> = state
                    .peers
                    .iter()
                    .enumerate()
                    .map(|(idx, p)| (idx, p.blocks.keys().cloned().collect::<Vec<_>>()))
                    .collect();

                // Peers with blocks (for update/delete) — includes all blocks.
                let _peers_with_blocks: Vec<(usize, Vec<String>)> = all_peer_block_ids
                    .iter()
                    .filter(|(_, ids)| !ids.is_empty())
                    .cloned()
                    .collect();

                // Peers with non-source blocks (for valid create-parent candidates).
                let peers_with_nonsource_blocks: Vec<(usize, Vec<String>)> = all_peer_block_ids
                    .iter()
                    .map(|(idx, ids)| {
                        let filtered: Vec<String> = ids
                            .iter()
                            .filter(|id| !source_block_ids.contains(*id))
                            .cloned()
                            .collect();
                        (*idx, filtered)
                    })
                    .filter(|(_, ids)| !ids.is_empty())
                    .collect();

                // PeerEdit::Create — deterministic stable ID from hash of
                // (peer_idx, parent, content, seq) ensures ref model and SUT agree.
                {
                    let pc = peer_count;
                    let seq = state.block_state.next_id;
                    let pwb_for_create = peers_with_nonsource_blocks.clone();
                    let create = (0..pc, "[a-z]{4,8}")
                        .prop_flat_map(move |(peer_idx, content)| {
                            let parent = if let Some((_, ids)) =
                                pwb_for_create.iter().find(|(i, _)| *i == peer_idx)
                            {
                                proptest::option::of(proptest::sample::select(ids.clone())).boxed()
                            } else {
                                Just(None).boxed()
                            };
                            parent.prop_map(move |parent_stable_id| {
                                let sid = super::transitions::deterministic_peer_block_id(
                                    peer_idx,
                                    parent_stable_id.as_deref(),
                                    &content,
                                    seq,
                                );
                                E2ETransition::PeerEdit {
                                    peer_idx,
                                    op: super::transitions::PeerEditOp::Create {
                                        parent_stable_id,
                                        content: content.clone(),
                                        stable_id: sid,
                                    },
                                }
                            })
                        })
                        .boxed();
                    strategies.add_weighted("peer_edit_create", 1, create);
                }

                // PeerEdit::Delete is disabled: cascading-delete ref model gap.
                //
                // PeerEdit::Update — update content of an existing peer block.
                // Source blocks are excluded: peer content is random `[a-z]{4,8}`
                // which would not be valid GQL/PRQL/SQL/YAML, breaking the root
                // layout's query parse and tripping inv10.
                if !peers_with_nonsource_blocks.is_empty() {
                    let pwb = peers_with_nonsource_blocks.clone();
                    let update = proptest::sample::select(pwb)
                        .prop_flat_map(|(peer_idx, ids)| {
                            (Just(peer_idx), proptest::sample::select(ids), "[a-z]{4,8}")
                        })
                        .prop_map(|(peer_idx, stable_id, content)| E2ETransition::PeerEdit {
                            peer_idx,
                            op: super::transitions::PeerEditOp::Update { stable_id, content },
                        })
                        .boxed();
                    strategies.add_weighted("peer_edit_update", 1, update);
                }

                // SyncWithPeer
                let sync = (0..peer_count)
                    .prop_map(|peer_idx| E2ETransition::SyncWithPeer { peer_idx })
                    .boxed();
                strategies.add_weighted("sync_with_peer", 2, sync);

                // MergeFromPeer
                let merge = (0..peer_count)
                    .prop_map(|peer_idx| E2ETransition::MergeFromPeer { peer_idx })
                    .boxed();
                strategies.add_weighted("merge_from_peer", 1, merge);
            }
        }

        // ── Atomic editor primitives — gated to GPUI runs ──
        //
        // Markov-style adaptive weights: condition each primitive's weight
        // on `state.last_transition_kind`, the variant tag of the most
        // recently applied transition. This biases the generator toward
        // *natural follow-ups* without locking out unknown-unknowns:
        // every transition keeps a weight floor of ≥1 so wild-card
        // sequences still appear ~1% of steps. The split-with-pending-edit
        // bug needs `Focus → TypeChars → DeleteBackward → PressKey(Enter)`,
        // so we boost TypeChars/DeleteBackward/PressKey right after Focus
        // and TypeChars right after themselves. PressKey gets a big boost
        // after a non-empty in-memory edit.
        if ReferenceState::atomic_editor_enabled()
            && state.app_started
            && state.is_properly_setup()
            && state.current_focus(holon_api::Region::Main).is_some()
        {
            let last = state.last_transition_kind;
            let editor_active = state.active_editor.is_some();
            let pending_edit = state
                .active_editor
                .as_ref()
                .map(|e| {
                    state
                        .block_state
                        .blocks
                        .get(&e.block_id)
                        .is_some_and(|b| b.content != e.in_memory_content)
                })
                .unwrap_or(false);

            // FocusEditableText candidates: text blocks that are
            // (a) descendants of the Main region's focus root in the ref
            // model AND (b) actually rendered in the live GPUI tree
            // (read from `BoundsRegistry` via `live_geometry`). The (b)
            // gate is essential — the ref model sees blocks that the
            // live tree doesn't (CDC lag, peer-pending, inv10i ghosts),
            // and proposing those tanks the SUT click into a missing
            // element.
            let main_focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
            let live_rendered = super::live_geometry::rendered_entity_ids();
            let focus_candidates: Vec<EntityUri> = state
                .block_state
                .blocks
                .iter()
                .filter(|(id, b)| {
                    b.content_type == ContentType::Text
                        && !b.is_page()
                        && !state.layout_blocks.contains(id)
                        && state.is_descendant_of_any(id, &main_focus_roots)
                        && !no_content_update.contains(id)
                        && live_rendered
                            .as_ref()
                            .is_some_and(|s| s.contains(id.as_str()))
                })
                .map(|(id, _)| id.clone())
                .collect();

            if !editor_active && !focus_candidates.is_empty() {
                // Boost focus when nothing happened editor-wise yet, low
                // weight when we already have an open editor (prefer to
                // continue interacting with it).
                let weight = match last {
                    Some("StartApp")
                    | Some("NavigateFocus")
                    | Some("NavigateSidebar")
                    | Some("ClickBlock")
                    | Some("Blur") => 5,
                    _ => 2,
                };
                strategies.add_weighted(
                    "focus_editable_text",
                    weight,
                    prop::sample::select(focus_candidates)
                        .prop_map(|block_id| E2ETransition::FocusEditableText { block_id })
                        .boxed(),
                );
            }

            if editor_active {
                let in_memory_len = state
                    .active_editor
                    .as_ref()
                    .map(|e| e.in_memory_content.len())
                    .unwrap_or(0);

                // MoveCursor — boost right after Focus (positioning the caret).
                let mc_weight = match last {
                    Some("FocusEditableText") => 4,
                    _ => 1,
                };
                strategies.add_weighted(
                    "move_cursor",
                    mc_weight,
                    (0..=in_memory_len)
                        .prop_map(|byte_position| E2ETransition::MoveCursor { byte_position })
                        .boxed(),
                );

                // TypeChars — boost after Focus, MoveCursor, and itself.
                let tc_weight = match last {
                    Some("FocusEditableText") | Some("MoveCursor") => 6,
                    Some("TypeChars") => 4,
                    _ => 1,
                };
                strategies.add_weighted(
                    "type_chars",
                    tc_weight,
                    "[a-z]{1,4}"
                        .prop_map(|text: String| E2ETransition::TypeChars { text })
                        .boxed(),
                );

                // DeleteBackward — boost after TypeChars (typo-correction
                // pattern) and after Focus when content already has length.
                let db_weight = match last {
                    Some("TypeChars") => 5,
                    Some("FocusEditableText") if in_memory_len > 0 => 4,
                    _ => 1,
                };
                if in_memory_len > 0 {
                    let max_delete = in_memory_len.min(4);
                    strategies.add_weighted(
                        "delete_backward",
                        db_weight,
                        (1usize..=max_delete)
                            .prop_map(|count| E2ETransition::DeleteBackward { count })
                            .boxed(),
                    );
                }

                // PressKey — heavy boost after TypeChars/DeleteBackward
                // (this is the chord-after-edit class that exposes the
                // commit-then-mutate contract violation).
                let pk_weight = if pending_edit {
                    10 // pending in-memory edit + chord = the bug class
                } else {
                    match last {
                        Some("TypeChars") | Some("DeleteBackward") => 6,
                        Some("MoveCursor") => 3,
                        _ => 1,
                    }
                };
                let chord_strategy = prop_oneof![
                    // Enter (no modifier) → split_block path.
                    3 => Just(holon_api::KeyChord(
                        std::iter::once(holon_api::Key::Enter).collect()
                    )),
                    // Backspace (no modifier) — only structural at cursor=0,
                    // but the SUT issues it unconditionally and the system
                    // routes mid-line backspace to InputState. Both paths
                    // are useful coverage.
                    2 => Just(holon_api::KeyChord(
                        std::iter::once(holon_api::Key::Backspace).collect()
                    )),
                    // Escape — blur-ish; production may discard pending.
                    1 => Just(holon_api::KeyChord(
                        std::iter::once(holon_api::Key::Escape).collect()
                    )),
                ];
                strategies.add_weighted(
                    "press_key",
                    pk_weight,
                    chord_strategy
                        .prop_map(|chord| E2ETransition::PressKey { chord })
                        .boxed(),
                );

                // Blur — low constant weight (Escape covers similar ground).
                strategies.add_weighted("blur", 1, Just(E2ETransition::Blur).boxed());
            }
        }

        strategies.build()
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        // Single source of truth: `E2ETransition::precondition` is also
        // reusable from generator filters (`.prop_filter`) so a generator
        // and the proptest precondition function can never drift apart.
        // VariantRef derefs to ReferenceState.
        transition.precondition(state)
    }
    fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
        match transition {
            E2ETransition::Nothing => {}
            // Pre-startup transitions
            E2ETransition::WriteOrgFile { filename, content } => {
                use regex::Regex;

                let doc_name = std::path::Path::new(filename.as_str())
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(filename)
                    .to_string();
                let doc_uri = state
                    .doc_uri_by_name(&doc_name)
                    .unwrap_or_else(|| state.next_synthetic_doc_uri());
                state.documents.insert(doc_uri.clone(), filename.clone());

                // Remove old content blocks from this document (handles re-writing the same file)
                let old_block_ids: Vec<EntityUri> = state
                    .block_state
                    .block_documents
                    .iter()
                    .filter(|(_, uri)| **uri == doc_uri)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in &old_block_ids {
                    state.block_state.blocks.remove(id);
                    state.block_state.block_documents.remove(id);
                    state.layout_blocks.remove(id);
                }

                // Add the page block (tags ⊇ ["Page"]) for this org file.
                // In production, OrgSyncController creates this with a UUID-based ID.
                // We use a synthetic block:ref-doc-N URI; doc_uri_map translates
                // it to the real UUID during invariant comparison.
                let mut doc_block =
                    Block::new_text(doc_uri.clone(), EntityUri::no_parent(), doc_name.clone());
                doc_block.set_page(true);
                // Set todo_keywords on document block so serialize_blocks_to_org_with_doc
                // outputs the #+TODO: header. Without this, non-default keywords like
                // WAITING are not recognized on re-parse, causing content corruption.
                if let Some(ref ks) = state.keyword_set {
                    use holon_orgmode::models::OrgDocumentExt;
                    doc_block.set_todo_keywords(Some(ks.0.clone()));
                }
                state.block_state.blocks.insert(doc_uri.clone(), doc_block);
                state
                    .block_state
                    .block_documents
                    .insert(doc_uri.clone(), doc_uri.clone());

                // Parse block IDs from content and add to reference state
                // This tracks what blocks will exist after the app starts and syncs the file
                let id_regex = Regex::new(r":ID:\s*(\S+)").unwrap();
                let headline_regex = Regex::new(r"^\*+\s+(.+)$").unwrap();
                let src_begin_regex = Regex::new(r"(?i)#\+begin_src\s+(\w+)(?:\s.*)?$").unwrap();
                let src_id_regex = Regex::new(r":id\s+(\S+)").unwrap();
                let src_end_regex = Regex::new(r"(?i)#\+end_src").unwrap();

                let mut current_headline: Option<String> = None;
                let mut current_block_id: Option<EntityUri> = None;
                let mut in_source_block = false;
                let mut source_language: Option<String> = None;
                let mut source_content = String::new();
                let mut source_block_id: Option<String> = None;
                let mut source_block_index = 0;
                // Global sequence counter matching production parser (single counter
                // for all blocks in the file, regardless of parent depth)
                let mut sequence_counter: i64 = 0;

                for line in content.lines() {
                    if let Some(caps) = headline_regex.captures(line) {
                        current_headline = Some(caps.get(1).unwrap().as_str().trim().to_string());
                        source_block_index = 0; // Reset source block counter for each headline
                    } else if let Some(caps) = id_regex.captures(line) {
                        let block_id = caps.get(1).unwrap().as_str().to_string();
                        let raw_headline = current_headline.clone().unwrap_or_default();

                        // Strip task keyword from headline (matches org parser behavior).
                        // The org parser recognizes TODO/DOING/DONE (or custom keywords)
                        // at the start of a headline and stores them as task_state.
                        let known_keywords: Vec<String> = state
                            .keyword_set
                            .as_ref()
                            .map(|ks| ks.all_keywords())
                            .unwrap_or_else(|| {
                                vec!["TODO".to_string(), "DOING".to_string(), "DONE".to_string()]
                            });
                        let (content, task_keyword) = known_keywords
                            .iter()
                            .find_map(|kw| {
                                raw_headline.strip_prefix(kw.as_str()).and_then(|rest| {
                                    if rest.is_empty() || rest.starts_with(' ') {
                                        Some((rest.trim_start().to_string(), kw.clone()))
                                    } else {
                                        None
                                    }
                                })
                            })
                            .map(|(c, kw)| (c, Some(kw)))
                            .unwrap_or((raw_headline, None));

                        let block_uri = EntityUri::block(&block_id);
                        let mut block =
                            Block::new_text(block_uri.clone(), doc_uri.clone(), content);
                        if let Some(kw) = task_keyword {
                            block.set_task_state(Some(TaskState::from_keyword(&kw)));
                        }
                        block.set_sequence(sequence_counter);
                        sequence_counter += 1;
                        state
                            .block_state
                            .block_documents
                            .insert(block_uri.clone(), doc_uri.clone());
                        current_block_id = Some(block_uri.clone());
                        state.block_state.blocks.insert(block_uri, block);
                    } else if let Some(caps) = src_begin_regex.captures(line) {
                        in_source_block = true;
                        source_language = Some(caps.get(1).unwrap().as_str().to_string());
                        source_content.clear();
                        // Extract :id from header args if present
                        source_block_id = src_id_regex
                            .captures(line)
                            .map(|c| c.get(1).unwrap().as_str().to_string());
                    } else if src_end_regex.is_match(line) && in_source_block {
                        // Create source block as child of current headline block
                        if let Some(parent_key) = &current_block_id {
                            let parent_block = &state.block_state.blocks[parent_key];
                            let parent_uri = parent_block.id.clone();
                            let src_id = source_block_id.take().unwrap_or_else(|| {
                                format!("{}::src::{}", parent_uri.id(), source_block_index)
                            });
                            let src_uri = EntityUri::block(&src_id);
                            let mut src_block = Block {
                                id: src_uri.clone(),
                                parent_id: parent_uri,
                                content: source_content.trim().to_string(),
                                content_type: ContentType::Source,
                                source_language: source_language
                                    .as_ref()
                                    .map(|s| s.parse::<SourceLanguage>().unwrap()),
                                created_at: 0,
                                updated_at: 0,
                                ..Block::default()
                            };
                            // Classify layout blocks in index.org by source language
                            if filename == "index.org"
                                && let Some(sl) = src_block.source_language.as_ref()
                            {
                                if sl.as_query().is_some() {
                                    state.layout_blocks.headline_ids.insert(parent_key.clone());
                                    state.layout_blocks.query_source_ids.insert(src_uri.clone());
                                } else if matches!(sl, SourceLanguage::Render) {
                                    state.layout_blocks.headline_ids.insert(parent_key.clone());
                                    state
                                        .layout_blocks
                                        .render_source_ids
                                        .insert(src_uri.clone());
                                    if let Some(expr) =
                                        super::reference_state::render_expr_from_rhai(
                                            src_block.content.as_str(),
                                        )
                                    {
                                        state.render_expressions.insert(src_uri.clone(), expr);
                                    }
                                }
                            }
                            src_block.set_sequence(sequence_counter);
                            sequence_counter += 1;
                            state.block_state.blocks.insert(src_uri.clone(), src_block);
                            state
                                .block_state
                                .block_documents
                                .insert(src_uri, doc_uri.clone());
                            source_block_index += 1;
                        }
                        in_source_block = false;
                        source_language = None;
                        source_content.clear();
                    } else if in_source_block {
                        if !source_content.is_empty() {
                            source_content.push('\n');
                        }
                        source_content.push_str(line);
                    }
                }
                // Re-assign sequences using canonical ordering (source blocks
                // first, then by sequence, then by ID). This matches what OrgRenderer
                // will produce after the production system round-trips the file.
                let mut all_blocks: Vec<Block> =
                    state.block_state.blocks.values().cloned().collect();
                assign_reference_sequences_canonical(&mut all_blocks);
                state.block_state.blocks =
                    all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();

                state.rebuild_profile_tracking();
                state.pre_startup_file_count += 1;
            }
            E2ETransition::CreateDirectory { path } => {
                state.pre_startup_directories.push(path.clone());
            }
            E2ETransition::GitInit => {
                state.git_initialized = true;
            }
            E2ETransition::JjGitInit => {
                state.jj_initialized = true;
                state.git_initialized = true; // jj git init also creates .git
            }
            E2ETransition::CreateStaleLoro { .. } => {
                // CreateStaleLoro doesn't change reference state - the blocks from the
                // corresponding org file should still exist after startup. The system
                // should detect the corrupted .loro file and recover from the .org file.
            }
            E2ETransition::StartApp { .. } => {
                state.app_started = true;

                // The production system always seeds default layout blocks on first startup.
                // On restart, seed blocks already exist and are skipped (idempotent).
                // If OrgSync creates a real layout later, it upserts blocks with the same IDs.
                let default_doc_uri = EntityUri::no_parent();
                {
                    // Add the seed page block itself (tags ⊇ ["Page"])
                    let mut seed_doc_block = Block::new_text(
                        default_doc_uri.clone(),
                        EntityUri::no_parent(),
                        "__default__",
                    );
                    seed_doc_block.set_page(true);
                    if let Some(ref ks) = state.keyword_set {
                        use holon_orgmode::models::OrgDocumentExt;
                        seed_doc_block.set_todo_keywords(Some(ks.0.clone()));
                    }
                    state
                        .block_state
                        .blocks
                        .insert(default_doc_uri.clone(), seed_doc_block);
                    state
                        .block_state
                        .block_documents
                        .insert(default_doc_uri.clone(), default_doc_uri.clone());

                    // Seed fixed-ID document blocks from DEFAULT_ASSETS
                    for asset in holon_frontend::DEFAULT_ASSETS {
                        if let Some(doc_id) = asset.fixed_doc_id {
                            let uri = EntityUri::from_raw(doc_id);
                            let name = asset
                                .filename
                                .strip_suffix(".org")
                                .unwrap_or(asset.filename);
                            let mut block = Block::new_text(
                                uri.clone(),
                                EntityUri::no_parent(),
                                name.to_string(),
                            );
                            block.set_page(true);
                            state.block_state.blocks.insert(uri.clone(), block);
                            state.block_state.block_documents.insert(uri.clone(), uri);
                        }
                    }

                    let default_content = include_str!("../../../../assets/default/index.org");
                    let parse_result = holon_orgmode::parse_org_file(
                        std::path::Path::new("index.org"),
                        default_content,
                        &default_doc_uri,
                        std::path::Path::new(""),
                    )
                    .expect("default index.org must parse");

                    // The production code rewrites top-level parent_ids from
                    // doc:index.org to doc:__default__ (sentinel:no_parent)
                    let file_doc_uri = parse_result.document.id.clone();
                    for block in parse_result.blocks {
                        let parent_id = if block.parent_id == file_doc_uri {
                            default_doc_uri.clone()
                        } else {
                            block.parent_id.clone()
                        };
                        let mut b = block;
                        b.parent_id = parent_id;
                        let block_id = b.id.clone();
                        state
                            .block_state
                            .block_documents
                            .insert(block_id.clone(), default_doc_uri.clone());
                        // Track render expressions for default layout render source blocks
                        if b.content_type == ContentType::Source
                            && b.source_language
                                .as_ref()
                                .is_some_and(|sl| matches!(sl, SourceLanguage::Render))
                            && let Ok(expr) = state.interpreter.parse_dsl(&b.content)
                        {
                            state.render_expressions.insert(block_id.clone(), expr);
                        }
                        state.block_state.blocks.insert(block_id, b);
                    }

                    // Classify seeded default blocks into layout_blocks to protect
                    // them from PBT mutation and enable ViewModel construction.
                    let default_block_ids: Vec<EntityUri> = state
                        .block_state
                        .blocks
                        .keys()
                        .filter(|id| {
                            state
                                .block_state
                                .block_documents
                                .get(*id)
                                .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel())
                        })
                        .cloned()
                        .collect();

                    // Collect classification info before mutating state
                    enum SeedClassification {
                        Query {
                            block_id: EntityUri,
                            parent_id: EntityUri,
                        },
                        Render {
                            block_id: EntityUri,
                            parent_id: EntityUri,
                        },
                        EntityProfile {
                            parent_id: EntityUri,
                            content: String,
                        },
                    }
                    let classifications: Vec<SeedClassification> = default_block_ids
                        .iter()
                        .filter_map(|block_id| {
                            let block = &state.block_state.blocks[block_id];
                            if block.content_type != ContentType::Source {
                                return None;
                            }
                            let sl = block.source_language.as_ref()?;
                            if sl.as_query().is_some() {
                                Some(SeedClassification::Query {
                                    block_id: block_id.clone(),
                                    parent_id: block.parent_id.clone(),
                                })
                            } else if matches!(sl, SourceLanguage::Render) {
                                Some(SeedClassification::Render {
                                    block_id: block_id.clone(),
                                    parent_id: block.parent_id.clone(),
                                })
                            } else if sl.to_string() == "holon_entity_profile_yaml" {
                                Some(SeedClassification::EntityProfile {
                                    parent_id: block.parent_id.clone(),
                                    content: block.content.clone(),
                                })
                            } else {
                                None
                            }
                        })
                        .collect();

                    for class in classifications {
                        match class {
                            SeedClassification::Query {
                                block_id,
                                parent_id,
                            } => {
                                state.layout_blocks.query_source_ids.insert(block_id);
                                state.layout_blocks.headline_ids.insert(parent_id);
                            }
                            SeedClassification::Render {
                                block_id,
                                parent_id,
                            } => {
                                state.layout_blocks.render_source_ids.insert(block_id);
                                state.layout_blocks.headline_ids.insert(parent_id);
                            }
                            SeedClassification::EntityProfile { parent_id, .. } => {
                                state.layout_blocks.headline_ids.insert(parent_id);
                            }
                        }
                    }
                } // end seed block scope

                // Load the seed entity profile from the TypeRegistry's bundled
                // block_profile.yaml (not from org blocks — the seed index.org
                // doesn't contain entity profile blocks).
                let registry = holon::type_registry::create_default_registry()
                    .expect("default TypeRegistry must initialize");
                let block_type_def = registry
                    .get("block")
                    .expect("Block type must be registered");
                state.seed_profile = holon::entity_profile::profile_from_type_def(&block_type_def);
            }

            // Post-startup transitions
            E2ETransition::CreateDocument { file_name } => {
                let doc_uri = state.next_synthetic_doc_uri();
                state.documents.insert(doc_uri.clone(), file_name.clone());

                // Add the page block (tags ⊇ ["Page"])
                let doc_name = std::path::Path::new(file_name.as_str())
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(file_name)
                    .to_string();
                let mut doc_block =
                    Block::new_text(doc_uri.clone(), EntityUri::no_parent(), doc_name);
                doc_block.set_page(true);
                // New empty documents don't have #+TODO: headers — keywords only
                // appear after the file is written with content. The on_file_changed
                // handler syncs parsed keywords to the document block.
                state.block_state.blocks.insert(doc_uri.clone(), doc_block);
                state
                    .block_state
                    .block_documents
                    .insert(doc_uri.clone(), doc_uri);
            }
            E2ETransition::ApplyMutation(event) => {
                if event.source == MutationSource::UI {
                    state.push_undo_snapshot();
                }
                if let Mutation::Create { id, parent_id, .. } = &event.mutation {
                    let doc_uri = if parent_id.is_no_parent() || parent_id.is_sentinel() {
                        parent_id.clone()
                    } else {
                        // Look up parent's document from block_documents map
                        state
                            .block_state
                            .block_documents
                            .get(parent_id)
                            .cloned()
                            .unwrap_or_else(|| parent_id.clone())
                    };
                    state
                        .block_state
                        .block_documents
                        .insert(id.clone(), doc_uri);
                }

                let mut blocks: Vec<Block> = state.block_state.blocks.values().cloned().collect();
                event.mutation.apply_to(&mut blocks);
                // Both UI and External mutations trigger org sync re-render
                // (on_block_changed), which re-writes the org file in canonical
                // order (source blocks first). Re-assign sequences to match.
                assign_reference_sequences_canonical(&mut blocks);
                state.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
                state.rebuild_profile_tracking();

                // Update tracked render expressions if a render source block was mutated
                if let Mutation::Update { id, fields, .. } = &event.mutation
                    && state.layout_blocks.render_source_ids.contains(id)
                    && fields.contains_key("content")
                    && let Some(block) = state.block_state.blocks.get(id)
                    && let Some(expr) =
                        super::reference_state::render_expr_from_rhai(block.content.as_str())
                {
                    state.render_expressions.insert(id.clone(), expr);
                }

                state.block_state.next_id += 1;

                // Update focus tracking after mutation
                match &event.mutation {
                    Mutation::Update { id, fields, .. } if fields.contains_key("content") => {
                        state.reset_cursor_if_focused(id);
                    }
                    Mutation::Delete { id, .. } => {
                        state.clear_focus_if_deleted(id);
                    }
                    _ => {}
                }
            }
            E2ETransition::SetupWatch {
                query_id,
                query,
                language,
            } => {
                state.active_watches.insert(
                    query_id.clone(),
                    WatchSpec {
                        query: query.clone(),
                        language: *language,
                    },
                );
            }
            E2ETransition::RemoveWatch { query_id } => {
                state.active_watches.remove(query_id);
            }
            E2ETransition::SwitchView { view_name } => {
                state.current_view = view_name.clone();
            }
            E2ETransition::NavigateFocus { region, block_id } => {
                let history = state
                    .navigation_history
                    .entry(*region)
                    .or_insert_with(NavigationHistory::new);

                history.entries.truncate(history.cursor + 1);
                history.entries.push(Some(block_id.clone()));
                history.cursor = history.entries.len() - 1;

                // NavigateFocus changes what's displayed but clears editor focus —
                // the previously-focused block may no longer be visible.
                state.focused_entity_id.remove(region);
                state.focused_cursor.remove(region);

                // Mirror `UiState::set_focus`: the navigation target becomes the
                // globally focused block. `focus_chain()` and `chain_ops()` read
                // from this — inv11 asserts they reflect the predicted URI.
                state.focused_block = Some(block_id.clone());
            }
            E2ETransition::NavigateBack { region } => {
                if let Some(history) = state.navigation_history.get_mut(region)
                    && history.cursor > 0
                {
                    history.cursor -= 1;
                }
                state.focused_entity_id.remove(region);
                state.focused_cursor.remove(region);
            }
            E2ETransition::NavigateForward { region } => {
                if let Some(history) = state.navigation_history.get_mut(region)
                    && history.cursor < history.entries.len() - 1
                {
                    history.cursor += 1;
                }
                state.focused_entity_id.remove(region);
                state.focused_cursor.remove(region);
            }
            E2ETransition::NavigateHome { region } => {
                let history = state
                    .navigation_history
                    .entry(*region)
                    .or_insert_with(NavigationHistory::new);

                history.entries.truncate(history.cursor + 1);
                history.entries.push(None);
                history.cursor = history.entries.len() - 1;

                state.focused_entity_id.remove(region);
                state.focused_cursor.remove(region);
                // Mirror production: `maybe_mirror_navigation_focus` clears
                // UiState.focused_block globally on "go_home", regardless of
                // which region triggered it. See reactive.rs:1824.
                state.focused_block = None;
            }
            E2ETransition::ClickBlock { region, block_id } => {
                // The default LeftSidebar wraps each doc in a `selectable` whose
                // bound action is `navigation.focus(region: "main", block_id: col("id"))`.
                // Clicking it dispatches that intent, which the production
                // navigation provider maps to a navigation-history push for
                // region=Main. Mirror that here so `focus_roots` / `current_focus`
                // checks line up with the real backend after the click.
                //
                // Other regions (Main, RightSidebar) don't have bound actions
                // in the default layout — clicking just sets editor focus. Once
                // we re-enable `LAYOUT_MUTATIONS_ENABLED`, this will need a
                // smarter "what is bound on this widget?" lookup against the
                // active render expression.
                if *region == Region::LeftSidebar {
                    let history = state
                        .navigation_history
                        .entry(Region::Main)
                        .or_insert_with(NavigationHistory::new);
                    history.entries.truncate(history.cursor + 1);
                    history.entries.push(Some(block_id.clone()));
                    history.cursor = history.entries.len() - 1;

                    state.focused_entity_id.remove(&Region::Main);
                    state.focused_cursor.remove(&Region::Main);
                    state.focused_block = Some(block_id.clone());
                } else {
                    // Clicking sets editor focus but does NOT change the navigation cursor.
                    // The user is still viewing the same document; only the focused editor
                    // changes. Arrow keys will now navigate among the clicked block's siblings.
                    // The global `focused_block` mirror also follows the click — production
                    // GPUI's `render_entity` click handler calls `services.set_focus(Some(id))`
                    // before dispatching `editor_focus`.
                    state.focused_block = Some(block_id.clone());
                    state.focused_entity_id.insert(*region, block_id.clone());
                    state
                        .focused_cursor
                        .insert(*region, CursorPosition::start());
                }
            }
            E2ETransition::ArrowNavigate {
                region,
                direction,
                steps,
            } => {
                use holon_frontend::navigation::{
                    Boundary, CollectionNavigator, CursorHint, NavDirection,
                };

                let mut current_id = state
                    .focused_entity_id
                    .get(region)
                    .expect("ArrowNavigate requires focused entity")
                    .clone();
                let mut cursor = state
                    .focused_cursor
                    .get(region)
                    .copied()
                    .unwrap_or(CursorPosition::start());

                let navigator = state.build_reference_navigator(*region);

                for _ in 0..*steps {
                    // Get the content of the currently focused block
                    let content = state
                        .block_state
                        .blocks
                        .get(&current_id)
                        .map(|b| b.content.as_str())
                        .unwrap_or("");
                    let line_count = if content.is_empty() {
                        1
                    } else {
                        content.split('\n').count()
                    };
                    let last_line = line_count.saturating_sub(1);

                    // Predict whether this arrow causes cross-block navigation
                    let crosses_block = match direction {
                        NavDirection::Up => cursor.line == 0,
                        NavDirection::Down => cursor.line >= last_line,
                        NavDirection::Left => cursor.line == 0 && cursor.column == 0,
                        NavDirection::Right => {
                            let line_len = content
                                .split('\n')
                                .nth(cursor.line)
                                .map(|l| l.len())
                                .unwrap_or(0);
                            cursor.line >= last_line && cursor.column >= line_len
                        }
                    };

                    if crosses_block {
                        if let Some(ref nav) = navigator {
                            let boundary = match direction {
                                NavDirection::Up => Boundary::Top,
                                NavDirection::Down => Boundary::Bottom,
                                NavDirection::Left => Boundary::Left,
                                NavDirection::Right => Boundary::Right,
                            };
                            let hint = CursorHint {
                                column: cursor.column,
                                boundary,
                            };
                            if let Some(target) =
                                nav.navigate(current_id.as_str(), *direction, &hint)
                            {
                                current_id = EntityUri::from_raw(&target.block_id);
                                // Update cursor from placement
                                let target_content = state
                                    .block_state
                                    .blocks
                                    .get(&current_id)
                                    .map(|b| b.content.as_str())
                                    .unwrap_or("");
                                let offset = holon_frontend::navigation::placement_to_offset(
                                    target_content,
                                    target.placement,
                                );
                                let (line, col) = holon_frontend::navigation::offset_to_line_col(
                                    target_content,
                                    offset,
                                );
                                cursor = CursorPosition { line, column: col };
                            }
                            // else: at boundary of collection, stay put
                        }
                    } else {
                        // Intra-block cursor movement
                        match direction {
                            NavDirection::Up => {
                                cursor.line = cursor.line.saturating_sub(1);
                            }
                            NavDirection::Down => {
                                cursor.line = (cursor.line + 1).min(last_line);
                            }
                            NavDirection::Left => {
                                if cursor.column > 0 {
                                    cursor.column -= 1;
                                } else if cursor.line > 0 {
                                    cursor.line -= 1;
                                    let prev_line_len = content
                                        .split('\n')
                                        .nth(cursor.line)
                                        .map(|l| l.len())
                                        .unwrap_or(0);
                                    cursor.column = prev_line_len;
                                }
                            }
                            NavDirection::Right => {
                                let line_len = content
                                    .split('\n')
                                    .nth(cursor.line)
                                    .map(|l| l.len())
                                    .unwrap_or(0);
                                if cursor.column < line_len {
                                    cursor.column += 1;
                                } else if cursor.line < last_line {
                                    cursor.line += 1;
                                    cursor.column = 0;
                                }
                            }
                        }
                    }
                }

                // Update focused entity and cursor. Arrow keys change editor
                // focus but NOT navigation — navigation_history is untouched.
                // The global `focused_block` mirror also moves: production
                // GPUI's arrow handler calls `services.set_focus()` on the
                // new target (mirroring what a click would do), so the
                // engine's `UiState.focused_block` follows the per-region
                // pointer.
                state.focused_block = Some(current_id.clone());
                state.focused_entity_id.insert(*region, current_id);
                state.focused_cursor.insert(*region, cursor);
            }
            E2ETransition::SimulateRestart => {
                // SimulateRestart doesn't change reference state - blocks should be preserved.
                // The SUT will clear last_projection and trigger file re-processing.
            }
            E2ETransition::BulkExternalAdd { blocks, .. } => {
                // Add all blocks to the reference state, normalizing each
                // block's content the same way `Mutation::apply_to` does for
                // Create. The org renderer round-trips through the parser
                // (which `.trim()`s headlines and `.trim_end()`s content),
                // so the ref must mirror that normalization or `text(col(...))`
                // displays diverge by the trailing-whitespace the parser
                // strips. Without this, `inv-displayed-text` panics on bulk
                // blocks whose generator-produced content ends in a space.
                for block in blocks {
                    let mut block = block.clone();
                    block.content =
                        normalize_content_for_org_roundtrip(&block.content, block.content_type);
                    state.block_state.blocks.insert(block.id.clone(), block);
                }
                // BulkExternalAdd serializes via serialize_blocks_to_org (canonical order)
                let mut all_blocks: Vec<Block> =
                    state.block_state.blocks.values().cloned().collect();
                assign_reference_sequences_canonical(&mut all_blocks);
                state.block_state.blocks =
                    all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
                state.rebuild_profile_tracking();
                state.block_state.next_id += blocks.len();
            }
            E2ETransition::ConcurrentSchemaInit => {
                // ConcurrentSchemaInit doesn't change reference state - it only tests
                // that the database doesn't get locked when schema init runs concurrently.
            }
            E2ETransition::EditViaDisplayTree {
                block_id,
                new_content,
            } => {
                state.push_undo_snapshot();
                state.apply_mutation(&MutationEvent {
                    source: MutationSource::UI,
                    mutation: Mutation::Update {
                        entity: "block".to_string(),
                        id: block_id.clone(),
                        fields: [("content".to_string(), Value::String(new_content.clone()))]
                            .into(),
                    },
                });
                state.reset_cursor_if_focused(block_id);
            }

            E2ETransition::EditViaViewModel {
                block_id,
                new_content,
            } => {
                state.push_undo_snapshot();
                state.apply_mutation(&MutationEvent {
                    source: MutationSource::UI,
                    mutation: Mutation::Update {
                        entity: "block".to_string(),
                        id: block_id.clone(),
                        fields: [("content".to_string(), Value::String(new_content.clone()))]
                            .into(),
                    },
                });
                state.reset_cursor_if_focused(block_id);
            }

            E2ETransition::ToggleState {
                block_id,
                new_state,
            } => {
                state.push_undo_snapshot();
                state.apply_mutation(&MutationEvent {
                    source: MutationSource::UI,
                    mutation: Mutation::Update {
                        entity: "block".to_string(),
                        id: block_id.clone(),
                        fields: [("task_state".to_string(), Value::String(new_state.clone()))]
                            .into(),
                    },
                });
            }

            E2ETransition::TriggerSlashCommand { block_id } => {
                state.push_undo_snapshot();
                state.apply_mutation(&MutationEvent {
                    source: MutationSource::UI,
                    mutation: Mutation::Delete {
                        entity: "block".to_string(),
                        id: block_id.clone(),
                    },
                });
                state.clear_focus_if_deleted(block_id);
            }

            E2ETransition::TriggerDocLink { .. } => {
                // Read-only: validates the [[ trigger → InsertText pipeline.
                // No state change in the reference model.
            }

            E2ETransition::ConcurrentMutations {
                ui_mutation,
                external_mutation,
            } => {
                // Detect same-block concurrent content updates — use Loro CRDT merge
                // to determine the expected merged content rather than naive LWW.
                let same_block_update = match (&ui_mutation.mutation, &external_mutation.mutation) {
                    (
                        Mutation::Update {
                            id: ui_id,
                            fields: ui_fields,
                            ..
                        },
                        Mutation::Update {
                            id: ext_id,
                            fields: ext_fields,
                            ..
                        },
                    ) if ui_id == ext_id => {
                        let ui_content = ui_fields.get("content").and_then(|v| v.as_string());
                        let ext_content = ext_fields.get("content").and_then(|v| v.as_string());
                        match (ui_content, ext_content) {
                            (Some(ui_c), Some(ext_c)) => {
                                Some((ui_id.clone(), ui_c.to_string(), ext_c.to_string()))
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };

                if let Some((block_id, ui_content, ext_content)) = same_block_update {
                    if state.variant.enable_loro {
                        // Loro CRDT merge determines expected content
                        let original = state
                            .block_state
                            .blocks
                            .get(&block_id)
                            .map(|b| b.content.as_str())
                            .unwrap_or("");
                        let merged = loro_merge_text(original, &ui_content, &ext_content);
                        if let Some(block) = state.block_state.blocks.get_mut(&block_id) {
                            block.content = merged;
                        }
                    } else {
                        // Without Loro, external write (org file) wins via LWW
                        if let Some(block) = state.block_state.blocks.get_mut(&block_id) {
                            block.content = ext_content;
                        }
                    }
                } else {
                    // Non-overlapping mutations: apply both sequentially
                    for event in [ui_mutation, external_mutation] {
                        if let Mutation::Create { id, parent_id, .. } = &event.mutation {
                            let doc_uri = if parent_id.is_no_parent() || parent_id.is_sentinel() {
                                parent_id.clone()
                            } else {
                                fn find_doc(
                                    block_id: &EntityUri,
                                    state: &ReferenceState,
                                ) -> Option<EntityUri> {
                                    let block = state.block_state.blocks.get(block_id)?;
                                    if block.parent_id.is_no_parent()
                                        || block.parent_id.is_sentinel()
                                    {
                                        Some(block.parent_id.clone())
                                    } else {
                                        find_doc(&block.parent_id, state)
                                    }
                                }
                                find_doc(parent_id, &state).unwrap_or_else(|| parent_id.clone())
                            };
                            state
                                .block_state
                                .block_documents
                                .insert(id.clone(), doc_uri);
                        }

                        let mut blocks: Vec<Block> =
                            state.block_state.blocks.values().cloned().collect();
                        event.mutation.apply_to(&mut blocks);
                        state.block_state.blocks =
                            blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
                    }
                }
                // External mutation re-writes org file in canonical order
                let mut all_blocks: Vec<Block> =
                    state.block_state.blocks.values().cloned().collect();
                assign_reference_sequences_canonical(&mut all_blocks);
                state.block_state.blocks =
                    all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
                state.rebuild_profile_tracking();
                // UI used next_id, External used next_id+1
                state.block_state.next_id += 2;
            }

            E2ETransition::Indent { block_id } => {
                state.push_undo_snapshot();
                let prev_id = state.previous_sibling(block_id).unwrap();
                // Production indent re-parents the block under its previous
                // sibling, anchored after that parent's current last child —
                // i.e. it lands at the end of the new sibling group. Mirror
                // that with `move_block(after = last_child_of(prev_id))`.
                let after = state
                    .sorted_children_of(&prev_id)
                    .last()
                    .map(|b| b.id.clone());
                state.move_block(block_id, prev_id, after.as_ref());
            }

            E2ETransition::Outdent { block_id } => {
                state.push_undo_snapshot();
                state.outdent_block(block_id);
            }

            E2ETransition::MoveUp { block_id } => {
                state.push_undo_snapshot();
                let prev_id = state.previous_sibling(block_id).unwrap();
                state.swap_sequence(block_id, &prev_id);
            }

            E2ETransition::MoveDown { block_id } => {
                state.push_undo_snapshot();
                let next_id = state.next_sibling(block_id).unwrap();
                state.swap_sequence(block_id, &next_id);
            }

            E2ETransition::DragDropBlock { source, target } => {
                state.push_undo_snapshot();
                // Production's drop_zone dispatches `move_block(id=source,
                // parent_id=target, after_block_id=None)` which inserts at
                // the beginning of the target's children.
                state.move_block(source, target.clone(), None);
            }

            E2ETransition::SplitBlock { block_id, position } => {
                state.push_undo_snapshot();
                state.split_block(block_id, *position);
                state.reset_cursor_if_focused(block_id);
            }

            E2ETransition::JoinBlock { block_id } => {
                state.push_undo_snapshot();
                // Determine the merge target before mutation: prev sibling if
                // present, otherwise the parent block (child→parent join).
                let target_id = state.previous_sibling(block_id).unwrap_or_else(|| {
                    state
                        .block_state
                        .blocks
                        .get(block_id)
                        .map(|b| b.parent_id.clone())
                        .expect("JoinBlock precondition: block must exist with a parent")
                });
                state.join_block(block_id);
                // Focus moves to the merge target (prev sibling OR parent);
                // cursor lands at the join boundary, but the reference model
                // tracks (line, column) — match SplitBlock's behaviour and
                // reset to start. Production sets cursor at join boundary
                // via the editor_focus follow-up; PBT cursor checks are
                // best-effort and do not gate the test.
                use holon_api::Region;
                state.focused_entity_id.insert(Region::Main, target_id);
                state.focused_cursor.insert(
                    Region::Main,
                    super::reference_state::CursorPosition::start(),
                );
            }

            E2ETransition::UndoLastMutation => {
                state.pop_undo_to_redo();
                // Undo may restore different content — reset all cursors
                for region in state.focused_entity_id.keys().cloned().collect::<Vec<_>>() {
                    state.focused_cursor.insert(region, CursorPosition::start());
                }
            }
            E2ETransition::Redo => {
                state.pop_redo_to_undo();
                for region in state.focused_entity_id.keys().cloned().collect::<Vec<_>>() {
                    state.focused_cursor.insert(region, CursorPosition::start());
                }
            }
            E2ETransition::EmitMcpData => {
                // No reference state change — just triggers IVM re-evaluation.
            }

            // === Multi-instance sync transitions ===
            E2ETransition::AddPeer => {
                let peer_id = (state.peers.len() as u64) + 100;
                // Peer starts with a copy of the primary's blocks.
                // Exclude seed blocks (doc = sentinel) — they are inserted via
                // direct SQL and never reverse-synced to Loro, so the actual
                // peer LoroDoc (created from a Loro snapshot) won't have them.
                let peer_blocks: HashMap<String, super::peer_ops::PeerBlock> = state
                    .block_state
                    .blocks
                    .values()
                    .filter(|b| {
                        // Exclude seed blocks (doc = sentinel/no_parent).
                        let is_seed = state
                            .block_state
                            .block_documents
                            .get(&b.id)
                            .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel());
                        // Exclude document blocks (name != None) — they exist
                        // in the reference model but not as Loro tree nodes.
                        !is_seed && !b.is_page()
                    })
                    .map(|b| {
                        let pb = super::peer_ops::PeerBlock {
                            stable_id: b.id.id().to_string(),
                            parent_stable_id: if b.parent_id.is_no_parent()
                                || b.parent_id.is_sentinel()
                            {
                                None
                            } else {
                                Some(b.parent_id.id().to_string())
                            },
                            content: b.content_text().to_string(),
                        };
                        (pb.stable_id.clone(), pb)
                    })
                    .collect();
                let baseline_contents = peer_blocks
                    .values()
                    .map(|pb| (pb.stable_id.clone(), pb.content.clone()))
                    .collect();
                state.peers.push(super::reference_state::PeerRefState {
                    peer_id,
                    blocks: peer_blocks,
                    deleted_stable_ids: std::collections::HashSet::new(),
                    modified_stable_ids: std::collections::HashSet::new(),
                    created_stable_ids: std::collections::HashSet::new(),
                    baseline_contents,
                });
            }

            E2ETransition::PeerEdit { peer_idx, op } => {
                use super::transitions::PeerEditOp;
                let peer = &mut state.peers[*peer_idx];
                match op {
                    PeerEditOp::Create {
                        parent_stable_id,
                        content,
                        stable_id,
                    } => {
                        peer.blocks.insert(
                            stable_id.clone(),
                            super::peer_ops::PeerBlock {
                                stable_id: stable_id.clone(),
                                parent_stable_id: parent_stable_id.clone(),
                                content: content.clone(),
                            },
                        );
                        peer.created_stable_ids.insert(stable_id.clone());
                    }
                    PeerEditOp::Update { stable_id, content } => {
                        if let Some(block) = peer.blocks.get_mut(stable_id) {
                            block.content = content.clone();
                            peer.modified_stable_ids.insert(stable_id.clone());
                        }
                    }
                    PeerEditOp::Delete { stable_id } => {
                        peer.blocks.remove(stable_id);
                        peer.deleted_stable_ids.insert(stable_id.clone());
                    }
                }
            }

            E2ETransition::MergeFromPeer { peer_idx } => {
                let modified = state.peers[*peer_idx].modified_stable_ids.clone();
                let created = state.peers[*peer_idx].created_stable_ids.clone();
                let baseline = state.peers[*peer_idx].baseline_contents.clone();
                state.peers[*peer_idx].deleted_stable_ids.clear();
                state.peers[*peer_idx].modified_stable_ids.clear();
                state.peers[*peer_idx].created_stable_ids.clear();
                let peer_blocks: Vec<_> = state.peers[*peer_idx].blocks.values().cloned().collect();
                merge_peer_blocks_into_primary(
                    &mut state.block_state,
                    &peer_blocks,
                    &modified,
                    &created,
                    &baseline,
                );
                // Newly-created peer blocks are inserted with the default
                // `sequence=0` (via `Block::default()` in `from_block_content`),
                // colliding with whatever sequence the parent's existing
                // children already have. Production renders by
                // `(content_type group, sort_key, id)` and re-parses sequences
                // 0..N in render order — so the parent's child ordering only
                // converges after a recanon. Without this, the assertion at
                // `assertions.rs:117` flags an order mismatch on the next
                // org-file round-trip.
                state.recanon_and_rebuild();
                refresh_peer_baseline(&mut state.peers[*peer_idx]);
            }

            E2ETransition::SyncWithPeer { peer_idx } => {
                // Peer → primary: apply creates/updates.
                // Peer deletes are tracked on `deleted_stable_ids` but
                // NOT propagated to the primary reference model. The
                // production path DOES propagate them (via
                // `LoroSyncController`'s `subscribe_root` → outbound
                // reconcile), but accurately mirroring Loro's cascading-
                // delete semantics in the reference model requires a
                // PBT-wide refactor — the ref model uses a different ID
                // convention than the peer's Loro tree. Known gap:
                // scenarios that combine `PeerEdit::Delete` with
                // `SyncWithPeer`/`MergeFromPeer` will flag divergence.
                let modified = state.peers[*peer_idx].modified_stable_ids.clone();
                let created = state.peers[*peer_idx].created_stable_ids.clone();
                let baseline = state.peers[*peer_idx].baseline_contents.clone();
                state.peers[*peer_idx].deleted_stable_ids.clear();
                state.peers[*peer_idx].modified_stable_ids.clear();
                state.peers[*peer_idx].created_stable_ids.clear();
                let peer_blocks: Vec<_> = state.peers[*peer_idx].blocks.values().cloned().collect();
                merge_peer_blocks_into_primary(
                    &mut state.block_state,
                    &peer_blocks,
                    &modified,
                    &created,
                    &baseline,
                );
                // See the matching comment in `MergeFromPeer` above.
                state.recanon_and_rebuild();

                // Primary → peer: add any primary blocks missing from peer,
                // but skip blocks the peer deleted in this round (already removed
                // from primary above, so they won't be re-added).
                //
                // Also skip seed blocks (document blocks at sentinel:no_parent).
                // These live in the reference model for bookkeeping but never
                // reach Loro — the actual peer LoroDoc doesn't contain them.
                // Including them here would create a PeerBlock with
                // `stable_id = "no_parent"` (from the seed_doc_block's URI),
                // which a subsequent `MergeFromPeer` would synthesize back
                // into the primary ref model as a phantom `block:no_parent`.
                let primary_as_peer: Vec<_> = state
                    .block_state
                    .blocks
                    .values()
                    .filter(|b| {
                        let is_seed = state
                            .block_state
                            .block_documents
                            .get(&b.id)
                            .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel());
                        !is_seed && !b.is_page()
                    })
                    .map(|b| super::peer_ops::PeerBlock {
                        stable_id: b.id.id().to_string(),
                        parent_stable_id: if b.parent_id.is_no_parent() || b.parent_id.is_sentinel()
                        {
                            None
                        } else {
                            Some(b.parent_id.id().to_string())
                        },
                        content: b.content_text().to_string(),
                    })
                    .collect();
                let peer = &mut state.peers[*peer_idx];
                for pb in primary_as_peer {
                    // Bidirectional sync: both sides converge to the merged
                    // primary state. Overwrite (not just `or_insert`) so the
                    // peer's PeerBlock content reflects the post-merge truth.
                    peer.blocks.insert(pb.stable_id.clone(), pb);
                }
                refresh_peer_baseline(peer);
            }

            E2ETransition::PeerCharEdit {
                peer_idx,
                block_id,
                op: _,
            } => {
                // Reference model: PeerCharEdit doesn't change block-level
                // content (it operates at the LoroText character level).
                // The block content in the reference model stays the same;
                // cross-peer text convergence is checked by inv-cross-peer-
                // text-convergence after SyncWithPeer.
                let _ = (peer_idx, block_id);
            }

            // ── Atomic editor primitives ──
            E2ETransition::FocusEditableText { block_id } => {
                let saved = state
                    .block_state
                    .blocks
                    .get(block_id)
                    .map(|b| b.content.clone())
                    .unwrap_or_default();
                let cursor_byte = saved.len();
                state.active_editor = Some(super::reference_state::ActiveEditor {
                    block_id: block_id.clone(),
                    in_memory_content: saved,
                    cursor_byte,
                });
                // NOTE: deliberately do NOT update `focused_entity_id` /
                // `focused_block`. inv15 compares those to
                // `engine.focused_block()`, but production's set_focus()
                // path is signal-loop driven and a synthetic click may
                // not update it deterministically. `active_editor` is the
                // source of truth for editor focus; navigation focus
                // stays untouched.
            }
            E2ETransition::MoveCursor { byte_position } => {
                if let Some(editor) = state.active_editor.as_mut() {
                    editor.move_cursor(*byte_position);
                }
            }
            E2ETransition::TypeChars { text } => {
                if let Some(editor) = state.active_editor.as_mut() {
                    editor.type_chars(text);
                }
            }
            E2ETransition::DeleteBackward { count } => {
                if let Some(editor) = state.active_editor.as_mut() {
                    editor.delete_backward(*count);
                }
            }
            E2ETransition::Blur => {
                state.commit_active_editor_if_changed();
                state.active_editor = None;
            }
            E2ETransition::PressKey { chord } => {
                use holon_api::{Key, Region};

                let Some(editor) = state.active_editor.clone() else {
                    return state;
                };
                let block_id = editor.block_id.clone();
                let cursor_byte = editor.cursor_byte;

                let has_modifier = chord
                    .0
                    .iter()
                    .any(|k| matches!(k, Key::Cmd | Key::Ctrl | Key::Alt | Key::Shift));
                let regulars: Vec<Key> = chord
                    .0
                    .iter()
                    .filter(|k| !matches!(k, Key::Cmd | Key::Ctrl | Key::Alt | Key::Shift))
                    .cloned()
                    .collect();
                let single = if regulars.len() == 1 {
                    Some(regulars[0].clone())
                } else {
                    None
                };

                // Enter (no modifier): commit pending edit, then split
                // at cursor against the post-commit content.
                if matches!(single, Some(Key::Enter)) && !has_modifier {
                    state.commit_active_editor_if_changed();
                    state.push_undo_snapshot();
                    state.split_block(&block_id, cursor_byte);
                    state.reset_cursor_if_focused(&block_id);
                    state.active_editor = None;
                    state.focused_entity_id.remove(&Region::Main);
                }
                // Backspace at position 0: commit, then join.
                else if matches!(single, Some(Key::Backspace))
                    && !has_modifier
                    && cursor_byte == 0
                {
                    state.commit_active_editor_if_changed();
                    let prev = state.previous_sibling(&block_id);
                    let parent = state
                        .block_state
                        .blocks
                        .get(&block_id)
                        .map(|b| b.parent_id.clone());
                    let target_id = match (&prev, &parent) {
                        (Some(p), _) => Some(p.clone()),
                        (None, Some(p)) => {
                            // Only join into parent if parent is a non-layout text block.
                            let parent_ok = state
                                .block_state
                                .blocks
                                .get(p)
                                .is_some_and(|b| b.content_type == ContentType::Text)
                                && !state.layout_blocks.contains(p)
                                && !p.is_no_parent()
                                && !p.is_sentinel();
                            if parent_ok { Some(p.clone()) } else { None }
                        }
                        _ => None,
                    };
                    if let Some(target_id) = target_id {
                        state.push_undo_snapshot();
                        state.join_block(&block_id);
                        state.focused_entity_id.insert(Region::Main, target_id);
                        state.focused_cursor.insert(
                            Region::Main,
                            super::reference_state::CursorPosition::start(),
                        );
                        state.active_editor = None;
                    }
                }
                // Other chords (Tab, Escape, etc.): no structural change
                // modeled in v1. Pending edits remain in InputState.
            }
        }
        state.last_transition_kind = Some(transition.variant_name());
        state
    }
}

impl E2ETransition {
    /// Whether this transition is valid against the given reference state.
    ///
    /// Used both by proptest's `preconditions` (which the shrinker consults
    /// when removing transitions from a sequence — it doesn't re-run the
    /// generator) and by per-transition generator filters (so a generator
    /// can't produce a transition that the precondition would reject).
    pub fn precondition(&self, state: &ReferenceState) -> bool {
        // Preconditions are a SAFETY NET as well as a generator filter: the
        // generators above pre-narrow candidate args for low rejection
        // rates, but they call this method as a final check so they
        // can't drift from the precondition's view of validity.
        //
        // WriteOrgFile: always valid (can write files before or after startup)
        // StartApp: only valid when app is not started
        // All other transitions: only valid after startup
        match self {
            E2ETransition::Nothing => true,
            // Pre-startup transitions
            E2ETransition::WriteOrgFile { filename, content } => {
                if state.app_started {
                    return false;
                }
                // Reject if any block IDs in this file already exist under a different document.
                // Org :ID: properties must be globally unique — the system asserts on duplicates.
                let doc_name = std::path::Path::new(filename.as_str())
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or(filename);
                let doc_uri = state
                    .doc_uri_by_name(doc_name)
                    .unwrap_or_else(|| EntityUri::block("precondition-placeholder"));
                let id_re = regex::Regex::new(r":ID:\s*(\S+)").unwrap();
                for caps in id_re.captures_iter(content) {
                    let block_id = caps.get(1).unwrap().as_str();
                    let block_entity = EntityUri::block(block_id);
                    if let Some(existing_doc) = state.block_state.block_documents.get(&block_entity)
                        && *existing_doc != doc_uri
                    {
                        return false;
                    }
                }
                true
            }
            E2ETransition::CreateDirectory { .. } => !state.app_started, // Only before startup
            E2ETransition::GitInit => !state.app_started && !state.git_initialized, // Once only
            E2ETransition::JjGitInit => !state.app_started && !state.jj_initialized, // Once only
            E2ETransition::CreateStaleLoro { org_filename, .. } => {
                // Only valid before startup, and org file must exist
                !state.app_started && state.documents.values().any(|f| f == org_filename)
            }
            E2ETransition::StartApp { .. } => {
                // Require at least one org file before startup. The system always loads
                // assets/default/index.org, but the reference model only tracks blocks
                // from explicitly written files.
                !state.app_started && state.pre_startup_file_count > 0
            }

            // Post-startup transitions (require app to be running)
            E2ETransition::CreateDocument { .. } => state.app_started,
            E2ETransition::ApplyMutation(event) => {
                if !state.app_started {
                    return false;
                }
                match &event.mutation {
                    Mutation::Delete { id, .. } => {
                        state.block_state.blocks.contains_key(id)
                            && !state.layout_blocks.contains(id)
                    }
                    Mutation::Update { id, .. } => {
                        state.block_state.blocks.contains_key(id)
                            && !state.layout_blocks.is_immutable(id)
                    }
                    Mutation::Move {
                        id, new_parent_id, ..
                    } => {
                        state.block_state.blocks.contains_key(id)
                            // Don't move source blocks — Org format determines their parent
                            // by heading position, so moves can't round-trip correctly.
                            && state
                                .block_state.blocks
                                .get(id)
                                .is_some_and(|b| b.content_type != ContentType::Source)
                            && state
                                .block_state.blocks
                                .get(new_parent_id)
                                .map_or(state.documents.contains_key(new_parent_id), |b| {
                                    b.content_type != ContentType::Source
                                })
                    }
                    Mutation::Create { parent_id, .. } => {
                        state.documents.contains_key(parent_id)
                            || state.block_state.blocks.get(parent_id).is_some_and(|b| {
                                // Don't create children under source blocks — Org format
                                // can't represent children inside #+begin_src blocks, so
                                // the Org round-trip would flatten the hierarchy.
                                b.content_type != ContentType::Source
                            })
                    }
                    Mutation::RestartApp => true,
                }
            }
            E2ETransition::SetupWatch { .. } => state.app_started,
            E2ETransition::RemoveWatch { query_id } => {
                state.app_started && state.active_watches.contains_key(query_id)
            }
            E2ETransition::SwitchView { .. } => state.app_started,
            E2ETransition::NavigateFocus { block_id, .. } => {
                state.app_started && state.block_state.blocks.contains_key(block_id)
            }
            E2ETransition::NavigateBack { region } => {
                state.app_started && state.can_go_back(*region)
            }
            E2ETransition::NavigateForward { region } => {
                state.app_started && state.can_go_forward(*region)
            }
            E2ETransition::NavigateHome { .. } => state.app_started,
            E2ETransition::ClickBlock { block_id, region } => {
                state.app_started
                    && state.block_state.blocks.contains_key(block_id)
                    && state.layout_blocks.is_focusable(block_id)
                    && !state.focusable_rendered_block_ids(*region).is_empty()
            }
            E2ETransition::ArrowNavigate { region, .. } => {
                state.app_started && state.focused_entity_id.contains_key(region)
            }
            E2ETransition::SimulateRestart => {
                state.app_started && !state.block_state.blocks.is_empty()
            }
            E2ETransition::BulkExternalAdd { doc_uri, .. } => {
                state.app_started && state.documents.contains_key(doc_uri)
            }
            E2ETransition::ConcurrentSchemaInit => {
                state.app_started
                    && !state.block_state.blocks.is_empty()
                    && !state.active_watches.is_empty()
            }
            E2ETransition::EditViaDisplayTree { block_id, .. } => {
                let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
                state.app_started
                    && state.is_properly_setup()
                    && state.block_state.blocks.contains_key(block_id)
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .is_some_and(|b| b.content_type == ContentType::Text)
                    && !state.layout_blocks.contains(block_id)
                    && state.is_descendant_of_any(block_id, &focus_roots)
            }
            E2ETransition::EditViaViewModel { block_id, .. } => {
                let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
                state.app_started
                    && state.is_properly_setup()
                    && state.block_state.blocks.contains_key(block_id)
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .is_some_and(|b| b.content_type == ContentType::Text)
                    && !state.layout_blocks.contains(block_id)
                    && state.is_descendant_of_any(block_id, &focus_roots)
            }
            // ToggleState requires the block to be a *direct child* of the
            // current Main navigation focus — i.e. in `expected_focus_root_ids(Main)`.
            // The generator filter at state_machine.rs:935 only emits ToggleState
            // for blocks whose ID is in `main_focus_roots`, but the proptest
            // shrinker can drop the establishing ClickBlock/NavigateFocus,
            // leaving an orphan ToggleState in the sequence whose target block
            // is not actually rendered in the Main panel — `wait_for_entity_in_resolved_view_model`
            // then times out at sut.rs:1727. Mirror the generator's visibility
            // filter here so shrunk sequences without a valid Main focus are
            // rejected.
            //
            // The generator is still the authority for *which* state value to
            // pick (it walks the rendered ViewModel for state_toggle widgets);
            // we only re-validate the structural prerequisites the shrinker
            // could violate.
            E2ETransition::ToggleState { block_id, .. } => {
                let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
                state.app_started
                    && state.current_focus(holon_api::Region::Main).is_some()
                    // `region_predictable(Main)` rejects both custom/unparseable
                    // root layouts and layout-mutated panel render sources —
                    // either case produces rows with no entity binding, so
                    // `wait_for_entity_in_resolved_view_model` would time out.
                    && state.region_predictable(holon_api::Region::Main)
                    && focus_roots.contains(block_id)
                    // Layout headlines (in `layout_blocks.headline_ids`) define
                    // their own render expression via a child render source.
                    // Production renders the headline through that custom
                    // layout, which can omit `state_toggle` entirely. The
                    // headline never appears as a state_toggle entity in the
                    // resolved ViewModel, so ToggleState would time out.
                    // EditViaViewModel/Indent/MoveUp etc. already exclude
                    // layout blocks for the same reason.
                    && !state.layout_blocks.contains(block_id)
                    // A custom entity profile for `block` can replace the
                    // default render with anything (e.g. just an
                    // `editable_text`) — losing the state_toggle widget.
                    // The reference state doesn't introspect the active
                    // variant's widget set, so conservatively skip
                    // ToggleState whenever a custom block profile is loaded.
                    && !state.has_blocks_profile()
            }
            E2ETransition::TriggerSlashCommand { block_id } => {
                let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
                state.app_started
                    && state.is_properly_setup()
                    && state.block_state.blocks.contains_key(block_id)
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .is_some_and(|b| b.content_type == ContentType::Text)
                    && !state.layout_blocks.contains(block_id)
                    && !block_id.as_str().contains("default-")
                    && state.block_state.blocks.len() > 2
                    && state.is_descendant_of_any(block_id, &focus_roots)
            }
            E2ETransition::TriggerDocLink {
                block_id,
                target_block_id,
            } => {
                let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
                state.app_started
                    && state.is_properly_setup()
                    && state.block_state.blocks.contains_key(block_id)
                    && state.block_state.blocks.contains_key(target_block_id)
                    && block_id != target_block_id
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .is_some_and(|b| b.content_type == ContentType::Text)
                    && !state.layout_blocks.contains(block_id)
                    && state.is_descendant_of_any(block_id, &focus_roots)
            }
            E2ETransition::ConcurrentMutations {
                ui_mutation,
                external_mutation,
            } => {
                if !state.app_started {
                    return false;
                }
                // Both sub-mutations must pass preconditions individually
                let ui_ok = E2ETransition::ApplyMutation(ui_mutation.clone()).precondition(state);
                let ext_ok =
                    E2ETransition::ApplyMutation(external_mutation.clone()).precondition(state);
                // Reject if both are creates with the same ID (impossible in practice)
                let same_create_id = matches!(
                    (&ui_mutation.mutation, &external_mutation.mutation),
                    (Mutation::Create { id: id1, .. }, Mutation::Create { id: id2, .. }) if id1 == id2
                );
                ui_ok && ext_ok && !same_create_id
            }
            E2ETransition::Indent { block_id } => {
                let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
                let focused_in_main = state.focused_entity(holon_api::Region::Main);
                state.app_started
                    && state.is_properly_setup()
                    && focused_in_main == Some(block_id)
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .is_some_and(|b| b.content_type == ContentType::Text)
                    && !state.layout_blocks.contains(block_id)
                    && state.is_descendant_of_any(block_id, &focus_roots)
                    && state.previous_sibling(block_id).is_some()
            }
            E2ETransition::Outdent { block_id } => {
                let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
                let focused_in_main = state.focused_entity(holon_api::Region::Main);
                state.app_started
                    && state.is_properly_setup()
                    && focused_in_main == Some(block_id)
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .is_some_and(|b| b.content_type == ContentType::Text)
                    && !state.layout_blocks.contains(block_id)
                    && state.is_descendant_of_any(block_id, &focus_roots)
                    && state.grandparent(block_id).is_some()
            }
            E2ETransition::MoveUp { block_id } => {
                let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
                let focused_in_main = state.focused_entity(holon_api::Region::Main);
                state.app_started
                    && state.is_properly_setup()
                    && focused_in_main == Some(block_id)
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .is_some_and(|b| b.content_type == ContentType::Text)
                    && !state.layout_blocks.contains(block_id)
                    && state.is_descendant_of_any(block_id, &focus_roots)
                    && state.previous_sibling(block_id).is_some()
            }
            E2ETransition::MoveDown { block_id } => {
                let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
                let focused_in_main = state.focused_entity(holon_api::Region::Main);
                state.app_started
                    && state.is_properly_setup()
                    && focused_in_main == Some(block_id)
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .is_some_and(|b| b.content_type == ContentType::Text)
                    && !state.layout_blocks.contains(block_id)
                    && state.is_descendant_of_any(block_id, &focus_roots)
                    && state.next_sibling(block_id).is_some()
            }
            E2ETransition::DragDropBlock { source, target } => {
                // Source must be the currently-focused block: this guarantees
                // it is rendered with a `Draggable` wrapper (the production
                // shape — a real user typically clicks the block before
                // dragging it). Target must be a different text block in the
                // focus tree so its `DropZone` widget is also rendered.
                if !state.app_started || !state.is_properly_setup() {
                    return false;
                }
                if source == target {
                    return false;
                }
                let focused_in_main = state.focused_entity(holon_api::Region::Main);
                if focused_in_main != Some(source) {
                    return false;
                }
                let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
                let is_text = |id: &EntityUri| {
                    state
                        .block_state
                        .blocks
                        .get(id)
                        .is_some_and(|b| b.content_type == ContentType::Text)
                };
                if !is_text(source) || !is_text(target) {
                    return false;
                }
                if state.layout_blocks.contains(source) || state.layout_blocks.contains(target) {
                    return false;
                }
                if !state.is_descendant_of_any(source, &focus_roots)
                    || !state.is_descendant_of_any(target, &focus_roots)
                {
                    return false;
                }
                // No-op: target is already source's parent.
                if state
                    .block_state
                    .blocks
                    .get(source)
                    .is_none_or(|b| &b.parent_id == target)
                {
                    return false;
                }
                // Cycle: target is a descendant of source.
                let mut singleton = std::collections::BTreeSet::new();
                singleton.insert(source.clone());
                if state.is_descendant_of_any(target, &singleton) {
                    return false;
                }
                true
            }
            E2ETransition::SplitBlock { block_id, position } => {
                let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
                let focused_in_main = state.focused_entity(holon_api::Region::Main);
                state.app_started
                    && state.is_properly_setup()
                    && focused_in_main == Some(block_id)
                    && state.block_state.blocks.get(block_id).is_some_and(|b| {
                        b.content_type == ContentType::Text && *position <= b.content_text().len()
                    })
                    && !state.layout_blocks.contains(block_id)
                    && state.is_descendant_of_any(block_id, &focus_roots)
            }
            E2ETransition::JoinBlock { block_id } => {
                let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
                let focused_in_main = state.focused_entity(holon_api::Region::Main);
                let base_ok = state.app_started
                    && state.is_properly_setup()
                    && focused_in_main == Some(block_id)
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .is_some_and(|b| b.content_type == ContentType::Text)
                    && !state.layout_blocks.contains(block_id)
                    && state.is_descendant_of_any(block_id, &focus_roots);
                if !base_ok {
                    return false;
                }
                // Case 1: previous text sibling exists → join into prev sibling.
                let prev_text = state
                    .previous_sibling(block_id)
                    .and_then(|prev| {
                        state
                            .block_state
                            .blocks
                            .get(&prev)
                            .map(|b| b.content_type == ContentType::Text)
                    })
                    .unwrap_or(false);
                if prev_text {
                    return true;
                }
                // Case 2: no previous sibling AND parent is a non-layout text block
                // → join into parent. Mirrors the production semantics added
                // for child→parent join.
                if state.previous_sibling(block_id).is_some() {
                    return false;
                }
                let parent_id = match state.block_state.blocks.get(block_id) {
                    Some(b) => b.parent_id.clone(),
                    None => return false,
                };
                if parent_id.is_no_parent() || parent_id.is_sentinel() {
                    return false;
                }
                let parent_is_text = state
                    .block_state
                    .blocks
                    .get(&parent_id)
                    .is_some_and(|b| b.content_type == ContentType::Text);
                parent_is_text && !state.layout_blocks.contains(&parent_id)
            }
            E2ETransition::UndoLastMutation => state.app_started && !state.undo_stack.is_empty(),
            E2ETransition::Redo => state.app_started && !state.redo_stack.is_empty(),
            E2ETransition::EmitMcpData => state.app_started,
            E2ETransition::AddPeer => {
                state.app_started && state.variant.enable_loro && state.peers.len() < 3
            }
            E2ETransition::PeerEdit { peer_idx, op } => {
                if !state.app_started || *peer_idx >= state.peers.len() {
                    return false;
                }
                let peer = &state.peers[*peer_idx];
                match op {
                    super::transitions::PeerEditOp::Create {
                        parent_stable_id, ..
                    } => parent_stable_id
                        .as_ref()
                        .is_none_or(|pid| peer.blocks.contains_key(pid)),
                    super::transitions::PeerEditOp::Update { stable_id, .. } => {
                        peer.blocks.contains_key(stable_id)
                    }
                    super::transitions::PeerEditOp::Delete { stable_id } => {
                        peer.blocks.contains_key(stable_id)
                    }
                }
            }
            E2ETransition::SyncWithPeer { peer_idx }
            | E2ETransition::MergeFromPeer { peer_idx } => {
                state.app_started && *peer_idx < state.peers.len()
            }
            E2ETransition::PeerCharEdit {
                peer_idx, block_id, ..
            } => {
                ReferenceState::mutable_text_enabled()
                    && state.app_started
                    && *peer_idx < state.peers.len()
                    && state.peers[*peer_idx].blocks.contains_key(block_id)
            }

            // Atomic editor primitives — gated to GPUI runs (PBT_ATOMIC_EDITOR=1).
            // Headless drivers have no `InputState` so these would just shadow
            // bookkeeping with no real-system counterpart.
            E2ETransition::FocusEditableText { block_id } => {
                if !ReferenceState::atomic_editor_enabled() {
                    return false;
                }
                // Require a *live-rendered* candidate. The ref-state's
                // descendant set leaks blocks that are in the model but
                // not yet (or no longer) in the GPUI tree (CDC lag, ghost
                // matview rows from inv10i, peer-pending). Filtering by
                // the live `BoundsRegistry` snapshot is what keeps the
                // SUT click from landing on a non-existent element.
                let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
                state.app_started
                    && state.is_properly_setup()
                    && state.current_focus(holon_api::Region::Main).is_some()
                    && state
                        .block_state
                        .blocks
                        .get(block_id)
                        .is_some_and(|b| b.content_type == ContentType::Text && !b.is_page())
                    && !state.layout_blocks.contains(block_id)
                    && state.is_descendant_of_any(block_id, &focus_roots)
                    && super::live_geometry::is_entity_rendered(block_id.as_str())
            }
            E2ETransition::MoveCursor { byte_position: _ }
            | E2ETransition::TypeChars { .. }
            | E2ETransition::DeleteBackward { .. }
            | E2ETransition::Blur => {
                ReferenceState::atomic_editor_enabled() && state.active_editor.is_some()
            }
            E2ETransition::PressKey { .. } => {
                ReferenceState::atomic_editor_enabled() && state.active_editor.is_some()
            }
        }
    }
}
