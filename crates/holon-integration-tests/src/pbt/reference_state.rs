//! Reference model for the PBT state machine.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::render_types::{Arg, RenderExpr};
use holon_api::{ContentType, EntityName, Region, Value};

use super::query::WatchSpec;
use super::types::TestVariant;

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

/// Reference state tracking all expected data (uses production Block struct)
#[derive(Debug, Clone)]
pub struct ReferenceState {
    /// Whether the application has been started
    pub app_started: bool,

    /// Block data affected by undo/redo
    pub block_state: BlockState,

    /// Created documents (doc_uri -> file_name).
    /// `BTreeMap` for deterministic iteration (see `BlockState::blocks`).
    pub documents: BTreeMap<EntityUri, String>,

    /// Active query watches (query_id -> watch spec with TestQuery)
    pub active_watches: HashMap<String, WatchSpec>,

    /// ID counter for generating unique document IDs
    pub next_doc_id: usize,

    /// Current view filter ("all", "main", "sidebar")
    pub current_view: String,

    /// Navigation history per region (for back/forward navigation)
    pub navigation_history: HashMap<Region, NavigationHistory>,

    /// Open `navigation_history` rows per region (`closed_at IS NULL`).
    /// Mirrors the rows the `focus_roots` matview projects from.
    ///
    /// - `NavigateFocus` / `NavigateHome` close all prior open in the region,
    ///   then push a new open row.
    /// - `PinBlock` (right sidebar) dedups by `(region, block_id)`: refresh
    ///   `added_ts_logical` if a matching open row exists, else push a new one.
    /// - `UnpinBlock` removes the row by `history_id` (sidebar X button).
    /// - `NavigateBack` / `NavigateForward` walk the cursor only — they don't
    ///   touch `closed_at`, so this map is unchanged.
    pub open_pins: HashMap<Region, Vec<OpenPinEntry>>,

    /// Block URIs whose `expand_toggle` widget is currently expanded. Empty
    /// at startup — every toggle defaults collapsed. Mutated by
    /// `ExpandToggle` (insert) and `ToggleCollapse` (remove) transitions.
    /// Only meaningful for blocks whose render expression contains an
    /// `expand_toggle` function call; non-toggle blocks are never present.
    pub expanded_toggles: std::collections::HashSet<EntityUri>,

    /// Mirrors SQLite's `navigation_history.id AUTOINCREMENT` counter.
    /// Bumped on every INSERT (not on UPDATE-only paths like the move-to-top
    /// `update_pin_timestamp.sql`). PBT relies on this to align with the
    /// real backend's id allocation when `UnpinBlock` dispatches `close(history_id)`.
    pub next_history_id: i64,

    /// Monotonic logical timestamp for `OpenPinEntry::added_ts_logical`.
    /// Bumped on every INSERT and on every move-to-top refresh; gives a
    /// stable sort order independent of the SQL `datetime('now')` clock.
    pub next_pin_ts: u64,

    /// Currently focused entity ID per region (set by ClickBlock, updated by ArrowNavigate).
    /// None means no block is focused in that region.
    pub focused_entity_id: HashMap<Region, EntityUri>,

    /// Globally focused block mirror of `UiState.focused_block`. Updated by
    /// `NavigateFocus` to the navigation target. Feeds `focus_chain()` /
    /// `chain_ops()` row predictions used by inv-value-fn-provider-arg-variance/inv-sql-budget.
    pub focused_block: Option<EntityUri>,

    /// Cursor position in the focused block per region. Used to predict whether
    /// arrow keys cause cross-block navigation (cursor at boundary) or intra-block
    /// cursor movement (cursor in middle of multi-line content).
    pub focused_cursor: HashMap<Region, CursorPosition>,

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

    /// Typed layout block classification for index.org.
    pub layout_blocks: LayoutBlockInfo,

    /// Profile block IDs (blocks with source_language = holon_entity_profile_yaml)
    pub profile_block_ids: HashSet<EntityUri>,

    /// Current active profile YAML index per entity_name.
    pub active_profiles: HashMap<EntityName, (EntityUri, usize)>,

    /// Test variant configuration (which components are enabled)
    pub variant: TestVariant,

    /// TODO keyword set for task_state mutations (generated once per test case)
    pub keyword_set: Option<super::generators::TodoKeywordSet>,

    /// Active render expressions per render source block (block_id → RenderExpr).
    /// Updated when render source blocks are created or mutated.
    /// `BTreeMap` for deterministic iteration (see `BlockState::blocks`).
    pub render_expressions: BTreeMap<EntityUri, RenderExpr>,

    /// Undo stack: snapshots of BlockState before each UI mutation
    pub undo_stack: Vec<BlockState>,

    /// Redo stack: snapshots of BlockState before each undo
    pub redo_stack: Vec<BlockState>,

    /// Parsed entity profile from the seed YAML (or custom org file).
    /// Used by `BuilderServices::resolve_profile` for ViewModel construction.
    pub seed_profile: Option<holon::entity_profile::EntityProfile>,

    /// Block entity operations (set_field, create, update, delete, cycle_task_state).
    /// Used by `BuilderServices::resolve_profile` to inject operations into RowProfile.
    pub block_operations: Vec<holon_api::render_types::OperationDescriptor>,

    /// Loro-only peer instances for multi-instance sync testing.
    pub peers: Vec<PeerRefState>,

    /// Shadow interpreter resolved from FluxDI — source of truth for widget
    /// names and render DSL parsing.
    pub interpreter: Arc<ShadowInterpreter>,

    /// Mirror of the GPUI editor's live `InputState` for the focused
    /// EditableText. `Some` after `FocusEditableText` and until focus moves
    /// elsewhere (NavigateFocus / NavigateHome / ClickBlock onto a non-
    /// editable / structural-chord that destroys the row). Diverges from
    /// `block.content` whenever the user has typed/deleted without
    /// blurring — drives the commit-then-mutate contract for chord
    /// transitions like Enter/Backspace/Tab.
    pub active_editor: Option<ActiveEditor>,

    /// Variant tag of the most recently applied transition. Drives Markov-
    /// style adaptive weighting in `transitions()` — e.g. boost MoveCursor /
    /// TypeChars / PressKey weights right after a FocusEditableText.
    pub last_transition_kind: Option<&'static str>,

    /// Open/closed state per drawer (keyed by drawer block_id, e.g.
    /// `"block:default-left-sidebar"`). Mirrors the production
    /// `widget_open` table. Default layout's two sidebars start open
    /// after `apply_start_app`. Mutated by `ToggleDrawer`.
    pub drawer_open: HashMap<String, bool>,
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
}

impl ActiveEditor {
    /// Insert ASCII text at the cursor and advance.
    pub fn type_chars(&mut self, text: &str) {
        debug_assert!(self.cursor_byte <= self.in_memory_content.len());
        self.in_memory_content.insert_str(self.cursor_byte, text);
        self.cursor_byte += text.len();
    }

    /// Delete `count` chars before the cursor (Backspace ×count). Stops at start.
    pub fn delete_backward(&mut self, count: usize) {
        for _ in 0..count {
            if self.cursor_byte == 0 {
                break;
            }
            // ASCII-only: byte == char in our generators, so safe to step by 1.
            let new_cursor = self.cursor_byte - 1;
            self.in_memory_content.remove(new_cursor);
            self.cursor_byte = new_cursor;
        }
    }

    /// Move the caret to a clamped byte position.
    pub fn move_cursor(&mut self, position: usize) {
        self.cursor_byte = position.min(self.in_memory_content.len());
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

impl ReferenceState {
    pub fn new(variant: TestVariant, interpreter: Arc<ShadowInterpreter>) -> Self {
        Self {
            app_started: false,
            block_state: BlockState {
                blocks: BTreeMap::new(),
                block_documents: BTreeMap::new(),
                next_id: 0,
            },
            documents: BTreeMap::new(),
            active_watches: HashMap::new(),
            next_doc_id: 0,
            current_view: "all".to_string(),
            navigation_history: HashMap::new(),
            open_pins: HashMap::new(),
            expanded_toggles: std::collections::HashSet::new(),
            next_history_id: 1,
            next_pin_ts: 1,
            focused_entity_id: HashMap::new(),
            focused_block: None,
            focused_cursor: HashMap::new(),
            runtime: Arc::new(tokio::runtime::Runtime::new().unwrap()),
            pre_startup_directories: Vec::new(),
            git_initialized: false,
            jj_initialized: false,
            pre_startup_file_count: 0,
            layout_blocks: LayoutBlockInfo::default(),
            profile_block_ids: HashSet::new(),
            active_profiles: HashMap::new(),
            variant,
            keyword_set: None,
            render_expressions: BTreeMap::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            seed_profile: None,
            block_operations: default_block_operations(),
            peers: Vec::new(),
            interpreter,
            active_editor: None,
            last_transition_kind: None,
            drawer_open: HashMap::new(),
        }
    }

    /// Whether atomic editor transitions (FocusEditableText, MoveCursor,
    /// TypeChars, DeleteBackward, PressKey) are enabled. Gated to the
    /// GPUI PBT — they need a real `InputState` to expose the
    /// in-memory-vs-DB divergence the bug class lives in.
    pub fn atomic_editor_enabled() -> bool {
        std::env::var("PBT_ATOMIC_EDITOR")
            .ok()
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false)
    }

    pub fn mutable_text_enabled() -> bool {
        std::env::var("PBT_MUTABLE_TEXT")
            .ok()
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false)
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
        let Some(editor) = self.active_editor.as_ref() else {
            return false;
        };
        let block_id = editor.block_id.clone();
        let in_memory = editor.in_memory_content.clone();
        let Some(block) = self.block_state.blocks.get_mut(&block_id) else {
            return false;
        };
        let normalized =
            super::types::normalize_content_for_org_roundtrip(&in_memory, block.content_type);
        if block.content == normalized {
            return false;
        }
        block.content = normalized;
        true
    }

    pub fn current_focus(&self, region: Region) -> Option<EntityUri> {
        self.navigation_history
            .get(&region)
            .and_then(|h| h.current_focus())
    }

    pub fn can_go_back(&self, region: Region) -> bool {
        self.navigation_history
            .get(&region)
            .map(|h| h.can_go_back())
            .unwrap_or(false)
    }

    /// If `block_id` is the focused entity in any region, reset the cursor to start.
    /// Called after mutations that change block content — the real editor would
    /// reposition the cursor (blur/refocus cycle), so the reference model must too.
    pub fn reset_cursor_if_focused(&mut self, block_id: &EntityUri) {
        for (region, focused_id) in &self.focused_entity_id {
            if focused_id == block_id {
                self.focused_cursor.insert(*region, CursorPosition::start());
            }
        }
    }

    /// If `block_id` is the focused entity in any region, clear the focus
    /// (the block was deleted — can't be focused anymore).
    pub fn clear_focus_if_deleted(&mut self, block_id: &EntityUri) {
        self.focused_entity_id.retain(|_, id| id != block_id);
        // focused_cursor entries for removed regions will be stale but harmless
    }

    /// Whether any region currently has a focused entity (required for ArrowNavigate).
    pub fn has_focus(&self) -> bool {
        !self.focused_entity_id.is_empty()
    }

    /// Get the focused entity in a region (set by ClickBlock).
    pub fn focused_entity(&self, region: Region) -> Option<&EntityUri> {
        self.focused_entity_id.get(&region)
    }

    pub fn can_go_forward(&self, region: Region) -> bool {
        self.navigation_history
            .get(&region)
            .map(|h| h.can_go_forward())
            .unwrap_or(false)
    }

    pub fn current_view(&self) -> String {
        self.current_view.clone()
    }

    /// Returns expected query results for a watch using the TestQuery evaluator.
    pub fn query_results(&self, watch_spec: &WatchSpec) -> Vec<HashMap<String, Value>> {
        watch_spec.query.evaluate(&self.block_state.blocks)
    }

    /// Check if index.org exists with the structure required by initial_widget().
    /// Generate a synthetic `block:ref-doc-N` URI for a new document and bump the counter.
    pub fn next_synthetic_doc_uri(&mut self) -> EntityUri {
        let uri = EntityUri::block(&format!("ref-doc-{}", self.next_doc_id));
        self.next_doc_id += 1;
        uri
    }

    /// Find a page block by its title (first line of content, e.g. "index").
    pub fn doc_uri_by_name(&self, title: &str) -> Option<EntityUri> {
        self.block_state
            .blocks
            .values()
            .find(|b| b.is_page() && b.title() == title)
            .map(|b| b.id.clone())
    }

    /// Whether the system has a valid root layout (from seed blocks or user-written index.org).
    /// Used to gate render_entity, ReactiveEngine, and ViewModel checks.
    pub fn is_properly_setup(&self) -> bool {
        !self.layout_blocks.query_source_ids.is_empty() || self.has_user_index_org()
    }

    /// Whether the user has written an index.org with query+render blocks.
    /// Used to gate block comparison invariants (seed blocks don't round-trip through org files).
    pub fn has_user_index_org(&self) -> bool {
        let index_doc_uri = match self.doc_uri_by_name("index") {
            Some(uri) => uri,
            None => return false,
        };

        let root_blocks: Vec<&Block> = self
            .block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == index_doc_uri)
            .collect();

        root_blocks.iter().any(|root_block| {
            self.block_state.blocks.values().any(|child| {
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
        self.block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == index_doc_uri)
            .find(|root_block| {
                self.block_state.blocks.values().any(|child| {
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

    /// Get the active `RenderExpr` for the root layout's render source block.
    /// Returns `None` if no render source is tracked.
    pub fn root_render_expr(&self) -> Option<&RenderExpr> {
        let root_id = self.root_layout_block_id()?;
        // Find the render source block that is a child of the root layout
        self.layout_blocks
            .render_source_ids
            .iter()
            .find(|id| {
                self.block_state
                    .blocks
                    .get(*id)
                    .map(|b| b.parent_id == root_id)
                    .unwrap_or(false)
            })
            .and_then(|id| self.render_expressions.get(id))
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
        let child_ids: Vec<String> = children
            .iter()
            .filter(|b| b.content_type == ContentType::Text)
            .map(|b| b.id.as_str().to_string())
            .collect();

        if child_ids.is_empty() {
            return None;
        }

        match self.active_render_expr_name(region).as_deref() {
            Some("tree") | Some("outline") => {
                let mut dfs_order = Vec::new();
                let mut parent_map = std::collections::HashMap::new();
                self.collect_dfs_order(&focus_id, &mut dfs_order, &mut parent_map);
                if dfs_order.is_empty() {
                    return None;
                }
                Some(Box::new(TreeNavigator::from_dfs_and_parents(
                    dfs_order, parent_map,
                )))
            }
            // list / columns / table / unknown → ListNavigator
            _ => Some(Box::new(ListNavigator::new(child_ids))),
        }
    }

    fn collect_dfs_order(
        &self,
        parent_id: &EntityUri,
        dfs_order: &mut Vec<String>,
        parent_map: &mut std::collections::HashMap<String, String>,
    ) {
        let children = self.sorted_children_of(parent_id);
        for child in children {
            if child.content_type != ContentType::Text {
                continue;
            }
            let child_id = child.id.as_str().to_string();
            dfs_order.push(child_id.clone());
            if parent_id != &EntityUri::no_parent() {
                parent_map.insert(child_id.clone(), parent_id.as_str().to_string());
            }
            self.collect_dfs_order(&child.id, dfs_order, parent_map);
        }
    }

    /// Block IDs whose `content` must NEVER be mutated by an edit transition:
    /// query / render source blocks (would corrupt the active layout) and
    /// entity-profile blocks (typed YAML, not free-form text).
    pub fn no_content_update_set(&self) -> std::collections::HashSet<EntityUri> {
        self.layout_blocks
            .render_source_ids
            .iter()
            .chain(self.layout_blocks.query_source_ids.iter())
            .chain(self.profile_block_ids.iter())
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
        let block = self.block_state.blocks.get(&focused)?;
        if block.content_type != ContentType::Text || block.is_page() {
            return None;
        }
        if !self.layout_blocks.is_focusable(&focused) {
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
        self.block_state
            .blocks
            .iter()
            .filter(|(id, b)| {
                b.content_type == ContentType::Text
                    && !b.is_page()
                    && !self.layout_blocks.contains(id)
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
        let Some(block) = self.block_state.blocks.get(uri) else {
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
        self.block_state
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
        self.block_state
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
        let block = self.block_state.blocks.get(block_id)?;
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
        let block = self.block_state.blocks.get(block_id)?;
        let children = self.sorted_children_of(&block.parent_id);
        let idx = children.iter().position(|b| b.id == *block_id)?;
        children.get(idx + 1).map(|b| b.id.clone())
    }

    /// Grandparent of block_id (parent's parent). None if at root level.
    pub fn grandparent(&self, block_id: &EntityUri) -> Option<EntityUri> {
        let block = self.block_state.blocks.get(block_id)?;
        let parent = self.block_state.blocks.get(&block.parent_id)?;
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

        self.block_state.blocks.get_mut(block_id).unwrap().parent_id = new_parent.clone();

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
            if let Some(b) = self.block_state.blocks.get_mut(id) {
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
        let block = self.block_state.blocks.get(block_id).unwrap();
        let old_parent_id = block.parent_id.clone();
        let old_parent = self.block_state.blocks.get(&old_parent_id).unwrap();
        let grandparent_id = old_parent.parent_id.clone();
        let old_parent_seq = old_parent.sequence();

        let target_seq = old_parent_seq + 1;
        for sibling in self.block_state.blocks.values_mut() {
            if sibling.id == *block_id {
                continue;
            }
            if sibling.parent_id == grandparent_id && sibling.sequence() >= target_seq {
                let s = sibling.sequence();
                sibling.set_sequence(s + 1);
            }
        }
        let block = self.block_state.blocks.get_mut(block_id).unwrap();
        block.parent_id = grandparent_id;
        block.set_sequence(target_seq);
        self.recanon_and_rebuild();
    }

    /// Swap the sequence of two blocks, re-canonicalize, and rebuild profiles.
    pub fn swap_sequence(&mut self, a: &EntityUri, b: &EntityUri) {
        use holon_orgmode::models::OrgBlockExt;
        let seq_a = self.block_state.blocks.get(a).unwrap().sequence();
        let seq_b = self.block_state.blocks.get(b).unwrap().sequence();
        self.block_state
            .blocks
            .get_mut(a)
            .unwrap()
            .set_sequence(seq_b);
        self.block_state
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

        let original = self.block_state.blocks.get(block_id).unwrap();
        let content = original.content.clone();
        let parent_id = original.parent_id.clone();
        let original_seq = original.sequence();

        // Split content (same logic as traits.rs:756-763)
        let content_before = content[..position].trim_end().to_string();
        let content_after = content[position..].trim_start().to_string();

        // Update original block
        self.block_state.blocks.get_mut(block_id).unwrap().content = content_before;

        // Create new block with synthetic ID
        let new_id = EntityUri::block(&format!(":split-{}", self.block_state.next_id));
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
        for sibling in self.block_state.blocks.values_mut() {
            if sibling.parent_id == parent_id && sibling.sequence() >= shift_threshold {
                let s = sibling.sequence();
                sibling.set_sequence(s + 1);
            }
        }
        new_block.set_sequence(shift_threshold);

        // Track in block_documents with same doc_uri as original
        let doc_uri = self
            .block_state
            .block_documents
            .get(block_id)
            .cloned()
            .unwrap_or_else(|| parent_id.clone());
        self.block_state
            .block_documents
            .insert(new_id.clone(), doc_uri);

        self.block_state.blocks.insert(new_id.clone(), new_block);
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

        let block = self.block_state.blocks.get(block_id).unwrap().clone();
        let prev_id = self.previous_sibling(block_id);
        let target_id = match &prev_id {
            Some(id) => id.clone(),
            None => block.parent_id.clone(),
        };
        let into_parent = prev_id.is_none();

        // Capture original contents.
        let target = self.block_state.blocks.get(&target_id).unwrap();
        let target_content = target.content.clone();
        let join_offset = target_content.len();

        // Append block's content to target's content.
        self.block_state.blocks.get_mut(&target_id).unwrap().content =
            format!("{}{}", target_content, block.content);

        // Re-parent block's children to target.
        let block_child_ids: Vec<EntityUri> = self
            .block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == *block_id)
            .map(|b| b.id.clone())
            .collect();
        let mut sorted_children = block_child_ids;
        sorted_children.sort_by_key(|id| {
            self.block_state
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
                    .block_state
                    .blocks
                    .values()
                    .filter(|b| {
                        b.parent_id == target_id && b.id != *block_id && b.sequence() > block_seq
                    })
                    .map(|b| b.id.clone())
                    .collect();
                for sid in to_shift {
                    let s = self.block_state.blocks.get_mut(&sid).unwrap();
                    s.set_sequence(s.sequence() + shift);
                }
            }
            for (i, child_id) in sorted_children.iter().enumerate() {
                let child = self.block_state.blocks.get_mut(child_id).unwrap();
                child.parent_id = target_id.clone();
                child.set_sequence(block_seq + i as i64);
            }
        } else {
            // Prev-sibling: append block's children after target's existing
            // children, preserving relative order within block's children.
            let max_target_child_seq = self
                .block_state
                .blocks
                .values()
                .filter(|b| b.parent_id == target_id)
                .map(|b| b.sequence())
                .max()
                .unwrap_or(0);
            let mut next_seq = max_target_child_seq + 1;
            for child_id in sorted_children {
                let child = self.block_state.blocks.get_mut(&child_id).unwrap();
                child.parent_id = target_id.clone();
                child.set_sequence(next_seq);
                next_seq += 1;
            }
        }

        // Delete block_id from blocks + block_documents.
        self.block_state.blocks.remove(block_id);
        self.block_state.block_documents.remove(block_id);

        self.recanon_and_rebuild();
        join_offset
    }

    /// Apply a mutation to the block state, re-canonicalize, and rebuild profiles.
    pub fn apply_mutation(&mut self, event: &super::types::MutationEvent) {
        let mut blocks: Vec<Block> = self.block_state.blocks.values().cloned().collect();
        event.mutation.apply_to(&mut blocks);
        self.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        self.recanon_and_rebuild();
    }

    /// Re-canonicalize sequences and rebuild profile tracking.
    pub fn recanon_and_rebuild(&mut self) {
        let mut blocks: Vec<Block> = self.block_state.blocks.values().cloned().collect();
        crate::assign_reference_sequences_canonical(&mut blocks);
        self.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        self.rebuild_profile_tracking();
        self.block_state.next_id += 1;
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
        self.open_pins
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
            if let Some(block) = self.block_state.blocks.get(&current) {
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
        self.active_profiles.contains_key("block")
    }

    /// Rebuild profile tracking from current blocks state.
    pub fn rebuild_profile_tracking(&mut self) {
        self.profile_block_ids.clear();
        self.active_profiles.clear();
        for (block_key, block) in &self.block_state.blocks {
            // Skip seeded default layout blocks — they exist in the DB but
            // the profile resolver picks them up independently from the
            // ProfileResolver's LiveData source, not from the test's org files.
            if self
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
                self.profile_block_ids.insert(block_key.clone());
                if let Some(yaml_idx) = VALID_PROFILE_YAMLS
                    .iter()
                    .position(|y| block.content.trim() == y.trim())
                    && let Some(entity_name) = block
                        .content
                        .lines()
                        .next()
                        .and_then(|l| l.strip_prefix("entity_name: "))
                {
                    self.active_profiles.insert(
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
        self.redo_stack.push(self.block_state.clone());
        self.block_state = self.undo_stack.pop().expect("undo stack is empty");
        self.recompute_derived();
    }

    /// Redo: snapshot current state onto undo stack, restore from redo stack.
    pub fn pop_redo_to_undo(&mut self) {
        self.undo_stack.push(self.block_state.clone());
        self.block_state = self.redo_stack.pop().expect("redo stack is empty");
        self.recompute_derived();
    }

    /// Recompute derived fields (profiles, render expressions) after undo/redo restore.
    fn recompute_derived(&mut self) {
        self.rebuild_profile_tracking();
        self.render_expressions.clear();
        for id in &self.layout_blocks.render_source_ids {
            if let Some(block) = self.block_state.blocks.get(id)
                && let Some(expr) = render_expr_from_rhai(block.content.as_str())
            {
                self.render_expressions.insert(id.clone(), expr);
            }
        }
    }

    /// Get the main panel's render expression (the render source child of the main panel headline).
    pub fn main_panel_render_expr(&self) -> Option<&RenderExpr> {
        let main_panel_id = EntityUri::from_raw("block:default-main-panel");
        self.layout_blocks
            .render_source_ids
            .iter()
            .find(|id| {
                self.block_state
                    .blocks
                    .get(*id)
                    .is_some_and(|b| b.parent_id == main_panel_id)
            })
            .and_then(|id| self.render_expressions.get(id))
    }
}

// ── BuilderServices implementation ──────────────────────────────────────

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

/// Default block entity operations matching SqlOperationProvider.
fn default_block_operations() -> Vec<holon_api::render_types::OperationDescriptor> {
    use holon_api::render_types::{OperationDescriptor, OperationParam, TypeHint};

    let entity_name = "block".to_string();
    let entity_short_name = "block".to_string();
    let id_param = OperationParam {
        name: "id".to_string(),
        type_hint: TypeHint::String,
        description: "Entity ID".to_string(),
    };

    vec![
        OperationDescriptor {
            entity_name: entity_name.clone().into(),
            entity_short_name: entity_short_name.clone(),
            name: "set_field".to_string(),
            display_name: "Set Field".to_string(),
            description: "Set a field on block".to_string(),
            required_params: vec![
                id_param.clone(),
                OperationParam {
                    name: "field".to_string(),
                    type_hint: TypeHint::String,
                    description: "Field name".to_string(),
                },
                OperationParam {
                    name: "value".to_string(),
                    type_hint: TypeHint::String,
                    description: "Field value".to_string(),
                },
            ],
            ..Default::default()
        },
        OperationDescriptor {
            entity_name: entity_name.clone().into(),
            entity_short_name: entity_short_name.clone(),
            name: "cycle_task_state".to_string(),
            display_name: "Cycle Task State".to_string(),
            description: "Cycle to the next task state".to_string(),
            required_params: vec![id_param],
            affected_fields: vec!["task_state".to_string()],
            ..Default::default()
        },
    ]
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
        // Find render source child of this block in layout_blocks
        let render_expr = self
            .layout_blocks
            .render_source_ids
            .iter()
            .find(|rid| {
                self.block_state
                    .blocks
                    .get(*rid)
                    .is_some_and(|b| b.parent_id == *id)
            })
            .and_then(|rid| self.render_expressions.get(rid))
            .cloned()
            .unwrap_or_else(|| RenderExpr::FunctionCall {
                name: "table".into(),
                args: vec![],
            });

        // Data rows = children blocks converted to DataRow
        let rows: Vec<holon_api::widget_spec::DataRow> = self
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
    ) -> Option<holon::entity_profile::RowProfile> {
        use holon_api::render_types::RenderVariant;

        let profile = self.seed_profile.as_ref()?;
        let engine = rhai::Engine::new();
        let (candidates, _computed) = profile.resolve_candidates(row, &engine);
        let ops = self.block_operations.clone();
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
            .map(|(_, stored)| holon::entity_profile::RowProfile {
                name: stored.name.clone(),
                render: stored.render.clone(),
                operations: ops,
                variants,
            })
    }

    fn compile_to_sql(&self, _: &str, _: holon_api::QueryLanguage) -> anyhow::Result<String> {
        panic!("compile_to_sql not supported on ReferenceState")
    }

    fn start_query(
        &self,
        _: String,
        _: Option<holon_frontend::QueryContext>,
    ) -> anyhow::Result<holon_frontend::RowChangeStream> {
        panic!("start_query not supported on ReferenceState")
    }

    fn widget_state(&self, _: &str) -> holon_frontend::config::WidgetState {
        holon_frontend::config::WidgetState::default()
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

    fn popup_query(
        &self,
        _: String,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = anyhow::Result<Vec<holon_api::widget_spec::DataRow>>>
                + Send
                + 'static,
        >,
    > {
        Box::pin(async { anyhow::bail!("popup_query not supported on ReferenceState") })
    }
}
