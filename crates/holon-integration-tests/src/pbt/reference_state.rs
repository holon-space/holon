//! Reference model for the PBT state machine.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::render_types::{Arg, RenderExpr};
use holon_api::{ContentType, EntityName, Region, Value};
use holon_frontend::editor_caret;

use super::action_actor_state::ActionActorState;
use super::file_adapter_state::FileAdapterState;
use super::mcp_server_actor_state::MCPServerActorState;
use super::query::{QuerySource, TestQuery, WatchSpec};
use super::reference_domain_state::ReferenceDomainState;
use super::ui_actor_state::UIActorState;
use holon_pbt_core::Wiring;

pub type ShadowInterpreter =
    holon_frontend::render_interpreter::RenderInterpreter<holon_frontend::ReactiveViewModel>;

fn fc(name: &str, args: Vec<Arg>) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: name.into(),
        args,
    }
}

fn named(name: &str, value: RenderExpr) -> Arg {
    Arg {
        name: Some(name.into()),
        value,
    }
}

fn pos(value: RenderExpr) -> Arg {
    Arg { name: None, value }
}

/// Valid render expressions for mutating render source blocks.
///
/// Each `RenderExpr` generates its Rhai source via `to_rhai()`.
/// The reference model stores the `RenderExpr` so we know exactly
/// what was written and can verify the rendered output.
pub fn valid_render_expressions() -> Vec<RenderExpr> {
    vec![
        // table()
        fc("table", vec![]),
        // list(#{item_template: render_entity()})
        fc(
            "list",
            vec![named("item_template", fc("render_entity", vec![]))],
        ),
        // tree(#{parent_id: col("parent_id"), sortkey: col("sequence"),
        //        item_template: render_entity(), creation_slot: true})
        // Exercises the virtual child / trailing slot path. `virtual_parent`
        // is intentionally omitted — `virtual_child_slot_from_arg` falls
        // back to the context row's `id` column (the focused block).
        fc(
            "tree",
            vec![
                named(
                    "parent_id",
                    RenderExpr::ColumnRef {
                        name: "parent_id".into(),
                    },
                ),
                named(
                    "sortkey",
                    RenderExpr::ColumnRef {
                        name: "sequence".into(),
                    },
                ),
                named("item_template", fc("render_entity", vec![])),
                named(
                    "creation_slot",
                    RenderExpr::Literal {
                        value: Value::Boolean(true),
                    },
                ),
            ],
        ),
        // columns(#{gap: 4, item_template: render_entity()})
        fc(
            "columns",
            vec![
                named(
                    "gap",
                    RenderExpr::Literal {
                        value: Value::Integer(4),
                    },
                ),
                named("item_template", fc("render_entity", vec![])),
            ],
        ),
        // list(#{item_template: row(text(col("content")))})
        fc(
            "list",
            vec![named(
                "item_template",
                fc(
                    "row",
                    vec![pos(fc(
                        "text",
                        vec![pos(RenderExpr::ColumnRef {
                            name: "content".into(),
                        })],
                    ))],
                ),
            )],
        ),
        // list(#{item_template: row(state_toggle(col("task_state")), editable_text(col("content")))})
        fc(
            "list",
            vec![named(
                "item_template",
                fc(
                    "row",
                    vec![
                        pos(fc(
                            "state_toggle",
                            vec![pos(RenderExpr::ColumnRef {
                                name: "task_state".into(),
                            })],
                        )),
                        pos(fc(
                            "editable_text",
                            vec![pos(RenderExpr::ColumnRef {
                                name: "content".into(),
                            })],
                        )),
                    ],
                ),
            )],
        ),
        // Mobile action-bar pattern used by inv-value-fn-provider-arg-variance/12/13 — drives the
        // value-fn providers (`focus_chain`, `chain_ops`) through the
        // real render pipeline so cache identity / arg variance can be
        // observed on the produced display tree.
        //
        // columns(#{collection: focus_chain(),
        //           item_template: columns(#{collection: chain_ops(col("level")),
        //                                    item_template: text(col("name"))})})
        fc(
            "columns",
            vec![
                named("collection", fc("focus_chain", vec![])),
                named(
                    "item_template",
                    fc(
                        "columns",
                        vec![
                            named(
                                "collection",
                                fc(
                                    "chain_ops",
                                    vec![pos(RenderExpr::ColumnRef {
                                        name: "level".into(),
                                    })],
                                ),
                            ),
                            named(
                                "item_template",
                                fc(
                                    "text",
                                    vec![pos(RenderExpr::ColumnRef {
                                        name: "name".into(),
                                    })],
                                ),
                            ),
                        ],
                    ),
                ),
            ],
        ),
    ]
}

/// The default render expression from `assets/default/index.org`:
/// `columns(#{gap: 4, item_template: render_entity()})`
pub fn default_root_render_expr() -> RenderExpr {
    fc(
        "columns",
        vec![
            named(
                "gap",
                RenderExpr::Literal {
                    value: Value::Integer(4),
                },
            ),
            named("item_template", fc("render_entity", vec![])),
        ],
    )
}

/// Backward-compatible string slice for code that still needs raw strings.
pub fn valid_render_expression_strings() -> Vec<String> {
    valid_render_expressions()
        .iter()
        .map(|e| e.to_rhai())
        .collect()
}

/// Look up which `RenderExpr` produced a given Rhai string.
/// Returns `None` if the string doesn't match any known expression.
pub fn render_expr_from_rhai(rhai: &str) -> Option<RenderExpr> {
    valid_render_expressions()
        .into_iter()
        .find(|e| e.to_rhai() == rhai)
}

/// A test entity profile that generates its own YAML and knows how to check
/// whether a block matches its variant condition.
pub struct TestEntityProfile {
    pub profile_name: &'static str,
    pub field_name: &'static str,
}

impl TestEntityProfile {
    fn to_yaml(&self) -> String {
        format!(
            "entity_name: block\ncomputed:\n  has_{field}: \"= {field} != ()\"\nvariants:\n  - name: {name}\n    priority: 1\n    condition: \"= has_{field}\"\n    render: 'row(editable_text(col(\"content\")))'\n  - name: default\n    priority: -1\n    render: 'row(editable_text(col(\"content\")))'",
            field = self.field_name,
            name = self.profile_name,
        )
    }
}

/// Index 0 in VALID_PROFILE_YAMLS is the "no variants" YAML (always "default").
/// Indices 1..N correspond to TEST_PROFILES[0..N-1].
pub const TEST_PROFILES: &[TestEntityProfile] = &[
    TestEntityProfile {
        profile_name: "task",
        field_name: "task_state",
    },
    TestEntityProfile {
        profile_name: "has_content",
        field_name: "content",
    },
];

const NO_VARIANTS_YAML: &str = "entity_name: block\ncomputed: {}\nvariants:\n  - name: default\n    priority: -1\n    render: 'row(editable_text(col(\"content\")))'";

pub static VALID_PROFILE_YAMLS: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
    let mut yamls = vec![NO_VARIANTS_YAML.to_string()];
    for tep in TEST_PROFILES {
        yamls.push(tep.to_yaml());
    }
    yamls
});

/// Typed classification of layout block IDs in index.org.
///
/// Layout blocks are split into three categories with different mutation rules:
/// - **headline_ids**: The text headline blocks that parent query/render sources.
///   These can have content, task_state, priority, tags mutated.
/// - **query_source_ids**: PRQL/GQL/SQL source blocks. These are truly immutable
///   because changing them would break `initial_widget()`.
/// - **render_source_ids**: Render DSL source blocks. These can have their content
///   changed to any valid render expression.
#[derive(Debug, Clone, Default)]
pub struct LayoutBlockInfo {
    pub headline_ids: HashSet<EntityUri>,
    pub query_source_ids: HashSet<EntityUri>,
    pub render_source_ids: HashSet<EntityUri>,
}

impl LayoutBlockInfo {
    /// Returns true if the block is part of the layout at all.
    pub fn contains(&self, id: &EntityUri) -> bool {
        self.headline_ids.contains(id)
            || self.query_source_ids.contains(id)
            || self.render_source_ids.contains(id)
    }

    /// Returns true if the block must never be mutated (query sources only).
    pub fn is_immutable(&self, id: &EntityUri) -> bool {
        self.query_source_ids.contains(id)
    }

    /// Returns true if the block is focusable — i.e. it has an EditableText node.
    /// Source blocks (query/render) are NOT focusable. Headline blocks (parents
    /// of source blocks) ARE focusable in the current reference model because
    /// the PBT uses them as navigation targets; marking them non-focusable
    /// would break ClickBlock generation entirely (see note in the editable
    /// transition generation).
    pub fn is_focusable(&self, id: &EntityUri) -> bool {
        !self.query_source_ids.contains(id) && !self.render_source_ids.contains(id)
    }

    /// Remove a block from all sets.
    pub fn remove(&mut self, id: &EntityUri) {
        self.headline_ids.remove(id);
        self.query_source_ids.remove(id);
        self.render_source_ids.remove(id);
    }
}

/// Block-related state that is affected by undo/redo operations.
/// Extracted so snapshots can be taken via `.clone()` before UI mutations.
#[derive(Debug, Clone)]
pub struct BlockState {
    /// Canonical block state (using production Block struct).
    ///
    /// `BTreeMap` (not `HashMap`) so iteration order is deterministic across
    /// process instantiations. The PBT canonicalizer (`apply_mutation`,
    /// `recanon_and_rebuild`) builds a `Vec<Block>` from these values and the
    /// resulting sequence numbers depend on iteration order — `HashMap`'s
    /// random seed made the same proptest seed produce different reference
    /// states across runs.
    pub blocks: BTreeMap<EntityUri, Block>,

    /// Mapping of block_id → doc_uri (persists even after blocks are deleted).
    /// `BTreeMap` for the same determinism reason as `blocks`.
    pub block_documents: BTreeMap<EntityUri, EntityUri>,

    /// ID counter for generating unique block IDs
    pub next_id: usize,
}

impl BlockState {
    /// Return a clone with every block's `id`/`parent_id` and the
    /// `block_documents` keys remapped through `map` (synthetic doc URI →
    /// real SUT UUID). URIs absent from `map` (i.e. all content-block IDs,
    /// which the ref and SUT already share) pass through unchanged.
    ///
    /// Instead of every invariant translating IDs at each comparison point,
    /// the reference model is mapped *once* into the SUT's ID space so
    /// capability-bound invariant bodies can compare directly. Only doc URIs
    /// differ, and only block `id`/`parent_id` + `block_documents` keys carry
    /// them, so this resolves exactly `block.id`, `block.parent_id`, and the
    /// `block_documents` keys.
    pub fn remapped_doc_uris(&self, map: &BTreeMap<EntityUri, EntityUri>) -> BlockState {
        let resolve = |u: &EntityUri| map.get(u).cloned().unwrap_or_else(|| u.clone());
        let blocks = self
            .blocks
            .values()
            .map(|b| {
                let mut b = b.clone();
                b.id = resolve(&b.id);
                b.parent_id = resolve(&b.parent_id);
                (b.id.clone(), b)
            })
            .collect();
        let block_documents = self
            .block_documents
            .iter()
            .map(|(id, doc)| (resolve(id), doc.clone()))
            .collect();
        BlockState {
            blocks,
            block_documents,
            next_id: self.next_id,
        }
    }
}

/// Reference state tracking all expected data (uses production Block struct)
#[derive(Debug, Clone)]
pub struct ReferenceState {
    /// Tier-1 domain fragment (ADR 0004 Phase 2): block tree, layout/profile
    /// classification, author-intent render config, seed profile, block ops.
    /// Extracted so it can be the single domain fragment shared across wirings.
    pub domain: ReferenceDomainState,

    /// Action-engine actor fragment (ADR 0004/0006 Phase 4): lifecycle flag,
    /// doc-id allocator, last-transition tag, undo/redo stacks. Vanishes when
    /// the action engine isn't wired.
    pub action: ActionActorState,

    /// MCP server actor fragment (ADR 0004/0006 Phase 4): active query watches.
    /// Vanishes when the MCP server isn't wired.
    pub mcp: MCPServerActorState,

    /// Org/Markdown adapter file-state fragment (ADR 0004 Phase 5): the
    /// doc_uri -> filename mapping. An adapter concern (how org/markdown
    /// persist a document on disk), distinct from domain identity.
    pub files: FileAdapterState,

    /// Tier-3 UI actor fragment (ADR 0004/0006 Phase 3): navigation history,
    /// pins, per-region focus + cursor, view selection, drawer/toggle
    /// open-state, active-editor mirror. Extracted so a non-UI wiring drops
    /// the whole fragment instead of carrying dead fields.
    pub ui: UIActorState,

    /// Runtime for async operations
    pub runtime: Arc<tokio::runtime::Runtime>,

    /// Pre-startup directories created (relative paths)
    pub pre_startup_directories: Vec<String>,

    /// Whether git has been initialized
    pub git_initialized: bool,

    /// Whether jj has been initialized
    pub jj_initialized: bool,

    /// Number of pre-startup org files created (for weighting StartApp)
    pub pre_startup_file_count: usize,

    /// The wiring manifest this reference run was built for (which storage
    /// adapters, sync adapters, and actors are present). Drives the
    /// `enable_loro()` capability check and per-transition / per-invariant
    /// `RequiredWiring` gating (ADR 0007).
    pub wiring: Wiring,

    /// The capability set the SUT supplies, when the SUT is a composed `CapMap`.
    /// `None` = unrestricted: a concrete SUT (`E2ESut`) provides every cap, or this is
    /// a non-composed run, so the cap gate passes everything and the alphabet behaves
    /// exactly as before. `Some(set)` = a composed/partial SUT, so transitions whose
    /// [`TransitionFactory::required_caps`] aren't all present are gated out of the
    /// alphabet — the cap-analog of [`wiring`](Self::wiring)/`RequiredWiring` (PCG-2).
    pub cap_set: Option<holon_pbt_core::composition::CapSet>,

    /// Whether a **real editor** (a live `InputState` driven by the GPUI/TUI
    /// `UserDriver`) — not the headless `HeadlessEditorMirror` — drives the SUT.
    /// Set by the real-editor driver harness (`phased.rs`), which builds the
    /// reference state directly. When true, [`Self::blur_active_editor`] commits
    /// the editor's dirty buffer to block content on blur, mirroring prod's
    /// `on_blur` → `set_field("content")`. Replaces the former process-global
    /// `PBT_REAL_EDITOR` env gate — the property now lives on the state the
    /// driver constructs, so it is deterministic and capture/replay-faithful
    /// without an env-var side channel. Headless slices leave it `false`.
    pub real_editor: bool,

    /// Loro-only peer instances for multi-instance sync testing.
    pub peers: Vec<PeerRefState>,

    /// Shadow interpreter resolved from FluxDI — source of truth for widget
    /// names and render DSL parsing.
    pub interpreter: Arc<ShadowInterpreter>,
}

/// Reference state for a Loro-only peer.
#[derive(Debug, Clone)]
pub struct PeerRefState {
    pub peer_id: u64,
    pub blocks: HashMap<String, super::peer_ops::PeerBlock>,
    /// Stable IDs this peer has deleted since its last sync with the
    /// primary. Propagated by `SyncWithPeer`/`MergeFromPeer` so the
    /// primary's reference block map reflects the delete the production
    /// controller just applied via `subscribe_root`.
    pub deleted_stable_ids: std::collections::HashSet<String>,
    /// Stable IDs explicitly modified by PeerEdit::Update since AddPeer.
    /// Used by `merge_peer_blocks_into_primary` to distinguish peer edits
    /// from inherited-at-AddPeer blocks.
    pub modified_stable_ids: std::collections::HashSet<String>,
    /// Stable IDs created by PeerEdit::Create since the last sync. Only
    /// these are added to the primary on merge — inherited-at-AddPeer
    /// blocks the primary may have since deleted must NOT be re-added,
    /// because the actual Loro CRDT keeps primary-side deletes.
    pub created_stable_ids: std::collections::HashSet<String>,
    /// Snapshot of block content at AddPeer time (or after the last sync).
    /// Used by `merge_peer_blocks_into_primary` to detect concurrent
    /// primary+peer edits on the same block: if both `existing.content` and
    /// `pb.content` diverged from the baseline, Loro's text CRDT keeps both
    /// insertions, so we need a real CRDT merge instead of naive LWW.
    pub baseline_contents: HashMap<String, String>,
}

/// Cursor position within a focused block. Tracks line and column to predict
/// whether arrow keys cause cross-block navigation or intra-block movement.
#[derive(Debug, Clone, Copy)]
pub struct CursorPosition {
    pub line: usize,
    pub column: usize,
}

impl CursorPosition {
    pub fn start() -> Self {
        Self { line: 0, column: 0 }
    }
}

/// Mirror of the GPUI editor's live `InputState`: the in-memory text of the
/// currently focused EditableText, plus the cursor offset within that text.
/// Diverges from `block.content` whenever the user has typed/deleted without
/// blurring — exactly the divergence that surfaces split-with-pending-edit
/// (and similar) bugs.
#[derive(Debug, Clone)]
pub struct ActiveEditor {
    pub block_id: EntityUri,
    /// What the GPUI `InputState.text()` currently shows.
    pub in_memory_content: String,
    /// Byte offset of the caret within `in_memory_content`.
    pub cursor_byte: usize,
    /// True once modeled typing/deleting touched `in_memory_content` since
    /// the editor opened (or since the last commit). Mirrors what prod's
    /// commit paths observe: a DIRTY editor's text is user-authored and
    /// commits on blur / at a structural commit point; a clean editor whose
    /// text merely diverged from `block.content` is STALE against an
    /// external change (prod's data subscription refreshes idle editors) —
    /// committing it would write old text into the ref.
    pub dirty: bool,
}

impl ActiveEditor {
    /// Insert text at the cursor and advance. Delegates caret/text math to the
    /// **shared** `editor_caret` primitive — the SAME one the SUT's
    /// `InMemEditorComponent` drives — so ref and SUT cannot diverge on the
    /// text primitive itself (multibyte-safe).
    pub fn type_chars(&mut self, text: &str) {
        debug_assert!(self.cursor_byte <= self.in_memory_content.len());
        self.cursor_byte =
            editor_caret::insert_at(&mut self.in_memory_content, self.cursor_byte, text);
        self.dirty = true;
    }

    /// Delete `count` chars before the cursor (Backspace ×count). Stops at start.
    pub fn delete_backward(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        self.cursor_byte =
            editor_caret::delete_back(&mut self.in_memory_content, self.cursor_byte, count);
        self.dirty = true;
    }

    /// Move the caret to a clamped byte position. Snaps to the nearest char
    /// boundary at or before the target (a raw byte target — home/end/a click —
    /// may land mid-codepoint); `clamp_boundary` keeps the caret legal.
    pub fn move_cursor(&mut self, position: usize) {
        self.cursor_byte = editor_caret::clamp_boundary(&self.in_memory_content, position);
    }
}

/// Navigation history for a region (for back/forward navigation)
#[derive(Debug, Clone)]
pub struct NavigationHistory {
    /// History entries: None = home view, Some(id) = focused on block
    pub entries: Vec<Option<EntityUri>>,
    /// Current cursor position in history
    pub cursor: usize,
}

/// One open `navigation_history` row (`closed_at IS NULL`). Mirrors the
/// open-rows projection that drives the `focus_roots` matview.
///
/// `block_id = None` represents a home row (block_id NULL in SQL); home
/// rows are kept here because they bump `next_history_id` and contribute
/// to move-to-top dedup, but they are excluded from `expected_focus_root_ids`
/// (they're filtered out by the consumer GQL JOIN on `root.id = fr.root_id`).
#[derive(Debug, Clone)]
pub struct OpenPinEntry {
    pub history_id: i64,
    pub block_id: Option<EntityUri>,
    pub added_ts_logical: u64,
}

impl Default for NavigationHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl NavigationHistory {
    pub fn new() -> Self {
        Self {
            entries: vec![None],
            cursor: 0,
        }
    }

    pub fn can_go_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.cursor < self.entries.len().saturating_sub(1)
    }

    pub fn current_focus(&self) -> Option<EntityUri> {
        self.entries.get(self.cursor).cloned().flatten()
    }
}

/// Witness that a [`ReferenceState`]'s ids live in the SUT's id space — either
/// [`ReferenceState::with_resolved_doc_uris`] has run (synthetic `block::split-N`
/// / `block:ref-doc-N` placeholders remapped to the SUT's real ids), or the ref
/// was built over a backend that mints no new ids (see [`Resolved::identity`]).
///
/// The capability-bound comparison entry points (`reference_state_ref_caps`,
/// `run_with_seeded_ref`) require this witness, so an **unresolved** reference
/// state can no longer be compared against the SUT. The historical false
/// divergence — comparing a synthetic `block::split-N` against a real UUID
/// because someone forgot to reconcile — is now a *compile error* rather than a
/// runtime assertion (cf. `exp3_unreconciled_split_is_caught`, which simulates
/// the *under*-reconciled case with an empty map).
#[derive(Clone)]
pub struct Resolved<T>(T);

impl<T> Resolved<T> {
    /// The resolved value.
    pub fn get(&self) -> &T {
        &self.0
    }

    /// Consume the witness, yielding the resolved value.
    pub fn into_inner(self) -> T {
        self.0
    }

    /// Map the inner value while carrying the witness forward — e.g.
    /// `Resolved<ReferenceState>` → `Resolved<Arc<ReferenceState>>`. The closure
    /// only repackages an already-resolved value, so the witness still holds.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Resolved<U> {
        Resolved(f(self.0))
    }

    /// Crate-internal mutable access for post-resolution seed preparation
    /// (`inject_scaffold_seed`), whose injected ids are themselves real
    /// SUT-keyed scaffold ids — so the value stays resolved.
    pub(crate) fn inner_mut(&mut self) -> &mut T {
        &mut self.0
    }
}

impl Resolved<ReferenceState> {
    /// Witness that no id resolution is needed: the reference state was built over
    /// a backend that mints no fresh ids (seed/started refs, or the counter-sync
    /// `MemoryBackend` whose `align_ids` keeps mints in lockstep with the oracle),
    /// so the synthetic and real id spaces already coincide.
    pub fn identity(state: ReferenceState) -> Self {
        Resolved(state)
    }
}

impl ReferenceState {
    /// Return a [`Resolved`] clone of this reference state with its block tree
    /// remapped into the SUT's ID space via `map` (synthetic doc URI → real UUID).
    /// Capability-bound invariant bodies run against this resolved view so
    /// they can compare block IDs/parents directly against the SUT without
    /// any per-comparison resolution.
    ///
    /// Scope: currently remaps `block_state` only (covers the block-tree /
    /// SQL-projection invariants). Focus/navigation/watch fields also carry
    /// doc URIs and must be added here as those invariant bodies migrate.
    pub fn with_resolved_doc_uris(
        &self,
        map: &BTreeMap<EntityUri, EntityUri>,
    ) -> Resolved<ReferenceState> {
        let resolve = |u: &EntityUri| map.get(u).cloned().unwrap_or_else(|| u.clone());
        let mut resolved = self.clone();
        resolved.domain.block_state = self.domain.block_state.remapped_doc_uris(map);

        // Fields the matview/ViewModel bodies read alongside `block_state`
        // can themselves reference synthetic doc URIs (a pinned page's
        // block_id IS its doc URI; layout/profile scaffolding can hang off
        // a doc block). Remap them into SUT ID space too so the resolved
        // view is uniformly SUT-keyed.
        for pins in resolved.ui.user.open_pins.values_mut() {
            for pin in pins.iter_mut() {
                if let Some(id) = pin.block_id.as_ref() {
                    pin.block_id = Some(resolve(id));
                }
            }
        }
        resolved.domain.layout_blocks = LayoutBlockInfo {
            headline_ids: self
                .domain
                .layout_blocks
                .headline_ids
                .iter()
                .map(resolve)
                .collect(),
            query_source_ids: self
                .domain
                .layout_blocks
                .query_source_ids
                .iter()
                .map(resolve)
                .collect(),
            render_source_ids: self
                .domain
                .layout_blocks
                .render_source_ids
                .iter()
                .map(resolve)
                .collect(),
        };
        resolved.domain.profile_block_ids =
            self.domain.profile_block_ids.iter().map(resolve).collect();
        // Navigation focus per region can itself be a doc URI (a region drilled
        // into a document). Remap every history entry so the resolved view's
        // `current_focus(region)` is SUT-keyed — `inv-navigation-focus` compares
        // it directly against the `current_focus` matview (real ids).
        for history in resolved.ui.tab.navigation_history.values_mut() {
            for entry in history.entries.iter_mut() {
                if let Some(id) = entry.as_ref() {
                    *entry = Some(resolve(id));
                }
            }
        }
        // The active editor's block can be a split-created block stored under a
        // synthetic `block::split-N` key; the rendered window tracks it under
        // the real UUID. Remap so `inv-displayed-text` matches the geometry
        // element's (real-UUID) `entity_id` directly — the resolved-view
        // replacement for the inline `reverse_map`.
        if let Some(editor) = resolved.ui.tab.active_editor.as_mut() {
            editor.block_id = resolve(&editor.block_id);
        }
        Resolved(resolved)
    }

    pub fn new(wiring: Wiring, interpreter: Arc<ShadowInterpreter>) -> Self {
        Self {
            domain: ReferenceDomainState::new(),
            action: ActionActorState::new(),
            mcp: MCPServerActorState::new(),
            files: FileAdapterState::new(),
            ui: UIActorState::new(),
            runtime: Arc::new(tokio::runtime::Runtime::new().unwrap()),
            pre_startup_directories: Vec::new(),
            git_initialized: false,
            jj_initialized: false,
            pre_startup_file_count: 0,
            wiring,
            cap_set: None,
            real_editor: false,
            peers: Vec::new(),
            interpreter,
        }
    }

    /// Set the composed SUT's capability set (the cap gate's RHS). The composed harness
    /// calls this with `caps.cap_set()` so the alphabet auto-narrows to the transitions
    /// whose caps the `CapMap` actually supplies. Concrete-SUT runs leave it `None`.
    pub fn with_cap_set(mut self, cap_set: holon_pbt_core::composition::CapSet) -> Self {
        self.cap_set = Some(cap_set);
        self
    }

    /// Whether the active SUT supplies every cap in `required` — the cap gate mirroring
    /// `RequiredWiring::satisfied_by(&self.wiring)`. Unrestricted (`cap_set == None`)
    /// always passes, so concrete-SUT runs gate nothing (regression-safe). **Necessary,
    /// not sufficient**, exactly like the wiring gate.
    pub fn caps_available(&self, required: &[holon_pbt_core::composition::CapId]) -> bool {
        match &self.cap_set {
            None => true,
            Some(set) => required.iter().all(|cap| set.contains(cap)),
        }
    }

    /// Whether the Loro CRDT storage adapter is wired (the reference-side
    /// capability check). Inherent mirror of the `RefLifecycle::enable_loro`
    /// trait method so transition bodies that hold a concrete
    /// `&ReferenceState` can read it without importing the trait.
    pub fn enable_loro(&self) -> bool {
        self.wiring
            .has_storage(holon_pbt_core::StorageAdapter::Loro)
    }

    /// Whether this reference owns an editor buffer carrying uncommitted text —
    /// the headless atomic-editor capability (the single editor-transition gate;
    /// see [`RefLifecycle::has_editor_buffer`]). Inherent mirror of the trait
    /// method so transition bodies holding a concrete `&ReferenceState` can read
    /// it without importing the trait. Derived from the wiring's UI actor (the
    /// editor's `InputState`/buffer host), not Loro-as-storage or an env var.
    pub fn has_editor_buffer(&self) -> bool {
        self.wiring.has_actor(holon_pbt_core::Actor::UI)
    }

    pub fn mutable_text_enabled() -> bool {
        std::env::var("PBT_MUTABLE_TEXT")
            .ok()
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false)
    }

    /// Close the active editor, committing its pending text first when a real
    /// editor is driving (see [`Self::real_editor`]). The commit is idempotent
    /// under Loro (per-keystroke writes already committed it) and a no-op when
    /// no editor is active, so call sites can swap a bare `active_editor = None`
    /// for this unconditionally.
    pub fn blur_active_editor(&mut self) {
        // Dirty-gated: only user-authored pending text commits on an
        // authority move (prod commits via the focus-binding's
        // authority-left arm — deterministic, window-activation-independent).
        // A clean mirror that merely diverged from block.content is stale
        // against an external change and must not be committed.
        if self.real_editor && self.ui.tab.active_editor.as_ref().is_some_and(|e| e.dirty) {
            self.commit_active_editor_if_changed();
        }
        self.ui.tab.active_editor = None;
    }

    /// Commit `active_editor.in_memory_content` to the underlying block if
    /// it diverged from the DB. Called at the start of any chord transition
    /// (Enter/Backspace/Tab/...) to encode the *intended* contract:
    /// chord-on-active-editor commits pending edits before mutating
    /// structure. Returns whether a commit was needed (for diagnostics).
    ///
    /// The committed value is normalized through
    /// `normalize_content_for_org_roundtrip` to mirror the trim that
    /// `SqlOperationProvider::trimmed_content` applies on the prod write
    /// path. Without this, a trailing-whitespace state in the editor
    /// (e.g. `"LM "` after backspacing past `"LM lX8G"` 's last visible
    /// char) leaves ref `block.content` at `"LM "` while prod's SQL
    /// projection has trimmed to `"LM"`.
    pub fn commit_active_editor_if_changed(&mut self) -> bool {
        let Some(editor) = self.ui.tab.active_editor.as_ref() else {
            return false;
        };
        let block_id = editor.block_id.clone();
        let in_memory = editor.in_memory_content.clone();
        let Some(block) = self.domain.block_state.blocks.get_mut(&block_id) else {
            return false;
        };
        let normalized =
            super::types::normalize_content_for_org_roundtrip(&in_memory, block.content_type);
        if block.content == normalized {
            if let Some(e) = self.ui.tab.active_editor.as_mut() {
                e.dirty = false;
            }
            return false;
        }
        block.content = normalized;
        if let Some(e) = self.ui.tab.active_editor.as_mut() {
            e.dirty = false;
        }
        true
    }

    pub fn current_focus(&self, region: Region) -> Option<EntityUri> {
        self.ui
            .tab
            .navigation_history
            .get(&region)
            .and_then(|h| h.current_focus())
    }

    pub fn can_go_back(&self, region: Region) -> bool {
        self.ui
            .tab
            .navigation_history
            .get(&region)
            .map(|h| h.can_go_back())
            .unwrap_or(false)
    }

    /// If `block_id` is the focused entity in any region, reset the cursor to start.
    /// Called after mutations that change block content — the real editor would
    /// reposition the cursor (blur/refocus cycle), so the reference model must too.
    pub fn reset_cursor_if_focused(&mut self, block_id: &EntityUri) {
        for (region, focused_id) in &self.ui.tab.focused_entity_id {
            if focused_id == block_id {
                self.ui
                    .tab
                    .focused_cursor
                    .insert(*region, CursorPosition::start());
            }
        }
    }

    /// If `block_id` is the focused entity in any region, clear the focus
    /// (the block was deleted — can't be focused anymore).
    pub fn clear_focus_if_deleted(&mut self, block_id: &EntityUri) {
        self.ui.tab.focused_entity_id.retain(|_, id| id != block_id);
        // focused_cursor entries for removed regions will be stale but harmless

        // The GLOBAL in-memory focus mirror (ADR 0010 engine focus) is a
        // separate field from the per-region map above; prod clears it via
        // `maybe_clear_focus_on_delete` — mirror that here or
        // inv-focus-matches-ref expects focus on a deleted block.
        if self.ui.tab.focused_block.as_ref() == Some(block_id) {
            self.ui.tab.focused_block = None;
        }

        // Deleting a block CLOSES its editor — drop without committing
        // (the block is gone; committing dirty text to it is meaningless,
        // and a lingering ActiveEditor makes inv-editor-text-matches-ref
        // compare a ghost editor against whatever stale cell the SUT still
        // caches for the deleted block — the slash-command "/delete"
        // residue face, 2026-06-11).
        if self
            .ui
            .tab
            .active_editor
            .as_ref()
            .is_some_and(|e| &e.block_id == block_id)
        {
            self.ui.tab.active_editor = None;
        }
    }

    /// Whether any region currently has a focused entity (required for ArrowNavigate).
    pub fn has_focus(&self) -> bool {
        !self.ui.tab.focused_entity_id.is_empty()
    }

    /// Get the focused entity in a region (set by ClickBlock).
    pub fn focused_entity(&self, region: Region) -> Option<&EntityUri> {
        self.ui.tab.focused_entity_id.get(&region)
    }

    pub fn can_go_forward(&self, region: Region) -> bool {
        self.ui
            .tab
            .navigation_history
            .get(&region)
            .map(|h| h.can_go_forward())
            .unwrap_or(false)
    }

    pub fn current_view(&self) -> String {
        self.ui.user.current_view.clone()
    }

    /// Returns expected query results for a watch using the TestQuery evaluator.
    pub fn query_results(&self, watch_spec: &WatchSpec) -> Vec<HashMap<String, Value>> {
        watch_spec.query.evaluate(&self.domain.block_state.blocks)
    }

    /// Check if index.org exists with the structure required by initial_widget().
    /// Generate a synthetic `block:ref-doc-N` URI for a new document and bump the counter.
    pub fn next_synthetic_doc_uri(&mut self) -> EntityUri {
        let uri = EntityUri::block(&format!("ref-doc-{}", self.action.next_doc_id));
        self.action.next_doc_id += 1;
        uri
    }

    /// Find a page block by its title (first line of content, e.g. "index").
    pub fn doc_uri_by_name(&self, title: &str) -> Option<EntityUri> {
        self.domain
            .block_state
            .blocks
            .values()
            .find(|b| b.is_page() && b.title() == title)
            .map(|b| b.id.clone())
    }

    /// Whether the system has a valid root layout (from seed blocks or user-written index.org).
    /// Used to gate render_entity, ReactiveEngine, and ViewModel checks.
    pub fn is_properly_setup(&self) -> bool {
        !self.domain.layout_blocks.query_source_ids.is_empty() || self.has_user_index_org()
    }

    /// Whether the user has written an index.org with query+render blocks.
    /// Used to gate block comparison invariants (seed blocks don't round-trip through org files).
    pub fn has_user_index_org(&self) -> bool {
        let index_doc_uri = match self.doc_uri_by_name("index") {
            Some(uri) => uri,
            None => return false,
        };

        let root_blocks: Vec<&Block> = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == index_doc_uri)
            .collect();

        root_blocks.iter().any(|root_block| {
            self.domain.block_state.blocks.values().any(|child| {
                child.parent_id == root_block.id
                    && child.content_type == ContentType::Source
                    && child
                        .source_language
                        .as_ref()
                        .and_then(|sl| sl.as_query())
                        .is_some()
            })
        })
    }

    /// Get the first root layout block ID from index.org (a heading with a query source child).
    pub fn root_layout_block_id(&self) -> Option<EntityUri> {
        let index_doc_uri = self.doc_uri_by_name("index")?;
        self.domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == index_doc_uri)
            .find(|root_block| {
                self.domain.block_state.blocks.values().any(|child| {
                    child.parent_id == root_block.id
                        && child.content_type == ContentType::Source
                        && child
                            .source_language
                            .as_ref()
                            .and_then(|sl| sl.as_query())
                            .is_some()
                })
            })
            .map(|b| b.id.clone())
    }

    /// Whether the active main-panel layout renders `block_id` as a
    /// `draggable(...)` widget — the precondition for drag-and-drop, which
    /// needs a draggable source in the rendered tree to grab.
    ///
    /// Rather than guess from the render expression's shape, this renders the
    /// block's row through the active item template with the shadow
    /// interpreter (the same `BuilderServices::interpret` the SUT uses) and
    /// walks the resulting ViewModel for a `draggable` node — mirroring the
    /// SUT's `drop_entity` walk. The default layout's `render_entity()` item
    /// template resolves the block profile to `column(row(draggable(...)…))`
    /// synchronously; a custom `index.org` render (`row(text(...))`, a profile
    /// whose render drops the draggable, …) produces no draggable, so drag is
    /// not generated against it.
    pub fn block_renders_draggable(&self, block_id: &EntityUri) -> bool {
        self.main_layout_renders_widget(block_id, &["draggable"])
    }

    /// Whether the active main-panel layout's item template renders `block_id`'s
    /// row with any of `widgets` (by `widget_name`). Renders the row through the
    /// shadow interpreter (the same `BuilderServices::interpret` the SUT uses)
    /// and walks the resulting `ViewModel`. Returns `false` when no main-panel
    /// render template is tracked (the template-interactivity axis is only
    /// consulted for user `index.org` layouts, which always have one).
    fn main_layout_renders_widget(&self, block_id: &EntityUri, widgets: &[&str]) -> bool {
        use holon_frontend::reactive::BuilderServices;

        let Some(expr) = self
            .main_panel_render_expr()
            .or_else(|| self.root_render_expr())
        else {
            return false;
        };
        let Some(item_template) = holon_frontend::reactive_view_model::extract_item_template(expr)
        else {
            return false;
        };
        let Some(block) = self.domain.block_state.blocks.get(block_id) else {
            return false;
        };
        let row = std::sync::Arc::new(block_to_data_row(block));
        let ctx = holon_frontend::RenderContext::default().with_row(row);
        let vm = self.interpret(&item_template, &ctx);
        view_model_has_widget(&vm, widgets)
    }

    /// The active main-panel layout query, as a [`TestQuery`].
    ///
    /// Default layout (no user `index.org`) → the navigation-aware
    /// [`QuerySource::FocusRootDescendants`] (GQL `focus_root` + `CHILD_OF*0..20`).
    /// A user `index.org` → the [`QuerySource`] recovered from its main-panel
    /// query source block (via [`QuerySource::recognize`]), bound to the layout
    /// block as the navigation-blind `from children` context.
    pub fn active_main_query(&self) -> TestQuery {
        if !self.has_user_index_org() {
            return TestQuery::layout(QuerySource::FocusRootDescendants {
                region: "main".to_string(),
                max_depth: 20,
            });
        }
        let source = self
            .root_layout_block_id()
            .and_then(|layout_block| {
                self.domain
                    .block_state
                    .blocks
                    .values()
                    .find(|b| {
                        b.parent_id == layout_block
                            && b.content_type == ContentType::Source
                            && b.source_language
                                .as_ref()
                                .and_then(|sl| sl.as_query())
                                .is_some()
                    })
                    .map(|query_block| {
                        let lang = query_block
                            .source_language
                            .as_ref()
                            .and_then(|sl| sl.as_query())
                            .expect("query source block has a query language");
                        QuerySource::recognize(&query_block.content, lang, &layout_block)
                    })
            })
            .unwrap_or(QuerySource::AllBlocks);
        TestQuery::layout(source)
    }

    /// Block ids the active main-panel layout renders — its query's rendered set.
    /// The faithful replacement for the `is_descendant_of_any(focus_roots)`
    /// proxy: it agrees with the default layout (focus-root descendants) and is
    /// correct for custom layouts (a `from children` layout renders only the
    /// layout block's direct children, an all-blocks layout renders everything).
    pub fn main_rendered_block_ids(&self) -> BTreeSet<EntityUri> {
        let query = self.active_main_query();
        let mut focus_roots = std::collections::BTreeMap::new();
        focus_roots.insert(
            "main".to_string(),
            self.expected_focus_root_ids(Region::Main),
        );
        query
            .rendered_block_ids(&self.domain.block_state.blocks, &focus_roots)
            .into_iter()
            .collect()
    }

    /// Whether the active layout's item template renders blocks interactively —
    /// i.e. with a widget a block-interaction transition can dispatch against.
    ///
    /// The default layout renders every block through the block entity profile's
    /// `render_entity()` (which attaches operations, a `draggable`, and an
    /// `editable_text`) — interactive by construction. A user `index.org`
    /// renders through an explicit template that may be static (`row(text(…))`,
    /// no operations) or interactive; we walk the actual template to decide.
    fn layout_renders_interactively(&self, block_id: &EntityUri) -> bool {
        if !self.has_user_index_org() {
            return true;
        }
        self.main_layout_renders_widget(
            block_id,
            &[
                "editable_text",
                "draggable",
                "state_toggle",
                "rendered_text",
            ],
        )
    }

    /// Whether a block-interaction transition (indent / chord / toggle / …) can
    /// dispatch against `block_id`: it must be in the active layout's rendered
    /// set AND rendered with an interactive widget. Replaces the
    /// `blocks_render_interactively()` stopgap (`!has_user_index_org()`) with a
    /// faithful rendered-set ∩ template-interactivity computation.
    pub fn renders_block_interactively(&self, block_id: &EntityUri) -> bool {
        self.main_rendered_block_ids().contains(block_id)
            && self.layout_renders_interactively(block_id)
    }

    /// Get the active `RenderExpr` for the root layout's render source block.
    /// Returns `None` if no render source is tracked.
    pub fn root_render_expr(&self) -> Option<&RenderExpr> {
        let root_id = self.root_layout_block_id()?;
        // Find the render source block that is a child of the root layout
        self.domain
            .layout_blocks
            .render_source_ids
            .iter()
            .find(|id| {
                self.domain
                    .block_state
                    .blocks
                    .get(*id)
                    .map(|b| b.parent_id == root_id)
                    .unwrap_or(false)
            })
            .and_then(|id| self.domain.render_expressions.get(id))
    }

    /// Name of the active render expression for `region` (e.g. "tree",
    /// "outline", "list"). Used by `build_reference_navigator` to pick
    /// the right `CollectionNavigator` shape for arrow-key navigation.
    pub fn active_render_expr_name(&self, _: Region) -> Option<String> {
        // For now, use the main panel's render expression (region is ignored
        // because the PBT currently only has one navigable region).
        let expr = self.main_panel_render_expr().or(self.root_render_expr())?;
        match expr {
            RenderExpr::FunctionCall { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    /// Build a reference-state `CollectionNavigator` for `region` to mirror
    /// what production's arrow-key handler would walk. Tree- and outline-
    /// layouts use `TreeNavigator`; everything else uses `ListNavigator`.
    pub fn build_reference_navigator(
        &self,
        region: Region,
    ) -> Option<Box<dyn holon_frontend::navigation::CollectionNavigator>> {
        use holon_frontend::navigation::{ListNavigator, TreeNavigator};

        let focus_id = self.current_focus(region)?;

        let children = self.sorted_children_of(&focus_id);
        let child_ids: Vec<EntityUri> = children
            .iter()
            .filter(|b| b.content_type == ContentType::Text)
            .map(|b| b.id.clone())
            .collect();

        if child_ids.is_empty() {
            return None;
        }

        // In the Main document view prod renders the focused root block itself
        // as the first row of the collection (the document/title row), so it is
        // an arrow-nav target sitting *above* the first child — arrow-Up from
        // the first child lands on it. The reference navigator is built from the
        // block tree, which would otherwise cover only the children and treat
        // first-child-Up as a boundary. Include the root row so the navigable
        // set mirrors the rendered collection. Sidebars list pages under a
        // synthetic root that is not itself rendered, so this only applies to
        // Main.
        let root_row = (region == Region::Main).then(|| focus_id.clone());

        match self.active_render_expr_name(region).as_deref() {
            Some("tree") | Some("outline") => {
                let mut dfs_order = Vec::new();
                let mut parent_map = std::collections::HashMap::new();
                self.collect_dfs_order(&focus_id, &mut dfs_order, &mut parent_map);
                if dfs_order.is_empty() {
                    return None;
                }
                // Root row is the tree root: prepend with no parent entry.
                if let Some(root) = root_row {
                    dfs_order.insert(0, root);
                }
                Some(Box::new(TreeNavigator::from_dfs_and_parents(
                    dfs_order, parent_map,
                )))
            }
            // list / columns / table / unknown → ListNavigator
            _ => {
                let mut ids = child_ids;
                if let Some(root) = root_row {
                    ids.insert(0, root);
                }
                Some(Box::new(ListNavigator::new(ids)))
            }
        }
    }

    fn collect_dfs_order(
        &self,
        parent_id: &EntityUri,
        dfs_order: &mut Vec<EntityUri>,
        parent_map: &mut std::collections::HashMap<EntityUri, EntityUri>,
    ) {
        let children = self.sorted_children_of(parent_id);
        for child in children {
            if child.content_type != ContentType::Text {
                continue;
            }
            dfs_order.push(child.id.clone());
            if parent_id != &EntityUri::no_parent() {
                parent_map.insert(child.id.clone(), parent_id.clone());
            }
            self.collect_dfs_order(&child.id, dfs_order, parent_map);
        }
    }

    /// Block IDs whose `content` must NEVER be mutated by an edit transition:
    /// query / render source blocks (would corrupt the active layout) and
    /// entity-profile blocks (typed YAML, not free-form text).
    pub fn no_content_update_set(&self) -> std::collections::HashSet<EntityUri> {
        self.domain
            .layout_blocks
            .render_source_ids
            .iter()
            .chain(self.domain.layout_blocks.query_source_ids.iter())
            .chain(self.domain.profile_block_ids.iter())
            .cloned()
            .collect()
    }

    /// Stable IDs of blocks any peer has modified. JoinBlock excludes these
    /// to avoid edit/peer interleaving races.
    pub fn peer_modified_stable_ids(&self) -> std::collections::HashSet<String> {
        self.peers
            .iter()
            .flat_map(|p| p.modified_stable_ids.iter().cloned())
            .collect()
    }

    /// The focused Main-region block, if it is a valid edit target:
    /// non-page text, focusable, not content-locked, and a descendant of
    /// Main's focus_roots. Returns None when no Main focus, the system
    /// isn't properly set up, or the focused block fails any check.
    ///
    /// Used by the "edit only the user-clicked block" transitions —
    /// SplitBlock, Indent, Outdent, EditViaViewModel, EditViaDisplayTree,
    /// DragDropBlock (source).
    pub fn focused_main_editable(&self) -> Option<EntityUri> {
        if !self.is_properly_setup() {
            return None;
        }
        let focused = self.focused_entity(Region::Main)?.clone();
        let block = self.domain.block_state.blocks.get(&focused)?;
        if block.content_type != ContentType::Text || block.is_page() {
            return None;
        }
        if !self.domain.layout_blocks.is_focusable(&focused) {
            return None;
        }
        if self.no_content_update_set().contains(&focused) {
            return None;
        }
        let focus_roots = self.expected_focus_root_ids(Region::Main);
        if !self.is_descendant_of_any(&focused, &focus_roots) {
            return None;
        }
        Some(focused)
    }

    /// All text blocks descendant of Main's focus_roots that are safe to edit:
    /// non-page text, not part of the layout, not content-locked, not
    /// peer-modified.
    ///
    /// Used by the "edit any visible block" transitions — JoinBlock today;
    /// SplitBlock and friends if/when the focus-only asymmetry is dropped.
    pub fn main_editable_descendants(&self) -> Vec<EntityUri> {
        let focus_roots = self.expected_focus_root_ids(Region::Main);
        let no_update = self.no_content_update_set();
        let peer_modified = self.peer_modified_stable_ids();
        self.domain
            .block_state
            .blocks
            .iter()
            .filter(|(id, b)| {
                b.content_type == ContentType::Text
                    && !b.is_page()
                    && !self.domain.layout_blocks.contains(id)
                    && !peer_modified.contains(id.id())
                    && !no_update.contains(id)
                    && self.is_descendant_of_any(id, &focus_roots)
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Whether a click on `uri` in `region` is predicted to dispatch
    /// `navigation.focus(region=main, block_id=uri)` — the bound action the
    /// default LeftSidebar wraps each doc selectable in.
    ///
    /// The default sidebar PRQL selects page blocks with non-special
    /// titles (not "index" / "__default__"), and the layout wraps every
    /// row in `selectable(action: navigation.focus(region="main",
    /// block_id=col("id")))`. Used by `ClickBlock::apply_to_ref`
    /// (LeftSidebar branch) and `NavigateFocus` to gate the
    /// navigation-history + open_pins mutations on whether prod would
    /// actually dispatch the bound intent. Without this, the ref model
    /// would push nav-history entries for sidebar clicks on entities
    /// prod treats as plain editor-focus targets, breaking
    /// `inv-focus-roots-consistent-with-ref`.
    pub fn predicts_navigation_focus(&self, uri: &EntityUri, region: Region) -> bool {
        if region != Region::LeftSidebar {
            return false;
        }
        // A user index.org replaces the whole 3-column layout: there is no
        // LeftSidebar live_block at all, so no sidebar row ever binds
        // `navigation.focus` (same family as DragDropBlock's
        // CustomLayoutNotDraggable gate).
        if self.has_user_index_org() {
            return false;
        }
        let Some(block) = self.domain.block_state.blocks.get(uri) else {
            return false;
        };
        if block.content_type != ContentType::Text || !block.is_page() {
            return false;
        }
        let t = block.title();
        !t.is_empty() && t != "index" && t != "__default__"
    }

    /// Block IDs in the predicted LeftSidebar render set — the same set
    /// the default sidebar PRQL produces. Each entry is wrapped by the
    /// default layout in a selectable bound to `navigation.focus`, so
    /// this is also the candidate set for `ClickBlock(LeftSidebar)` and
    /// `NavigateFocus` generators.
    pub fn predicted_sidebar_navigation_targets(&self) -> Vec<EntityUri> {
        self.domain
            .block_state
            .blocks
            .values()
            .filter(|b| {
                if b.content_type != ContentType::Text || !b.is_page() {
                    return false;
                }
                let t = b.title();
                !t.is_empty() && t != "index" && t != "__default__"
            })
            .map(|b| b.id.clone())
            .collect()
    }

    /// Get IDs of text blocks only (not source blocks).
    pub fn text_block_ids(&self) -> Vec<EntityUri> {
        self.domain
            .block_state
            .blocks
            .iter()
            .filter(|(_, b)| b.content_type == ContentType::Text)
            .map(|(id, _)| id.clone())
            .collect()
    }

    // ── Block hierarchy query helpers ──────────────────────────────────

    /// Children of parent sorted by sequence then ID (matching canonical ordering).
    pub fn sorted_children_of(&self, parent_id: &EntityUri) -> Vec<&Block> {
        use holon_orgmode::models::OrgBlockExt;
        let mut children: Vec<&Block> = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == *parent_id)
            .collect();
        children.sort_by(|a, b| {
            a.sequence()
                .cmp(&b.sequence())
                .then_with(|| a.id.cmp(&b.id))
        });
        children
    }

    /// Predicted ordered child ids of `parent_id`. Mirrors what
    /// `BlockOrdering::children(parent_id)` should return on the live
    /// side. The encoding-free child-id list is the contract — both
    /// sides produce a `Vec<EntityUri>`, no `sort_key` / `sequence`
    /// strings cross the boundary.
    pub fn children_of(&self, parent_id: &EntityUri) -> Vec<EntityUri> {
        self.sorted_children_of(parent_id)
            .into_iter()
            .map(|b| b.id.clone())
            .collect()
    }

    /// Previous sibling of block_id (same parent, immediately before in sequence order).
    pub fn previous_sibling(&self, block_id: &EntityUri) -> Option<EntityUri> {
        let block = self.domain.block_state.blocks.get(block_id)?;
        let children = self.sorted_children_of(&block.parent_id);
        let idx = children.iter().position(|b| b.id == *block_id)?;
        if idx > 0 {
            Some(children[idx - 1].id.clone())
        } else {
            None
        }
    }

    /// Next sibling of block_id (same parent, immediately after in sequence order).
    pub fn next_sibling(&self, block_id: &EntityUri) -> Option<EntityUri> {
        let block = self.domain.block_state.blocks.get(block_id)?;
        let children = self.sorted_children_of(&block.parent_id);
        let idx = children.iter().position(|b| b.id == *block_id)?;
        children.get(idx + 1).map(|b| b.id.clone())
    }

    /// Grandparent of block_id (parent's parent). None if at root level.
    pub fn grandparent(&self, block_id: &EntityUri) -> Option<EntityUri> {
        let block = self.domain.block_state.blocks.get(block_id)?;
        let parent = self.domain.block_state.blocks.get(&block.parent_id)?;
        if parent.parent_id.is_no_parent() || parent.parent_id.is_sentinel() {
            None
        } else {
            Some(parent.parent_id.clone())
        }
    }

    // ── Block hierarchy mutation helpers ─────────────────────────────

    /// Move `block_id` under `new_parent`, mirroring production's
    /// `move_block(id, parent_id, after_block_id)`
    /// (`crates/holon-core/src/traits.rs:542`).
    ///
    /// `after_block_id = None` inserts at the beginning of the new
    /// parent's children. `Some(anchor)` inserts immediately after
    /// `anchor` (which must already be a child of `new_parent`).
    ///
    /// Sequences for the new parent are reassigned to match the new
    /// order — we deliberately do NOT call `recanon_and_rebuild`, since
    /// the canonical "source content_type first" sort would override the
    /// production sort_key this operation is modeling.
    pub fn move_block(
        &mut self,
        block_id: &EntityUri,
        new_parent: EntityUri,
        after_block_id: Option<&EntityUri>,
    ) {
        use holon_orgmode::models::OrgBlockExt;

        self.domain
            .block_state
            .blocks
            .get_mut(block_id)
            .unwrap()
            .parent_id = new_parent.clone();

        let mut siblings: Vec<EntityUri> = self
            .sorted_children_of(&new_parent)
            .into_iter()
            .map(|b| b.id.clone())
            .filter(|id| id != block_id)
            .collect();
        let insert_at = match after_block_id {
            None => 0,
            Some(anchor) => siblings
                .iter()
                .position(|id| id == anchor)
                .map(|p| p + 1)
                .unwrap_or(siblings.len()),
        };
        siblings.insert(insert_at, block_id.clone());

        for (i, id) in siblings.iter().enumerate() {
            if let Some(b) = self.domain.block_state.blocks.get_mut(id) {
                b.set_sequence(i as i64);
            }
        }
        self.rebuild_profile_tracking();
    }

    /// Move `block_id` to the grandparent, placing it as the next sibling
    /// **after** its old parent. Mirrors production `outdent`
    /// (`crates/holon-core/src/traits.rs:693`) which calls
    /// `move_block(id, grandparent_id, Some(parent_id))` — production's
    /// `move_block` puts the block strictly between the predecessor (old
    /// parent) and whatever follows it under grandparent, using a
    /// fractional index. We mirror that by shifting later siblings up by
    /// one and setting `sequence = old_parent_seq + 1`.
    pub fn outdent_block(&mut self, block_id: &EntityUri) {
        use holon_orgmode::models::OrgBlockExt;
        let block = self.domain.block_state.blocks.get(block_id).unwrap();
        let old_parent_id = block.parent_id.clone();
        let old_parent = self.domain.block_state.blocks.get(&old_parent_id).unwrap();
        let grandparent_id = old_parent.parent_id.clone();
        let old_parent_seq = old_parent.sequence();

        let target_seq = old_parent_seq + 1;
        for sibling in self.domain.block_state.blocks.values_mut() {
            if sibling.id == *block_id {
                continue;
            }
            if sibling.parent_id == grandparent_id && sibling.sequence() >= target_seq {
                let s = sibling.sequence();
                sibling.set_sequence(s + 1);
            }
        }
        let block = self.domain.block_state.blocks.get_mut(block_id).unwrap();
        block.parent_id = grandparent_id;
        block.set_sequence(target_seq);
        self.recanon_and_rebuild();
    }

    /// Swap the sequence of two blocks, re-canonicalize, and rebuild profiles.
    pub fn swap_sequence(&mut self, a: &EntityUri, b: &EntityUri) {
        use holon_orgmode::models::OrgBlockExt;
        let seq_a = self.domain.block_state.blocks.get(a).unwrap().sequence();
        let seq_b = self.domain.block_state.blocks.get(b).unwrap().sequence();
        self.domain
            .block_state
            .blocks
            .get_mut(a)
            .unwrap()
            .set_sequence(seq_b);
        self.domain
            .block_state
            .blocks
            .get_mut(b)
            .unwrap()
            .set_sequence(seq_a);
        self.recanon_and_rebuild();
    }

    /// Split a block at the given byte position, mirroring `traits.rs::split_block`.
    ///
    /// Original block keeps `content[..position].trim_end()`.
    /// New block gets `content[position..].trim_start()` with a synthetic ID.
    /// Returns the synthetic ID of the newly created block.
    pub fn split_block(&mut self, block_id: &EntityUri, position: usize) -> EntityUri {
        use holon_orgmode::models::OrgBlockExt;

        let original = self.domain.block_state.blocks.get(block_id).unwrap();
        let content = original.content.clone();
        let parent_id = original.parent_id.clone();
        let original_seq = original.sequence();

        // Split content (same logic as traits.rs:756-763)
        let content_before = content[..position].trim_end().to_string();
        let content_after = content[position..].trim_start().to_string();

        // Update original block
        self.domain
            .block_state
            .blocks
            .get_mut(block_id)
            .unwrap()
            .content = content_before;

        // Create new block with synthetic ID
        let new_id = EntityUri::block(&format!(":split-{}", self.domain.block_state.next_id));
        let mut new_block = Block::new_text(new_id.clone(), parent_id.clone(), content_after);
        // Place after original: shift every sibling already at or after this
        // position one slot down before inserting, so the new block lands
        // uniquely between the original and the next existing sibling.
        //
        // Without the shift the new block ends up sharing `original_seq + 1`
        // with whatever sibling occupied that slot; `recanon_and_rebuild` then
        // tie-breaks by lexicographic id and routinely puts the new block
        // *past* that sibling instead of right after the original. Production's
        // `BlockOperations::split_block` uses fractional indices and always
        // lands the new block strictly between the two — mirror that ordering
        // here so chord-op chains (e.g. SplitBlock → MoveUp → Indent) compute
        // the same `previous_sibling`.
        let shift_threshold = original_seq + 1;
        for sibling in self.domain.block_state.blocks.values_mut() {
            if sibling.parent_id == parent_id && sibling.sequence() >= shift_threshold {
                let s = sibling.sequence();
                sibling.set_sequence(s + 1);
            }
        }
        new_block.set_sequence(shift_threshold);

        // Track in block_documents with same doc_uri as original
        let doc_uri = self
            .domain
            .block_state
            .block_documents
            .get(block_id)
            .cloned()
            .unwrap_or_else(|| parent_id.clone());
        self.domain
            .block_state
            .block_documents
            .insert(new_id.clone(), doc_uri);

        self.domain
            .block_state
            .blocks
            .insert(new_id.clone(), new_block);
        self.recanon_and_rebuild();
        new_id
    }

    /// Join `block_id` into its merge target.
    ///
    /// Two cases, both triggered by Backspace at position 0:
    ///   1. **Previous sibling exists** (target = prev sibling at same level):
    ///      - prev.content = prev.content + block.content
    ///      - re-parent block's children to prev, appended after prev's
    ///        existing children
    ///      - delete block
    ///   2. **No previous sibling, parent is text** (target = parent;
    ///      child→parent join):
    ///      - parent.content = parent.content + block.content
    ///      - re-parent block's children to parent, placed at block's old
    ///        slot (before block's old siblings)
    ///      - delete block
    ///
    /// Returns the byte offset in the target where the join happened (i.e.
    /// the length of the target's old content) — the cursor lands here.
    ///
    /// Panics if neither case applies — call only after the precondition
    /// has been validated.
    pub fn join_block(&mut self, block_id: &EntityUri) -> usize {
        use holon_orgmode::models::OrgBlockExt;

        let block = self
            .domain
            .block_state
            .blocks
            .get(block_id)
            .unwrap()
            .clone();
        let prev_id = self.previous_sibling(block_id);
        let target_id = match &prev_id {
            Some(id) => id.clone(),
            None => block.parent_id.clone(),
        };
        let into_parent = prev_id.is_none();

        // Capture original contents.
        let target = self.domain.block_state.blocks.get(&target_id).unwrap();
        let target_content = target.content.clone();
        let join_offset = target_content.len();

        // Append block's content to target's content.
        self.domain
            .block_state
            .blocks
            .get_mut(&target_id)
            .unwrap()
            .content = format!("{}{}", target_content, block.content);

        // Re-parent block's children to target.
        let block_child_ids: Vec<EntityUri> = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == *block_id)
            .map(|b| b.id.clone())
            .collect();
        let mut sorted_children = block_child_ids;
        sorted_children.sort_by_key(|id| {
            self.domain
                .block_state
                .blocks
                .get(id)
                .map(|b| b.sequence())
                .unwrap_or(0)
        });

        if into_parent {
            // Child→parent: place block's children at block's old slot, then
            // shift block's old siblings (those with sequence > block.seq) up
            // by `len(children) - 1` so the canonical order under parent
            // becomes [...children..., ...remaining-siblings...].
            let block_seq = block.sequence();
            let n = sorted_children.len();
            if n >= 2 {
                let shift = (n as i64) - 1;
                let to_shift: Vec<EntityUri> = self
                    .domain
                    .block_state
                    .blocks
                    .values()
                    .filter(|b| {
                        b.parent_id == target_id && b.id != *block_id && b.sequence() > block_seq
                    })
                    .map(|b| b.id.clone())
                    .collect();
                for sid in to_shift {
                    let s = self.domain.block_state.blocks.get_mut(&sid).unwrap();
                    s.set_sequence(s.sequence() + shift);
                }
            }
            for (i, child_id) in sorted_children.iter().enumerate() {
                let child = self.domain.block_state.blocks.get_mut(child_id).unwrap();
                child.parent_id = target_id.clone();
                child.set_sequence(block_seq + i as i64);
            }
        } else {
            // Prev-sibling: append block's children after target's existing
            // children, preserving relative order within block's children.
            let max_target_child_seq = self
                .domain
                .block_state
                .blocks
                .values()
                .filter(|b| b.parent_id == target_id)
                .map(|b| b.sequence())
                .max()
                .unwrap_or(0);
            let mut next_seq = max_target_child_seq + 1;
            for child_id in sorted_children {
                let child = self.domain.block_state.blocks.get_mut(&child_id).unwrap();
                child.parent_id = target_id.clone();
                child.set_sequence(next_seq);
                next_seq += 1;
            }
        }

        // Delete block_id from blocks + block_documents.
        self.domain.block_state.blocks.remove(block_id);
        self.domain.block_state.block_documents.remove(block_id);

        self.recanon_and_rebuild();
        join_offset
    }

    /// Apply a mutation to the block state, re-canonicalize, and rebuild profiles.
    pub fn apply_mutation(&mut self, event: &super::types::MutationEvent) {
        let mut blocks: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        event.mutation.apply_to(&mut blocks);
        self.domain.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        self.recanon_and_rebuild();
    }

    /// Re-canonicalize sequences and rebuild profile tracking.
    pub fn recanon_and_rebuild(&mut self) {
        let mut blocks: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        crate::assign_reference_sequences_canonical(&mut blocks);
        self.domain.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        self.rebuild_profile_tracking();
        self.domain.block_state.next_id += 1;
    }

    /// Returns the set of block IDs that should appear in `focus_roots` for a region.
    /// Mirrors `schema/matview_focus_roots.sql`: a flat projection of
    /// `navigation_history WHERE closed_at IS NULL`, excluding home rows
    /// (block_id NULL — they don't JOIN against `root.id` in the consumer GQL).
    ///
    /// For Region::Main, the close-prior-then-insert contract of
    /// `NavigateFocus`/`NavigateHome` keeps this set at size ≤ 1. For
    /// Region::RightSidebar, `PinBlock` can grow it (move-to-top dedup
    /// keeps each block_id unique within the region). Consumers use
    /// CHILD_OF*0..N to expand to root + descendants.
    pub fn expected_focus_root_ids(&self, region: Region) -> BTreeSet<EntityUri> {
        self.ui
            .user
            .open_pins
            .get(&region)
            .map(|pins| {
                pins.iter()
                    .filter_map(|p| p.block_id.clone())
                    .collect::<BTreeSet<_>>()
            })
            .unwrap_or_default()
    }
    // line padding to preserve archlint line offsets — Phase C semantic flip
    // intentionally trimmed body; downstream test files reference offsets.
    // Removing this comment shifts following ALLOW directives.

    /// Check if `block_id` is a descendant of any block in `roots` (or is itself in `roots`).
    pub fn is_descendant_of_any(
        &self,
        block_id: &EntityUri,
        roots: &std::collections::BTreeSet<EntityUri>,
    ) -> bool {
        if roots.contains(block_id) {
            return true;
        }
        // Walk up parent chain
        let mut current = block_id.clone();
        for _ in 0..50 {
            if let Some(block) = self.domain.block_state.blocks.get(&current) {
                if roots.contains(&block.parent_id) {
                    return true;
                }
                if block.parent_id.is_no_parent() || block.parent_id.is_sentinel() {
                    return false;
                }
                current = block.parent_id.clone();
            } else {
                return false;
            }
        }
        false
    }

    pub fn has_blocks_profile(&self) -> bool {
        self.domain.active_profiles.contains_key("block")
    }

    /// Rebuild profile tracking from current blocks state.
    pub fn rebuild_profile_tracking(&mut self) {
        self.domain.profile_block_ids.clear();
        self.domain.active_profiles.clear();
        for (block_key, block) in &self.domain.block_state.blocks {
            // Skip seeded default layout blocks — they exist in the DB but
            // the profile resolver picks them up independently from the
            // ProfileResolver's LiveData source, not from the test's org files.
            if self
                .domain
                .block_state
                .block_documents
                .get(&block.id)
                .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel())
            {
                continue;
            }
            if block
                .source_language
                .as_ref()
                .map(|sl| sl.to_string())
                .as_deref()
                == Some("holon_entity_profile_yaml")
            {
                self.domain.profile_block_ids.insert(block_key.clone());
                if let Some(yaml_idx) = VALID_PROFILE_YAMLS
                    .iter()
                    .position(|y| block.content.trim() == y.trim())
                    && let Some(entity_name) = block
                        .content
                        .lines()
                        .next()
                        .and_then(|l| l.strip_prefix("entity_name: "))
                {
                    self.domain.active_profiles.insert(
                        EntityName::new(entity_name.trim()),
                        (block_key.clone(), yaml_idx),
                    );
                }
            }
        }
    }

    /// Snapshot current block state before a UI mutation and clear redo stack.
    ///
    /// Currently a no-op: the engine's SqlOperationProvider returns
    /// `OperationResult::irreversible()` for all operations, so the real
    /// undo stack is never populated. Re-enable once the provider produces
    /// inverse operations.
    pub fn push_undo_snapshot(&mut self) {
        // self.undo_stack.push(self.block_state.clone());
        // self.redo_stack.clear();
    }

    /// Undo: snapshot current state onto redo stack, restore from undo stack.
    pub fn pop_undo_to_redo(&mut self) {
        self.action.redo_stack.push(self.domain.block_state.clone());
        self.domain.block_state = self.action.undo_stack.pop().expect("undo stack is empty");
        self.recompute_derived();
    }

    /// Redo: snapshot current state onto undo stack, restore from redo stack.
    pub fn pop_redo_to_undo(&mut self) {
        self.action.undo_stack.push(self.domain.block_state.clone());
        self.domain.block_state = self.action.redo_stack.pop().expect("redo stack is empty");
        self.recompute_derived();
    }

    /// Recompute derived fields (profiles, render expressions) after undo/redo restore.
    fn recompute_derived(&mut self) {
        self.rebuild_profile_tracking();
        self.domain.render_expressions.clear();
        for id in &self.domain.layout_blocks.render_source_ids {
            if let Some(block) = self.domain.block_state.blocks.get(id)
                && let Some(expr) = render_expr_from_rhai(block.content.as_str())
            {
                self.domain.render_expressions.insert(id.clone(), expr);
            }
        }
    }

    /// The id of the layout's main-panel container block, when the active
    /// layout has one. Identified semantically: a render-source block whose
    /// parent is the main-panel container resolves the container id back out.
    /// Returns `None` in layout-less mode (no main-panel render source).
    ///
    /// The well-known default-layout main panel id is the seed id
    /// `block:default-main-panel`; this accessor returns it from the resolved
    /// block state so callers (e.g. `inv-viewmodel-root-matches-render-expr`)
    /// never embed that literal themselves.
    pub fn main_panel_block_id(&self) -> Option<EntityUri> {
        let main_panel_id = EntityUri::parse("block:default-main-panel").expect("static id");
        self.domain
            .block_state
            .blocks
            .contains_key(&main_panel_id)
            .then(|| main_panel_id.clone())
    }

    /// Get the main panel's render expression (the render source child of the main panel headline).
    pub fn main_panel_render_expr(&self) -> Option<&RenderExpr> {
        let main_panel_id = self.main_panel_block_id()?;
        self.domain
            .layout_blocks
            .render_source_ids
            .iter()
            .find(|id| {
                self.domain
                    .block_state
                    .blocks
                    .get(*id)
                    .is_some_and(|b| b.parent_id == main_panel_id)
            })
            .and_then(|id| self.domain.render_expressions.get(id))
    }
}

// ── BuilderServices implementation ──────────────────────────────────────

/// Walk a rendered `ReactiveViewModel` tree and report whether any node is one
/// of `widgets` (by `widget_name`). Mirrors `count_bottom_docks` (children +
/// collection items + slot content) — the canonical PBT widget-tree walk.
fn view_model_has_widget(node: &holon_frontend::ReactiveViewModel, widgets: &[&str]) -> bool {
    if node
        .widget_name()
        .as_deref()
        .is_some_and(|n| widgets.contains(&n))
    {
        return true;
    }
    if node
        .children
        .iter()
        .any(|c| view_model_has_widget(c, widgets))
    {
        return true;
    }
    if let Some(ref view) = node.collection
        && view
            .items
            .lock_ref()
            .iter()
            .any(|item| view_model_has_widget(item, widgets))
    {
        return true;
    }
    if let Some(ref slot) = node.slot {
        let content = slot.content.lock_ref();
        if view_model_has_widget(&content, widgets) {
            return true;
        }
    }
    false
}

/// Convert a Block to a DataRow (HashMap<String, Value>) for ViewModel construction.
pub fn block_to_data_row(block: &Block) -> holon_api::widget_spec::DataRow {
    let mut row = HashMap::new();
    row.insert("id".into(), Value::String(block.id.as_str().to_string()));
    row.insert("content".into(), Value::String(block.content.clone()));
    row.insert(
        "content_type".into(),
        Value::String(block.content_type.to_string()),
    );
    row.insert(
        "parent_id".into(),
        Value::String(block.parent_id.as_str().to_string()),
    );
    // document_id removed from Block struct; looked up via block_documents map if needed
    if let Some(Value::String(ts)) = block.properties.get("task_state") {
        row.insert("task_state".into(), Value::String(ts.clone()));
    }
    if let Some(sl) = &block.source_language {
        row.insert("source_language".into(), Value::String(sl.to_string()));
    }
    row
}

impl holon_frontend::reactive::BuilderServices for ReferenceState {
    fn interpret(
        &self,
        expr: &RenderExpr,
        ctx: &holon_frontend::RenderContext,
    ) -> holon_frontend::ReactiveViewModel {
        self.interpreter.interpret(expr, ctx, self)
    }

    fn get_block_data(
        &self,
        id: &EntityUri,
    ) -> (RenderExpr, Vec<Arc<holon_api::widget_spec::DataRow>>) {
        // Find render source child of this block in layout_blocks.
        // Two distinct "no expr" cases, previously conflated by a silent
        // `table()` fallback:
        // - a render-source child IS tracked but `render_expressions` has no
        //   entry → ref bookkeeping inconsistency, fail loud;
        // - no render-source child at all → the ref genuinely doesn't track
        //   a render for this block; fall back to `table()` like the prod
        //   stub services, but DISCLOSED (warn) so a vacuous comparison is
        //   attributable in the log.
        let render_source_child = self
            .domain
            .layout_blocks
            .render_source_ids
            .iter()
            .find(|rid| {
                self.domain
                    .block_state
                    .blocks
                    .get(*rid)
                    .is_some_and(|b| b.parent_id == *id)
            });
        let render_expr = match render_source_child {
            Some(rid) => self
                .domain
                .render_expressions
                .get(rid)
                .unwrap_or_else(|| {
                    panic!(
                        "[ref get_block_data] block {id} has a tracked render-source child \
                         {rid} but no compiled entry in render_expressions — reference-model \
                         bookkeeping inconsistency"
                    )
                })
                .clone(),
            None => {
                tracing::warn!(
                    "[ref get_block_data] no render-source child tracked for {id}; \
                     falling back to table() (untracked-by-reference render)"
                );
                RenderExpr::FunctionCall {
                    name: "table".into(),
                    args: vec![],
                }
            }
        };

        // Data rows = children blocks converted to DataRow
        let rows: Vec<holon_api::widget_spec::DataRow> = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == *id)
            .map(block_to_data_row)
            .collect();

        (render_expr, rows.into_iter().map(Arc::new).collect())
    }

    fn resolve_profile(
        &self,
        row: &holon_api::widget_spec::DataRow,
    ) -> Option<holon_api::RenderProfile> {
        use holon_api::render_types::RenderVariant;

        let profile = self.domain.seed_profile.as_ref()?;
        let engine = rhai::Engine::new();
        let (candidates, _computed) = profile.resolve_candidates(row, &engine);
        let ops = self.domain.block_operations.clone();
        let variants: Vec<RenderVariant> = candidates
            .iter()
            .map(|(variant, stored)| RenderVariant {
                name: stored.name.clone(),
                render: stored.render.clone(),
                operations: ops.clone(),
                condition: variant.ui_condition.clone(),
            })
            .collect();
        candidates
            .first()
            .map(|(_, stored)| holon_api::RenderProfile {
                name: stored.name.clone(),
                render: stored.render.clone(),
                operations: ops,
                variants,
            })
    }

    fn watch_query(
        &self,
        _: &str,
        _: holon_api::QueryLanguage,
        _: Option<holon_frontend::QueryContext>,
    ) -> anyhow::Result<holon_api::EnrichedChangeStream> {
        panic!("watch_query not supported on ReferenceState")
    }

    /// Mirror the ref's tracked drawer/toggle open-state (`ToggleDrawer`
    /// flips `ui.tab.drawer_open`, keyed by the schemed block-id string).
    /// Untracked ids default to open, matching production's boot layout —
    /// the previous unconditional `default()` made the ref render closed
    /// drawers as open (the closed-drawer NavigateFocus blind spot).
    fn widget_state(&self, id: &str) -> holon_frontend::config::WidgetState {
        holon_frontend::config::WidgetState {
            open: self.ui.tab.drawer_open.get(id).copied().unwrap_or(true),
            ..Default::default()
        }
    }

    fn dispatch_intent(&self, _: holon_frontend::operations::OperationIntent) {
        panic!("dispatch_intent not supported on ReferenceState")
    }

    fn present_op(
        &self,
        _: holon_api::render_types::OperationDescriptor,
        _: std::collections::HashMap<String, holon_api::Value>,
    ) {
        panic!("present_op not supported on ReferenceState — reference model has no UI")
    }

    fn key_bindings_snapshot(&self) -> std::collections::BTreeMap<String, holon_api::KeyChord> {
        let mut m = std::collections::BTreeMap::new();
        m.insert(
            "cycle_task_state".into(),
            holon_api::KeyChord::new(&[holon_api::Key::Cmd, holon_api::Key::Enter]),
        );
        m
    }

    fn runtime_handle(&self) -> tokio::runtime::Handle {
        panic!("runtime_handle not supported on ReferenceState — reference model is pure sync")
    }

    fn try_runtime_handle(&self) -> Option<tokio::runtime::Handle> {
        // Reference model is pure sync — no runtime, no spawning. Leaf
        // builders that conditionally spawn signal subscriptions check
        // this first and skip subscription setup here.
        None
    }

    fn search_link_candidates(
        &self,
        _: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = anyhow::Result<Vec<holon_api::LinkCandidate>>>
                + Send
                + 'static,
        >,
    > {
        Box::pin(async { anyhow::bail!("search_link_candidates not supported on ReferenceState") })
    }
}

#[cfg(test)]
mod remap_tests {
    use super::*;

    #[test]
    fn remapped_doc_uris_resolves_doc_ids_and_parents_only() {
        // Synthetic doc URI + one content block parented under it, plus a
        // content block parented under another content block (no doc URI).
        let syn_doc = EntityUri::block("ref-doc-1");
        let real_doc = EntityUri::block("11111111-2222-3333-4444-555555555555");
        let child = EntityUri::block("bulk-1");
        let grandchild = EntityUri::block("bulk-2");

        let mut blocks = BTreeMap::new();
        blocks.insert(
            syn_doc.clone(),
            Block::new_text(syn_doc.clone(), EntityUri::no_parent(), "doc"),
        );
        blocks.insert(
            child.clone(),
            Block::new_text(child.clone(), syn_doc.clone(), "child"),
        );
        blocks.insert(
            grandchild.clone(),
            Block::new_text(grandchild.clone(), child.clone(), "grandchild"),
        );

        let mut block_documents = BTreeMap::new();
        block_documents.insert(syn_doc.clone(), EntityUri::no_parent());
        block_documents.insert(child.clone(), syn_doc.clone());

        let bs = BlockState {
            blocks,
            block_documents,
            next_id: 3,
        };

        let mut map = BTreeMap::new();
        map.insert(syn_doc.clone(), real_doc.clone());

        let out = bs.remapped_doc_uris(&map);

        // The doc block is re-keyed + re-id'd to the real UUID.
        assert!(out.blocks.contains_key(&real_doc));
        assert!(!out.blocks.contains_key(&syn_doc));
        assert_eq!(out.blocks[&real_doc].id, real_doc);

        // The child's parent (a doc URI) is resolved; its own id (a content
        // URI, absent from the map) is untouched.
        assert_eq!(out.blocks[&child].parent_id, real_doc);
        assert_eq!(out.blocks[&child].id, child);

        // The grandchild (no doc URI anywhere) passes through unchanged.
        assert_eq!(out.blocks[&grandchild].parent_id, child);

        // block_documents keys are resolved (drives seed-id filtering).
        assert!(out.block_documents.contains_key(&real_doc));
        assert!(!out.block_documents.contains_key(&syn_doc));
        assert!(out.block_documents.contains_key(&child));

        // next_id preserved.
        assert_eq!(out.next_id, 3);
    }
}
