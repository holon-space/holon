//! Reference model for the PBT state machine.
//!
//! @pbt kind ref
//! @pbt oracle correspondence — the single `ReferenceState` oracle. Its
//!   `BuilderServices::interpret` REUSES the production `ShadowInterpreter`
//!   (render engine) driven from the ref's OWN block map (`get_block_data`),
//!   never SUT read-back: legitimate reused-not-under-test-engine oracle,
//!   deliberately blind to render-engine-internal bugs (covered by its own
//!   tier), sharp on the ref↔SUT projection axis.
//! @pbt covers block-tree/sibling-order — the structural mutation helpers
//!   (`move_block`/`outdent_block`/`swap_sequence`/`split_block`/`join_block`)
//!   each independently predict post-op sibling order. FIDELITY DRIFT:
//!   `move_block` deliberately SUPPRESSES the canonical re-sort (models the
//!   production fractional `sort_key`), whereas `outdent`/`swap`/content
//!   mutations funnel through `recanon_and_rebuild` →
//!   `assign_reference_sequences_canonical`, a HAND-mirror of the org
//!   `process_headlines` re-emission order (Source<Image<Text). A move
//!   followed by a same-parent content mutation re-canonicalizes and can
//!   silently reorder what the move placed — see the REF honesty-drift finding.

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use holon_api::ContentType;
use holon_api::EntityName;
use holon_api::Region;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_loro_testing::ref_ext::LoroRefExt;
use holon_pbt_core::Wiring;
use holon_pbt_core::capabilities::commit_active_editor_if_dirty;

use super::action_actor_state::ActionActorState;
use super::block_state::BlockState;
use super::block_state::LayoutBlockInfo;
use super::clock_state::ClockState;
use super::file_adapter_state::FileAdapterState;
use super::mcp_server_actor_state::MCPServerActorState;
use super::query::QuerySource;
use super::query::TestQuery;
use super::query::WatchSpec;
use super::reference_domain_state::ReferenceDomainState;
use super::ui_actor_state::UIActorState;
use super::ui_types::CursorPosition;
use crate::pbt::types::MutationApply;

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
        // list(#{item_template: row(state_toggle(col("task_state")),
        // editable_text(col("content")))})
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

/// The delegation node's name in its shadow-builder form. The only production
/// delegation form is the bare `live_block()`, which reaches the interpreter as
/// a `FunctionCall` resolving the target from the row's `id` column. The
/// explicit-target form (`live_block("block:x")`, which parses to
/// [`RenderExpr::LiveBlock`]) is seeded nowhere, so the ref asserts against it
/// rather than guessing at a model for it.
const LIVE_BLOCK_BUILDER: &str = "live_block";

/// Whether `expr` hands its rows to another block's own render via
/// `live_block()`.
pub fn contains_live_block(expr: &RenderExpr) -> bool {
    match expr {
        RenderExpr::LiveBlock { .. } => {
            unreachable!(
                "explicit-target live_block(..) is seeded nowhere; the ref models only bare live_block()"
            )
        }
        RenderExpr::FunctionCall { name, .. } if name == LIVE_BLOCK_BUILDER => true,
        RenderExpr::FunctionCall { args, .. } => args.iter().any(|a| contains_live_block(&a.value)),
        RenderExpr::Array { items } => items.iter().any(contains_live_block),
        RenderExpr::Object { fields } => fields.values().any(contains_live_block),
        RenderExpr::BinaryOp { left, right, .. } => {
            contains_live_block(left) || contains_live_block(right)
        }
        RenderExpr::ColumnRef { .. } | RenderExpr::Literal { .. } => false,
    }
}

/// Replace every `live_block()` delegation node in `expr` with `replacement`,
/// recursing through the whole render tree.
///
/// A `live_block()` template hands each row to that block's own render, which
/// the ref cannot evaluate — interpreting the raw node yields zero widgets and
/// makes every block-interaction transition look impossible. The delegate a
/// focus root without its own query resolves to is the profile collection over
/// its subtree, so `main_rendered_block_ids` already expands the ROWS to that
/// subtree; this is the same model applied to the TEMPLATE those rows render
/// through.
pub fn substitute_live_block(expr: RenderExpr, replacement: &RenderExpr) -> RenderExpr {
    use holon_api::render_types::Arg;
    match expr {
        RenderExpr::LiveBlock { .. } => {
            unreachable!(
                "explicit-target live_block(..) is seeded nowhere; the ref models only bare live_block()"
            )
        }
        RenderExpr::FunctionCall { ref name, .. } if name == LIVE_BLOCK_BUILDER => {
            replacement.clone()
        }
        RenderExpr::FunctionCall { name, args } => RenderExpr::FunctionCall {
            name,
            args: args
                .into_iter()
                .map(|a| Arg {
                    name: a.name,
                    value: substitute_live_block(a.value, replacement),
                })
                .collect(),
        },
        RenderExpr::Array { items } => RenderExpr::Array {
            items: items
                .into_iter()
                .map(|i| substitute_live_block(i, replacement))
                .collect(),
        },
        RenderExpr::Object { fields } => RenderExpr::Object {
            fields: fields
                .into_iter()
                .map(|(k, v)| (k, substitute_live_block(v, replacement)))
                .collect(),
        },
        RenderExpr::BinaryOp { op, left, right } => RenderExpr::BinaryOp {
            op,
            left: Box::new(substitute_live_block(*left, replacement)),
            right: Box::new(substitute_live_block(*right, replacement)),
        },
        other @ (RenderExpr::ColumnRef { .. } | RenderExpr::Literal { .. }) => other,
    }
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
            "entity_name: block\ncomputed:\n  has_{field}: \"= {field} != ()\"\nvariants:\n  - \
             name: {name}\n    priority: 1\n    condition: \"= has_{field}\"\n    render: \
             'row(editable_text(col(\"content\")))'\n  - name: default\n    priority: -1\n    \
             render: 'row(editable_text(col(\"content\")))'",
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

const NO_VARIANTS_YAML: &str = "entity_name: block\ncomputed: {}\nvariants:\n  - name: default\n    priority: -1\n    \
     render: 'row(editable_text(col(\"content\")))'";

pub static VALID_PROFILE_YAMLS: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| {
    let mut yamls = vec![NO_VARIANTS_YAML.to_string()];
    for tep in TEST_PROFILES {
        yamls.push(tep.to_yaml());
    }
    yamls
});

/// Harness-environment residue of the reference model (RefStateSplit Inc 3).
///
/// These fields are **not model state** — they are the async runtime, the
/// wiring manifest, the composed cap set, the real-editor driver flag, and the
/// shadow interpreter that the *harness* threads through the reference.
/// Gathering them here (rather than leaving them loose on [`ReferenceState`])
/// puts their `Clone` semantics in one visible place: proptest clones the
/// reference per step and per case, so `runtime`/`interpreter` are `Arc`-shared
/// (cheap clone, shared cell) while `wiring`/`cap_set`/`real_editor` are plain
/// values that clone by copy.
///
/// `clock_feed` is NOT here: it moved with the Loro extension into
/// [`LoroRefExt`] (RefStateSplit Inc 5), where its `Clone`-SHARES-the-cell seam
/// is documented alongside the shadow mesh it drives.
#[derive(Debug, Clone)]
pub struct HarnessEnv {
    /// Runtime for async operations. `Arc`-shared across clones.
    pub runtime: Arc<tokio::runtime::Runtime>,

    /// The wiring manifest this reference run was built for (which storage
    /// adapters, sync adapters, and actors are present). Drives the
    /// `enable_loro()` capability check and per-transition / per-invariant
    /// `RequiredWiring` gating (ADR 0007).
    pub wiring: Wiring,

    /// The capability set the SUT supplies, when the SUT is a composed
    /// `CapMap`. `None` = unrestricted: a concrete SUT (`E2ESut`) provides
    /// every cap, or this is a non-composed run, so the cap gate passes
    /// everything and the alphabet behaves exactly as before. `Some(set)` =
    /// a composed/partial SUT, so transitions whose
    /// [`TransitionFactory::required_caps`] aren't all present are gated out of
    /// the alphabet — the cap-analog of
    /// [`wiring`](Self::wiring)/`RequiredWiring` (PCG-2).
    pub cap_set: Option<holon_pbt_core::composition::CapSet>,

    /// Whether a **real editor** (a live `InputState` driven by the GPUI/TUI
    /// `UserDriver`) — not the headless `HeadlessEditorMirror` — drives the
    /// SUT. Set by the real-editor driver harness, which builds the
    /// reference state directly. When true,
    /// [`ReferenceState::blur_active_editor`] commits the editor's dirty
    /// buffer to block content on blur, mirroring prod's `on_blur` →
    /// `set_field("content")`. Replaces the former process-global
    /// `PBT_REAL_EDITOR` env gate — the property now lives on the state the
    /// driver constructs, so it is deterministic and capture/replay-faithful
    /// without an env-var side channel. Headless slices leave it `false`.
    pub real_editor: bool,

    /// Shadow interpreter resolved from FluxDI — source of truth for widget
    /// names and render DSL parsing. `Arc`-shared across clones.
    pub interpreter: Arc<ShadowInterpreter>,

    /// Memoized profile engine — see [`ProfileEngineCache`]. Empty in a fresh
    /// clone, so it never carries another state's engine.
    pub profile_engine: ProfileEngineCache,

    /// The reference's answer to "is this link scheme registered?" — built-in
    /// schemes only, since the reference carries no live type registry.
    pub link_classifier: holon_api::link_parser::LinkTargetClassifier,
}

/// Memo for [`ReferenceState::profile_engine`], keyed by a fingerprint of the
/// source-block projection the entity lookups read.
///
/// The key is derived from the very data the engine wraps, so a stale engine is
/// unrepresentable: mutating any source block's id, parent or language changes
/// the fingerprint and forces a rebuild. Without the memo, `resolve_profile`
/// rebuilt the whole engine per ROW — O(rows × blocks) per snapshot.
///
/// `Clone` yields an EMPTY cell rather than sharing one: proptest clones the
/// reference per step, and two clones that then diverge would otherwise evict
/// each other's engine on every call.
#[derive(Default)]
pub struct ProfileEngineCache(std::sync::Mutex<Option<(u64, Arc<rhai::Engine>)>>);

impl Clone for ProfileEngineCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl std::fmt::Debug for ProfileEngineCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ProfileEngineCache")
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
    /// doc_uri -> filename mapping plus pre-startup file/VCS boot flags. An
    /// adapter concern (how org/markdown persist a document on disk), distinct
    /// from domain identity.
    pub files: FileAdapterState,

    /// Tier-3 UI actor fragment (ADR 0004/0006 Phase 3): navigation history,
    /// pins, per-region focus + cursor, view selection, drawer/toggle
    /// open-state, active-editor mirror. Extracted so a non-UI wiring drops
    /// the whole fragment instead of carrying dead fields.
    pub ui: UIActorState,

    /// Harness environment (RefStateSplit Inc 3): runtime, wiring, cap set,
    /// real-editor flag, shadow interpreter. NOT model state — see
    /// [`HarnessEnv`].
    pub harness: HarnessEnv,

    /// Loro-private extension (RefStateSplit Inc 5): peer instances, the
    /// E-solid shadow CRDT mesh, and the Lamport `clock_feed` side-channel.
    /// Co-located in `holon-loro-testing` ([`LoroRefExt`]); the `RefPeers(Mut)`
    /// cap impls in `ref_caps/peers.rs` delegate here (orphan rule). The two
    /// Clone seams (`clock_feed` shares the cell; `shadow_mesh` deep-forks) are
    /// documented at that home.
    pub loro: LoroRefExt,

    /// Calendar-clock model for the `AdvanceDay` transition (ADR 0024 §6).
    pub clock: ClockState,

    /// Sharing overlay (ADR 0028 C2/H3): per-block policy audience + per-doc
    /// effective container audience + sharing epoch. Empty until a crossing
    /// transition writes it; read by `inv-audience-never-over-approximates`
    /// through the `RefAudience` cap.
    pub sharing: super::sharing_state::SharingRefState,

    /// C2 history-oracle expectation (NOT model state proper): populated by the
    /// harness `run_report` from the id-reconcile map. `history_ever_created`
    /// = every real id the oracle minted (anchor for the phantom-history subset
    /// check); `history_min_op_groups` = the UI-driven create count the SUT's
    /// `block_history` must meet or exceed (missed-history lower bound). Empty
    /// / zero on a bare state; the harness fills them just before the
    /// check.
    pub history_ever_created: BTreeSet<EntityUri>,
    pub history_min_op_groups: usize,

    /// Undo→redo burned-id oracle (NOT model state proper): the real block ids
    /// the harness reconcile retired because a `Redo` re-minted their block
    /// under a fresh uuid. Populated by `run_report` alongside the C2 fields;
    /// empty on a bare state and on every run without a completed round trip.
    /// Read by `inv-undo-redo-reference-heal` through `RefUndoRedoBurned`.
    pub undo_redo_burned_ids: BTreeSet<EntityUri>,

    /// Page paths (`/`-joined, root->leaf) a `RenamePage` has VACATED -- the
    /// temporal fuel for `CreatePageAtFreedPath`.
    ///
    /// Page ids are `blake3(path)` (`PageId::for_path`), so a path can only be
    /// re-minted after something frees it. Nothing else in the reference
    /// records that a name once belonged to a page that no longer carries it: a
    /// rename leaves the entity in place under its NEW title, and every other
    /// name-producing transition draws from a monotonic counter. Without this
    /// ledger the generator can never reach the "name freed, then reused"
    /// state -- exactly the shape `PageIdentityDeterminism.md` 5.3 speaks to.
    ///
    /// Append-only within a run and read through
    /// [`holon_pbt_core::capabilities::RefPageIdentity::freed_page_paths`],
    /// which filters out any path a page has since re-occupied.
    pub renamed_away_page_paths: Vec<String>,

    /// Datatype-axis oracle (BG-1): the free-standing typed entities the model
    /// has created, keyed by TYPE. Read by `RefTypedEntities`, compared against
    /// each type's matview by `inv-typed-matview-matches-ref`. Carries no type
    /// name of its own — the set comes from the registry.
    pub typed_entities: TypedEntitiesRefState,
}

/// Reference model of the datatype axis (BG-1): which free-standing types are
/// DECLARED, and which entities exist for each.
///
/// Types are not a fixed list. The registry's own free-standing types seed this
/// at construction, and `DeclareTypedSchema` adds runtime-declared ones with
/// proptest-drawn shapes — so the axis exercises arbitrary schemas, not one
/// hand-authored type. Nothing here names a type.
#[derive(Debug, Clone, Default)]
pub struct TypedEntitiesRefState {
    /// type name -> value columns, in the schema's order (the primary key is
    /// always `id` and is not listed).
    schemas: BTreeMap<String, Vec<String>>,
    /// type name -> its `computed_persisted` fields, each with the
    /// `Computation` the declaration compiled into. The oracle EVALUATES these
    /// where the SUT reads a planted matview column.
    computed: BTreeMap<String, Vec<(String, holon_api::computation::Computation)>>,
    /// type name -> entity id -> value cells, aligned with `schemas`.
    by_type: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

impl TypedEntitiesRefState {
    /// Declare a type the SUT has serialized. Idempotent per name; the
    /// generator only ever proposes undeclared names.
    pub fn declare(
        &mut self,
        type_name: String,
        value_columns: Vec<String>,
        computed: Vec<(String, holon_api::computation::Computation)>,
    ) {
        self.schemas.insert(type_name.clone(), value_columns);
        self.computed.insert(type_name, computed);
    }

    /// The full column list for a type: stored columns then computed ones, in
    /// the order `rows` emits their cells.
    pub fn columns(&self, type_name: &str) -> Vec<String> {
        let mut cols = self.schemas.get(type_name).cloned().unwrap_or_default();
        cols.extend(self.computed_of(type_name).iter().map(|(n, _)| n.clone()));
        cols
    }

    /// The type's computed fields, empty for a type that declares none.
    fn computed_of(&self, type_name: &str) -> &[(String, holon_api::computation::Computation)] {
        self.computed.get(type_name).map_or(&[], Vec::as_slice)
    }

    /// Whether a type has been declared (a create's precondition).
    pub fn is_declared(&self, type_name: &str) -> bool {
        self.schemas.contains_key(type_name)
    }

    /// Every declared type with its value columns, in a stable order.
    pub fn declared(&self) -> impl Iterator<Item = (&String, &Vec<String>)> {
        self.schemas.iter()
    }

    /// How many types are declared — the generator's name counter.
    pub fn declared_count(&self) -> usize {
        self.schemas.len()
    }

    /// Record an entity the oracle created. `values` are the schema's value
    /// columns in order (the id is the key, not a value).
    pub fn add(&mut self, type_name: String, id: String, values: Vec<String>) {
        let stored = Self::stored_id(&type_name, &id);
        self.by_type
            .entry(type_name)
            .or_default()
            .insert(stored, values);
    }

    /// The id the write authority STORES for a create. An id arriving without
    /// a scheme belongs to the entity being written, so the authority
    /// qualifies it with that entity's own scheme — mirroring
    /// `EntityUri::from_raw_for` on the create boundary. (The raw parse used to
    /// default every unschemed id to `block:`, filing a free-standing row
    /// under a scheme it does not have.)
    fn stored_id(type_name: &str, id: &str) -> String {
        if id.contains(':') {
            id.to_string()
        } else {
            // The CANONICAL entity name is the scheme (`EntityName` folds `_`
            // to `-`, since a scheme carrying an underscore is not a valid URI
            // scheme) — the same name the write authority routes by.
            let scheme = holon_api::EntityName::new(type_name);
            format!("{}:{id}", scheme.as_str())
        }
    }

    /// How many entities of this type exist (the generator's id counter).
    pub fn count(&self, type_name: &str) -> usize {
        self.by_type.get(type_name).map_or(0, BTreeMap::len)
    }

    /// Expected rows for one type as `[id, ..values, ..computed]`, canonically
    /// sorted to match the SUT matview read. A declared-but-empty type expects
    /// no rows.
    ///
    /// The computed cells are produced by evaluating the declaration's
    /// `Computation` over the row — deliberately NOT by restating its SQL, so
    /// the invariant compares Holon's two lowerings of one declaration against
    /// each other.
    pub fn rows(&self, type_name: &str) -> Vec<Vec<String>> {
        let value_columns = self.schemas.get(type_name).cloned().unwrap_or_default();
        let mut rows: Vec<Vec<String>> = self
            .by_type
            .get(type_name)
            .map(|entities| {
                entities
                    .iter()
                    .map(|(id, values)| {
                        let mut row = vec![id.clone()];
                        row.extend(values.iter().cloned());
                        row.extend(self.computed_cells(type_name, &value_columns, id, values));
                        row
                    })
                    .collect()
            })
            .unwrap_or_default();
        rows.sort();
        rows
    }

    /// Evaluate each of the type's computed fields over one row's cells. Every
    /// value the axis writes is TEXT, so the context is uniformly
    /// `Value::String` — the same shape the planted SQL sees.
    fn computed_cells(
        &self,
        type_name: &str,
        value_columns: &[String],
        id: &str,
        values: &[String],
    ) -> Vec<String> {
        let computed = self.computed_of(type_name);
        if computed.is_empty() {
            return Vec::new();
        }
        let mut ctx = holon_api::computation::Context::new();
        ctx.insert("id".to_string(), holon_api::Value::String(id.to_string()));
        for (column, value) in value_columns.iter().zip(values) {
            ctx.insert(column.clone(), holon_api::Value::String(value.clone()));
        }
        computed
            .iter()
            .map(|(name, computation)| {
                match computation.eval(&ctx).unwrap_or_else(|e| {
                    panic!("oracle cannot evaluate '{type_name}.{name}' over {ctx:?}: {e}")
                }) {
                    holon_api::Value::String(s) => s,
                    other => panic!(
                        "oracle expects '{type_name}.{name}' to evaluate to a string — the axis \
                         reads matview cells as text — but it produced {other:?}"
                    ),
                }
            })
            .collect()
    }

    /// Every id the oracle created, across all types — the identity check
    /// asserts none of them reaches a block table.
    pub fn all_ids(&self) -> impl Iterator<Item = &String> {
        self.by_type.values().flat_map(BTreeMap::keys)
    }
}

/// Witness that a [`ReferenceState`]'s ids live in the SUT's id space — either
/// [`ReferenceState::with_resolved_doc_uris`] has run (synthetic
/// `block::split-N` / `block:ref-doc-N` placeholders remapped to the SUT's real
/// ids), or the ref was built over a backend that mints no new ids (see
/// [`Resolved::identity`]).
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
    /// `Resolved<ReferenceState>` → `Resolved<Arc<ReferenceState>>`. The
    /// closure only repackages an already-resolved value, so the witness
    /// still holds.
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
    /// Witness that no id resolution is needed: the reference state was built
    /// over a backend that mints no fresh ids (seed/started refs, or the
    /// counter-sync `MemoryBackend` whose `align_ids` keeps mints in
    /// lockstep with the oracle), so the synthetic and real id spaces
    /// already coincide.
    pub fn identity(state: ReferenceState) -> Self {
        Resolved(state)
    }
}

impl ReferenceState {
    /// The oracle's Rhai engine for evaluating the bundled `block` profile.
    ///
    /// The bundled profile's computed fields call entity lookups
    /// (`query_source(id)`, `rule_sibling(id)`), which production registers on
    /// the ProfileResolver's engine from live entities. The oracle predicts the
    /// SAME profile, so it registers them through the SAME seat
    /// (`holon_profiles::build_lookup_engine`) — backed by the model's own
    /// block tree, not by prod's matviews.
    ///
    /// Memoized per source-block fingerprint ([`ProfileEngineCache`]): the
    /// render path resolves a profile per row, and rebuilding the engine each
    /// time walks the whole block map.
    pub fn profile_engine(&self) -> Arc<rhai::Engine> {
        let fingerprint = self.source_block_fingerprint();
        let mut slot = self.harness.profile_engine.0.lock().unwrap();
        if let Some((cached, engine)) = slot.as_ref()
            && *cached == fingerprint
        {
            return Arc::clone(engine);
        }
        let engine = Arc::new(self.build_profile_engine());
        *slot = Some((fingerprint, Arc::clone(&engine)));
        engine
    }

    /// Hash of exactly what the lookups read: every source block's id, parent
    /// and language. Two states agreeing here cannot disagree on any lookup.
    fn source_block_fingerprint(&self) -> u64 {
        use std::hash::Hash;
        use std::hash::Hasher;

        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for block in self.domain.block_state.blocks.values() {
            if block.content_type != ContentType::Source {
                continue;
            }
            let Some(lang) = block.source_language.as_ref() else {
                continue;
            };
            block.id.as_str().hash(&mut hasher);
            block.parent_id.as_str().hash(&mut hasher);
            lang.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// The model's answer to production's live entities: the SAME
    /// [`holon_profiles::LiveEntitySpec`] the Turso DI wiring and the Loro
    /// session build theirs from, applied to the model's block map.
    fn build_profile_engine(&self) -> rhai::Engine {
        let entities: holon_profiles::LiveEntities = holon_profiles::LiveEntitySpec::ALL
            .iter()
            .map(|spec| {
                (
                    spec.entity_name(),
                    spec.live_data_from_blocks(self.domain.block_state.blocks.values()),
                )
            })
            .collect();
        holon_profiles::build_lookup_engine(&entities)
    }

    /// Return a [`Resolved`] clone of this reference state with its block tree
    /// remapped into the SUT's ID space via `map` (synthetic doc URI → real
    /// UUID). Capability-bound invariant bodies run against this resolved
    /// view so they can compare block IDs/parents directly against the SUT
    /// without any per-comparison resolution.
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
        // Sharing overlay keys are block/doc uris; remap into SUT id space so the
        // audience oracle reads SUT-keyed audiences. A no-op on the empty default.
        resolved.sharing = self.sharing.remapped(map);
        // Tracked documents are keyed by doc uri, and `block_state` above is
        // SUT-keyed: synthetic keys here would make a resolved block's document
        // unreachable from the block itself (`RefDocuments::file_home_of`).
        resolved.files.documents = self
            .files
            .documents
            .iter()
            .map(|(uri, name)| (resolve(uri), name.clone()))
            .collect();
        Resolved(resolved)
    }

    pub fn new(wiring: Wiring, interpreter: Arc<ShadowInterpreter>) -> Self {
        Self {
            domain: ReferenceDomainState::new(),
            action: ActionActorState::new(),
            mcp: MCPServerActorState::new(),
            files: FileAdapterState::new(),
            ui: UIActorState::new(),
            harness: HarnessEnv {
                runtime: Arc::new(tokio::runtime::Runtime::new().unwrap()),
                wiring,
                cap_set: None,
                real_editor: false,
                interpreter,
                profile_engine: ProfileEngineCache::default(),
                link_classifier: holon_api::link_parser::LinkTargetClassifier::default(),
            },
            loro: LoroRefExt::default(),
            clock: ClockState::new(),
            sharing: super::sharing_state::SharingRefState::default(),
            history_ever_created: BTreeSet::new(),
            history_min_op_groups: 0,
            undo_redo_burned_ids: BTreeSet::new(),
            renamed_away_page_paths: Vec::new(),
            typed_entities: {
                // Seed the types the app registers at boot (the registry's
                // free-standing set). Runtime-declared types join them via
                // `DeclareTypedSchema`.
                let mut t = TypedEntitiesRefState::default();
                for schema in crate::pbt::typed_entity_schemas::free_standing_schemas() {
                    t.declare(
                        schema.type_name.clone(),
                        schema.value_columns.clone(),
                        schema.computed_columns.clone(),
                    );
                }
                t
            },
        }
    }

    /// Set the composed SUT's capability set (the cap gate's RHS). The composed
    /// harness calls this with `caps.cap_set()` so the alphabet
    /// auto-narrows to the transitions whose caps the `CapMap` actually
    /// supplies. Concrete-SUT runs leave it `None`.
    pub fn with_cap_set(mut self, cap_set: holon_pbt_core::composition::CapSet) -> Self {
        self.harness.cap_set = Some(cap_set);
        self
    }

    /// Whether the active SUT supplies every cap in `required` — the cap gate
    /// mirroring `RequiredWiring::satisfied_by(&self.harness.wiring)`.
    /// Unrestricted (`cap_set == None`) always passes, so concrete-SUT runs
    /// gate nothing (regression-safe). **Necessary, not sufficient**,
    /// exactly like the wiring gate.
    pub fn caps_available(&self, required: &[holon_pbt_core::composition::CapId]) -> bool {
        match &self.harness.cap_set {
            None => true,
            Some(set) => required.iter().all(|cap| set.contains(cap)),
        }
    }

    /// Whether the Loro CRDT storage adapter is wired (the reference-side
    /// capability check). Inherent mirror of the `RefLifecycle::enable_loro`
    /// trait method so transition bodies that hold a concrete
    /// `&ReferenceState` can read it without importing the trait.
    pub fn enable_loro(&self) -> bool {
        self.harness
            .wiring
            .has_storage(holon_pbt_core::StorageAdapter::Loro)
    }

    /// Whether this reference owns an editor buffer carrying uncommitted text —
    /// the headless atomic-editor capability (the single editor-transition
    /// gate; see [`RefLifecycle::has_editor_buffer`]). Inherent mirror of
    /// the trait method so transition bodies holding a concrete
    /// `&ReferenceState` can read it without importing the trait.
    ///
    /// Two honest sources, never Loro-as-storage or an env var:
    /// - `Actor::UI` — a real window hosts the editor's `InputState`. The only
    ///   source for refs with no composed SUT behind them (the fixed-wiring lib
    ///   slices), so their gating is unchanged.
    /// - the composed SUT actually hosting `SutEditorMirrorWrite`.
    ///   `compose_sut` FORBIDS `Actor::UI` (a GPUI window has thread affinity),
    ///   so the manifest alone can never admit an editor transition into the
    ///   composed keystone — yet its frontend arm runs the production
    ///   `HeadlessEditorMirror` in BOTH storage modes. Reading the cap set
    ///   makes the gate say what it means: "an editor is drivable here".
    pub fn has_editor_buffer(&self) -> bool {
        self.harness.wiring.has_actor(holon_pbt_core::Actor::UI)
            || (self.harness.cap_set.is_some()
                && self.caps_available(&[holon_pbt_core::composition::CapId::of::<
                    dyn holon_pbt_core::capabilities::SutEditorMirrorWrite,
                >()]))
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
    /// no editor is active, so call sites can swap a bare `active_editor =
    /// None` for this unconditionally.
    pub fn blur_active_editor(&mut self) {
        // Dirty-gated: only user-authored pending text commits on an
        // authority move (prod commits via the focus-binding's
        // authority-left arm — deterministic, window-activation-independent).
        // A clean mirror that merely diverged from block.content is stale
        // against an external change and must not be committed.
        if self.harness.real_editor && self.ui.tab.active_editor.as_ref().is_some_and(|e| e.dirty) {
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
        if !self.domain.block_state.blocks.contains_key(&block_id) {
            return false;
        }
        // The buffer is the block's SOURCE PROJECTION, so the commit routes
        // exactly as prod's editor routes it: the source channel re-derives
        // content AND task state, the content channel writes one column.
        let id = super::ref_caps::cap_id(&block_id);
        let surface = {
            use holon_pbt_core::capabilities::RefBlockTreeMut;
            self.editor_surface_text(&id)
        };
        if surface == in_memory {
            if let Some(e) = self.ui.tab.active_editor.as_mut() {
                e.dirty = false;
            }
            return false;
        }
        {
            use holon_pbt_core::capabilities::RefBlockTreeMut;
            if holon_org_format::source_channel_commit(&surface, &in_memory) {
                self.commit_editor_source(&id, &in_memory);
            } else {
                self.set_block_content(&id, &in_memory);
            }
        }
        if let Some(e) = self.ui.tab.active_editor.as_mut() {
            e.dirty = false;
        }
        true
    }

    pub fn current_focus(&self, region: Region) -> Option<EntityUri> {
        self.ui.tab.current_focus(region)
    }

    pub fn can_go_back(&self, region: Region) -> bool {
        self.ui.tab.can_go_back(region)
    }

    /// If `block_id` is the focused entity in any region, reset the cursor to
    /// start. Called after mutations that change block content — the real
    /// editor would reposition the cursor (blur/refocus cycle), so the
    /// reference model must too.
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

    /// If a CLEAN active editor is open on `block_id`, refresh its in-memory
    /// text to the block's current content. Mirrors prod's data subscription:
    /// the live editor cell (`editable_text(block, "content").current()`, the
    /// exact source `editor_live_text` reads) IS the block's content container,
    /// so an EXTERNAL content change to an idle (never-typed) editor surfaces
    /// in the live text immediately. A DIRTY editor holds user-authored
    /// pending text that prod's subscription does NOT clobber (the
    /// split-with-pending-edit contract), so it is left untouched. Without
    /// this, a clean editor opened at a split product (e.g. content "2")
    /// then hit by an external `Update{content:"a"}` leaves the ref's
    /// `active_editor.in_memory_content` stale at "2" while the SUT cell
    /// already reads "a" — the `inv-editor-text/mirror` residual (editor
    /// stale-buffer family).
    ///
    /// Only the live TEXT is refreshed, not the caret: the SUT's caret mirror
    /// (`HeadlessEditorMirror::tracked_cursor`) is advanced solely by
    /// keystrokes, so an external content change (no keystroke) leaves it
    /// untouched — the ref must do the same or `inv-editor-caret/mirror` would
    /// diverge on a `MoveCursor`-then-external-update sequence.
    pub fn refresh_clean_active_editor(&mut self, block_id: &EntityUri) {
        let Some(block) = self.domain.block_state.blocks.get(block_id) else {
            return;
        };
        let content_type = block.content_type;
        // A clean editor re-seeds from the AUTHORITY AS THE SURFACE SHOWS IT —
        // vault syntax — because that is what prod's convergence targets.
        let new_content = {
            use holon_pbt_core::capabilities::RefBlockTreeMut;
            let id = super::ref_caps::cap_id(block_id);
            self.editor_surface_text(&id)
        };
        if let Some(editor) = self.ui.tab.active_editor.as_mut()
            && &editor.block_id == block_id
            && !editor.dirty
        {
            // Seq/trim-discriminator-aware echo model (Inc 4, EditorBufferOwnership
            // plan). A clean editor is normally re-seeded to the block's stored
            // content — prod's data subscription refreshes idle editors. BUT the
            // stored content may be the SQL trailing-whitespace canonicalization of
            // THIS editor's OWN just-committed text, echoed back carrying the SAME
            // `write_seq` (non-editor writers do NOT bump `write_seq`, so seq alone
            // cannot distinguish an own-echo from a genuine external write — the
            // TRIM SHAPE is the load-bearing discriminator, mirroring the SUT's
            // `evaluate_data_sync_echo`). When the divergence is EXACTLY that
            // canonicalization, prod keeps the typed buffer
            // (`EchoDecision::AdoptBaseline`) instead of regressing the trailing
            // whitespace, so the ref must not regress it either; any SUBSTANTIVE
            // external change (split/join/org/peer) still converges (refreshes).
            let (canonical, _) = super::types::normalize_content_for_org_roundtrip(
                &editor.in_memory_content,
                content_type,
            );
            let is_own_trailing_ws_echo =
                canonical == new_content && editor.in_memory_content != new_content;
            if !is_own_trailing_ws_echo {
                // A converge can SHORTEN the surface (the store canonicalizes
                // `TODO  milk` into the task `milk`), so the caret is clamped
                // onto it exactly as prod's `preserved_caret` clamps.
                editor.cursor_byte =
                    holon_frontend::editor_caret::clamp_boundary(&new_content, editor.cursor_byte);
                editor.in_memory_content = new_content;
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
        // and a lingering ActiveEditor makes inv-editor-text/mirror
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

    /// Whether any region currently has a focused entity (required for
    /// ArrowNavigate).
    pub fn has_focus(&self) -> bool {
        self.ui.tab.has_focus()
    }

    /// Get the focused entity in a region (set by ClickBlock).
    pub fn focused_entity(&self, region: Region) -> Option<&EntityUri> {
        self.ui.tab.focused_entity(region)
    }

    pub fn can_go_forward(&self, region: Region) -> bool {
        self.ui.tab.can_go_forward(region)
    }

    pub fn current_view(&self) -> String {
        self.ui.user.current_view()
    }

    /// Returns expected query results for a watch using the TestQuery
    /// evaluator.
    pub fn query_results(&self, watch_spec: &WatchSpec) -> Vec<HashMap<String, Value>> {
        self.domain.query_results(watch_spec)
    }

    /// Check if index.org exists with the structure required by
    /// initial_widget(). Generate a synthetic `block:ref-doc-N` URI for a
    /// new document and bump the counter.
    pub fn next_synthetic_doc_uri(&mut self) -> EntityUri {
        self.action.next_synthetic_doc_uri()
    }

    /// Find a page block by its title (first line of content, e.g. "index").
    pub fn doc_uri_by_name(&self, title: &str) -> Option<EntityUri> {
        self.domain.block_state.doc_uri_by_name(title)
    }

    /// Whether the system has a valid root layout (from seed blocks or
    /// user-written index.org). Used to gate render_entity, ReactiveEngine,
    /// and ViewModel checks.
    pub fn is_properly_setup(&self) -> bool {
        self.domain.is_properly_setup()
    }

    /// Whether the user has written an index.org with query+render blocks.
    /// Used to gate block comparison invariants (seed blocks don't round-trip
    /// through org files).
    pub fn has_user_index_org(&self) -> bool {
        self.domain.has_user_index_org()
    }

    /// Get the first root layout block ID from index.org (a heading with a
    /// query source child).
    pub fn root_layout_block_id(&self) -> Option<EntityUri> {
        self.domain.root_layout_block_id()
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

    /// The active main-panel render expression with both indirections prod
    /// resolves before the frontend ever interprets it substituted away, so the
    /// oracle's interpreted tree matches the SUT's.
    ///
    /// Two nodes stand for a render the raw expression does not contain:
    /// `collection_view()`, which prod's `render_for_block` swaps for the
    /// profile-derived collection, and `live_block()`, which hands the row to
    /// another block's own render. Both resolve to the ref's canonical default
    /// collection — every visible row renders `render_entity()`, whose block
    /// profile carries the `state_toggle`, the `editable_text` and the
    /// `draggable`.
    pub fn resolved_main_panel_render_expr(&self) -> RenderExpr {
        let expr = self
            .main_panel_render_expr()
            .or_else(|| self.root_render_expr())
            .cloned()
            .unwrap_or_else(default_root_render_expr);

        let expr = if holon::api::block_domain::contains_collection_view(&expr) {
            holon::api::block_domain::substitute_collection_view(expr, &default_root_render_expr())
        } else {
            expr
        };

        if contains_live_block(&expr) {
            substitute_live_block(expr, &fc("render_entity", vec![]))
        } else {
            expr
        }
    }

    /// Whether the active main-panel layout's item template renders
    /// `block_id`'s row with any of `widgets` (by `widget_name`). Renders
    /// the row through the shadow interpreter (the same
    /// `BuilderServices::interpret` the SUT uses) and walks the resulting
    /// `ViewModel`. Returns `false` when no main-panel render template is
    /// tracked (the template-interactivity axis is only consulted for user
    /// `index.org` layouts, which always have one).
    fn main_layout_renders_widget(&self, block_id: &EntityUri, widgets: &[&str]) -> bool {
        use holon_frontend::reactive::BuilderServices;

        if self.main_panel_render_expr().is_none() && self.root_render_expr().is_none() {
            return false;
        }
        let expr = self.resolved_main_panel_render_expr();
        let Some(item_template) = holon_frontend::reactive_view_model::extract_item_template(&expr)
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
    /// Default layout (no user `index.org`) → [`QuerySource::FocusRootOnly`],
    /// the form `assets/default/index.org` seeds: the panel selects the
    /// focus-root row alone and delegates its subtree to that root's own
    /// render via `live_block()`. A user `index.org` → the [`QuerySource`]
    /// recovered from its main-panel query source block (via
    /// [`QuerySource::recognize`]), bound to the layout block as the
    /// navigation-blind `from children` context.
    pub fn active_main_query(&self) -> TestQuery {
        if !self.has_user_index_org() {
            return TestQuery::layout(QuerySource::FocusRootOnly {
                region: Region::Main.as_str().to_string(),
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

    /// Block ids the active main-panel layout renders — its query's rendered
    /// set. The faithful replacement for the
    /// `is_descendant_of_any(focus_roots)` proxy: it agrees with the
    /// default layout (focus-root descendants) and is correct for custom
    /// layouts (a `from children` layout renders only the layout block's
    /// direct children, an all-blocks layout renders everything).
    pub fn main_rendered_block_ids(&self) -> BTreeSet<EntityUri> {
        let query = self.active_main_query();
        let mut focus_roots = std::collections::BTreeMap::new();
        focus_roots.insert("main".to_string(), self.rendered_focus_root(Region::Main));
        let mut ids: BTreeSet<EntityUri> = query
            .rendered_block_ids(&self.domain.block_state.blocks, &focus_roots)
            .into_iter()
            .collect();

        // A panel whose query selects only the focus-root row delegates the
        // subtree to that root's own render, so the rendered set is the row
        // plus what the delegate draws — the same page-stopping walk the
        // backend synthesizes for a root that authors no query of its own.
        if let QuerySource::FocusRootOnly { region } = &query.source {
            // Journals delegates to its `journal_feed` (the Page-tagged day
            // pages), not a descendant walk from journals: a non-page child of
            // journals is absent from the feed and renders nothing.
            let mut delegated_roots = focus_roots.clone();
            if delegated_roots
                .get(region)
                .is_some_and(|r| r.contains(&EntityUri::block("journals")))
            {
                delegated_roots.insert(region.clone(), self.journal_feed_day_pages());
            }
            let delegated = TestQuery::layout(QuerySource::FocusRootDescendants {
                region: region.clone(),
                max_depth: crate::pbt::query::MAIN_PANEL_MAX_DEPTH,
                stop_at_pages: true,
            });
            ids.extend(
                delegated.rendered_block_ids(&self.domain.block_state.blocks, &delegated_roots),
            );
        }
        ids
    }

    /// Whether the active layout's item template renders blocks interactively —
    /// i.e. with a widget a block-interaction transition can dispatch against.
    ///
    /// The default layout renders every block through the block entity
    /// profile's `render_entity()` (which attaches operations, a
    /// `draggable`, and an `editable_text`) — interactive by construction.
    /// A user `index.org` renders through an explicit template that may be
    /// static (`row(text(…))`, no operations) or interactive; we walk the
    /// actual template to decide.
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
        self.domain.root_render_expr()
    }

    /// Name of the active render expression for `region` (e.g. "tree",
    /// "outline", "list"). Used by `build_reference_navigator` to pick
    /// the right `CollectionNavigator` shape for arrow-key navigation.
    pub fn active_render_expr_name(&self, region: Region) -> Option<String> {
        self.domain.active_render_expr_name(region)
    }

    /// Build a reference-state `CollectionNavigator` for `region` to mirror
    /// what production's arrow-key handler would walk. Tree- and outline-
    /// layouts use `TreeNavigator`; everything else uses `ListNavigator`.
    pub fn build_reference_navigator(
        &self,
        region: Region,
    ) -> Option<Box<dyn holon_frontend::navigation::CollectionNavigator>> {
        use holon_frontend::navigation::ListNavigator;
        use holon_frontend::navigation::TreeNavigator;

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
                self.domain.block_state.collect_dfs_order(
                    &focus_id,
                    &mut dfs_order,
                    &mut parent_map,
                );
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

    /// Block IDs whose `content` must NEVER be mutated by an edit transition:
    /// query / render source blocks (would corrupt the active layout) and
    /// entity-profile blocks (typed YAML, not free-form text).
    pub fn no_content_update_set(&self) -> std::collections::HashSet<EntityUri> {
        self.domain.no_content_update_set()
    }

    /// Stable IDs of blocks any peer has modified. JoinBlock excludes these
    /// to avoid edit/peer interleaving races. Delegates to the Loro ext.
    pub fn peer_modified_stable_ids(&self) -> std::collections::HashSet<String> {
        self.loro.all_modified_stable_ids()
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
        let focus_roots = self.rendered_focus_root(Region::Main);
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
    ///
    /// VISIBILITY is decided by the SAME traversal the main-panel query applies
    /// (`descendant_within_stopping_at_pages`, the mirror of the compiled
    /// recursive CTE), not by a bare parent-chain walk. The panel stops
    /// descending at any NON-ROOT page and truncates at nesting depth
    /// [`MAIN_PANEL_MAX_DEPTH`], so a plain ancestor check reports blocks the
    /// panel legitimately does not render (`query::MAIN_PANEL_MAX_DEPTH`).
    /// That mismatch had two costs: the
    /// generator offered click targets no user could click (the driver then
    /// spins its poll deadline and dispatches nothing), and
    /// `inv-main-panel-rows-match-focus` demanded rows prod is right to omit.
    pub fn main_editable_descendants(&self) -> Vec<EntityUri> {
        let focus_roots = self.rendered_focus_root(Region::Main);
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
                    && self.main_panel_renders_within(id, &focus_roots)
            })
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Whether the main panel renders a row for `block_id` — the SAME traversal
    /// the compiled panel query applies. See
    /// [`RefBlockTree::main_panel_renders`] for why a bare ancestor walk is
    /// not a visibility predicate.
    pub fn main_panel_renders(&self, block_id: &EntityUri) -> bool {
        let focus_roots = self.rendered_focus_root(Region::Main);
        self.main_panel_renders_within(block_id, &focus_roots)
    }

    fn main_panel_renders_within(
        &self,
        block_id: &EntityUri,
        focus_roots: &std::collections::BTreeSet<EntityUri>,
    ) -> bool {
        let journals = EntityUri::block("journals");
        // Journals renders through its `journal_feed` — the Page-tagged day
        // pages — not a descendant walk from journals itself. A non-page child
        // of journals is absent from the feed and renders nothing, so root the
        // page-stopping walk at the day pages instead of journals.
        if focus_roots.contains(&journals) {
            if block_id == &journals {
                return true;
            }
            let day_pages = self.journal_feed_day_pages();
            return crate::pbt::query::descendant_within_stopping_at_pages(
                &self.domain.block_state.blocks,
                block_id,
                &day_pages,
                crate::pbt::query::MAIN_PANEL_MAX_DEPTH,
            );
        }
        crate::pbt::query::descendant_within_stopping_at_pages(
            &self.domain.block_state.blocks,
            block_id,
            focus_roots,
            crate::pbt::query::MAIN_PANEL_MAX_DEPTH,
        )
    }

    /// The Page-tagged children of `block:journals` — the exact predicate
    /// `journal_feed` selects (mirrors [`QuerySource::JournalFeed`]).
    fn journal_feed_day_pages(&self) -> std::collections::BTreeSet<EntityUri> {
        let journals = EntityUri::block("journals");
        self.domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.is_page() && b.parent_id == journals)
            .map(|b| b.id.clone())
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
        self.domain.predicts_navigation_focus(uri, region)
    }

    /// Block IDs in the predicted LeftSidebar render set — the same set
    /// the default sidebar PRQL produces. Each entry is wrapped by the
    /// default layout in a selectable bound to `navigation.focus`, so
    /// this is also the candidate set for `ClickBlock(LeftSidebar)` and
    /// `NavigateFocus` generators.
    pub fn predicted_sidebar_navigation_targets(&self) -> Vec<EntityUri> {
        self.domain.predicted_sidebar_navigation_targets()
    }

    /// Get IDs of text blocks only (not source blocks).
    pub fn text_block_ids(&self) -> Vec<EntityUri> {
        self.domain.block_state.text_block_ids()
    }

    // ── Block hierarchy query helpers ──────────────────────────────────

    /// Children of parent sorted by sequence then ID (matching canonical
    /// ordering).
    pub fn sorted_children_of(&self, parent_id: &EntityUri) -> Vec<&Block> {
        self.domain.block_state.sorted_children_of(parent_id)
    }

    /// Predicted ordered child ids of `parent_id`. Mirrors what
    /// `BlockOrdering::children(parent_id)` should return on the live
    /// side. The encoding-free child-id list is the contract — both
    /// sides produce a `Vec<EntityUri>`, no `sort_key` / `sequence`
    /// strings cross the boundary.
    pub fn children_of(&self, parent_id: &EntityUri) -> Vec<EntityUri> {
        self.domain.block_state.children_of(parent_id)
    }

    /// Previous sibling of block_id (same parent, immediately before in
    /// sequence order).
    pub fn previous_sibling(&self, block_id: &EntityUri) -> Option<EntityUri> {
        self.domain.block_state.previous_sibling(block_id)
    }

    /// Next sibling of block_id (same parent, immediately after in sequence
    /// order).
    pub fn next_sibling(&self, block_id: &EntityUri) -> Option<EntityUri> {
        self.domain.block_state.next_sibling(block_id)
    }

    /// Grandparent of block_id (parent's parent). None if at root level.
    pub fn grandparent(&self, block_id: &EntityUri) -> Option<EntityUri> {
        self.domain.block_state.grandparent(block_id)
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

    /// Split a block at the given byte position, mirroring
    /// `traits.rs::split_block`.
    ///
    /// The id follows the TEXT. For `position > 0` the original keeps
    /// `content[..position].trim_end()` and a fresh synthetic id takes the tail
    /// in a new block below it. For `position == 0` the original keeps ALL the
    /// text — so backlinks, marks and `:ID:`-addressed references stay on the
    /// block the text is still in — and the fresh synthetic id goes to the
    /// EMPTY block inserted ABOVE.
    ///
    /// Returns the id of the LOWER block, which is always the split's focus
    /// target: the minted id for `position > 0`, the original at `position ==
    /// 0`.
    pub fn split_block(&mut self, block_id: &EntityUri, position: usize) -> EntityUri {
        use holon_orgmode::models::OrgBlockExt;

        let original = self.domain.block_state.blocks.get(block_id).unwrap();
        let content = original.content.clone();
        let origin_marks = original.marks.clone().unwrap_or_default();
        let parent_id = original.parent_id.clone();
        let original_seq = original.sequence();

        // Split content AND partition marks — model-first parity with prod's
        // `BlockOperations::split_block`, both routed through the ONE
        // `holon_api::split_content_marks` (link straddling → plain text on both
        // sides; formatting straddling → truncate; whitespace trims applied).
        // Before this the model mirrored prod's OLD bug (split content, leave
        // marks untouched) so a mark destroyed across a split diverged on
        // neither side — invisible to the keystone. Now both carry marks and a
        // regression that drops them goes RED.
        let holon_api::SplitContentMarks {
            left:
                holon_api::SplitSide {
                    content: content_before,
                    marks: left_marks,
                },
            right:
                holon_api::SplitSide {
                    content: content_after,
                    marks: right_marks,
                },
        } = holon_api::split_content_marks(&content, &origin_marks, position);

        // Identity follows the text: a position-0 split leaves the whole text on
        // `block_id` and gives the minted id the empty side, so the two sides
        // swap roles relative to a mid-text split.
        let at_start = position == 0;
        let (kept_content, kept_marks, minted_content, minted_marks) = if at_start {
            (content_after, right_marks, content_before, left_marks)
        } else {
            (content_before, left_marks, content_after, right_marks)
        };

        {
            let orig = self.domain.block_state.blocks.get_mut(block_id).unwrap();
            orig.content = kept_content;
            orig.marks = (!kept_marks.is_empty()).then_some(kept_marks);
        }

        // Create new block with synthetic ID
        let new_id = EntityUri::block(&format!(":split-{}", self.domain.block_state.next_id));
        let mut new_block = Block::new_text(new_id.clone(), parent_id.clone(), minted_content);
        new_block.marks = (!minted_marks.is_empty()).then_some(minted_marks);
        // Slot the new block directly ABOVE the original at a position-0 split
        // (it is the empty one) and directly BELOW it otherwise: shift every
        // sibling already at or after that slot one position down before
        // inserting, so the new block lands uniquely between its neighbours.
        //
        // Without the shift the new block ends up sharing `original_seq + 1`
        // with whatever sibling occupied that slot; `recanon_and_rebuild` then
        // tie-breaks by lexicographic id and routinely puts the new block
        // *past* that sibling instead of right after the original. Production's
        // `BlockOperations::split_block` uses fractional indices and always
        // lands the new block strictly between the two — mirror that ordering
        // here so chord-op chains (e.g. SplitBlock → MoveUp → Indent) compute
        // the same `previous_sibling`.
        let slot = if at_start {
            original_seq
        } else {
            original_seq + 1
        };
        for sibling in self.domain.block_state.blocks.values_mut() {
            if sibling.parent_id == parent_id && sibling.sequence() >= slot {
                let s = sibling.sequence();
                sibling.set_sequence(s + 1);
            }
        }
        new_block.set_sequence(slot);

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
        if at_start { block_id.clone() } else { new_id }
    }

    /// Re-mint `old_id` with a FRESH synthetic `block::split-N` id: move the
    /// block under the new key (content, parent, sequence all preserved) and
    /// re-parent every child (`parent_id == old_id` -> new id). Models the
    /// reference side of the R2 id-less-reconcile CHURN (see the
    /// `RefBlockTreeMut::remint_block` trait doc). No `recanon_and_rebuild`:
    /// positions are unchanged, and it would double-bump `next_id`.
    pub fn remint_block(&mut self, old_id: &EntityUri) -> EntityUri {
        let new_id = EntityUri::block(&format!(":split-{}", self.domain.block_state.next_id));
        self.domain.block_state.next_id += 1;
        let mut block = self
            .domain
            .block_state
            .blocks
            .remove(old_id)
            .unwrap_or_else(|| panic!("remint_block: block {old_id} absent from ref block_state"));
        block.id = new_id.clone();
        self.domain.block_state.blocks.insert(new_id.clone(), block);
        if let Some(doc) = self.domain.block_state.block_documents.remove(old_id) {
            self.domain
                .block_state
                .block_documents
                .insert(new_id.clone(), doc);
        }
        for child in self.domain.block_state.blocks.values_mut() {
            if child.parent_id == *old_id {
                child.parent_id = new_id.clone();
            }
        }
        new_id
    }

    /// Create a new text block under `parent` as its LAST child — the oracle
    /// prediction for the creation-slot "type here to create" gesture
    /// (`CreateBlockUnderFocus`). Prod's `block.create` from the
    /// `:__virtual:<parent>` slot appends the block at the end of the parent's
    /// children (the slot sorts last, then the created block takes a fresh
    /// fractional index at the tail); mirror that ordering with `max_seq + 1`
    /// so `recanon_and_rebuild`'s canonical pass lands it last. The synthetic
    /// `block::create-N` id pairs 1:1 with the SUT's minted uuid via the
    /// composed harness's per-tick reconcile. `recanon_and_rebuild` bumps
    /// `next_id` (same as `split_block`), so this does not increment it.
    pub fn create_block_under(&mut self, parent: &EntityUri, content: &str) -> EntityUri {
        let new_id = EntityUri::block(&format!(":create-{}", self.domain.block_state.next_id));
        self.create_block_under_with_id(parent, content, new_id.clone());
        new_id
    }

    /// The born-equal sibling of [`create_block_under`]: append a new text
    /// block under `parent` using exactly `new_id` (a fresh normal id
    /// supplied by the transition, NOT a minted `create-N` synthetic). The
    /// SUT's op-floor create dispatches the same id, so the reconcile
    /// treats it as born-equal (both sides already hold it — no
    /// synthetic→real pairing). Otherwise identical to
    /// [`create_block_under`] (tail append, document ownership, re-canon).
    pub fn create_block_under_with_id(
        &mut self,
        parent: &EntityUri,
        content: &str,
        new_id: EntityUri,
    ) {
        use holon_orgmode::models::OrgBlockExt;

        // Undo-stack correspondence: CreateBlockUnderFocus dispatches a
        // User-origin `block.create`, and the engine records an undo entry for
        // every User-origin reversible op (operation_engine.rs — genuine insert
        // journals a `delete` inverse). The ref MUST snapshot here to stay 1:1
        // with that journal; without it, a later UndoLastMutation pops the ref's
        // *previous* snapshot (e.g. a split) while the engine pops the create
        // inverse, undoing different ops and diverging. This is the sole choke
        // point for create-under-focus (`create_block_under` delegates here).
        //
        // No reversibility gate is needed while every caller supplies a FRESH
        // id (always a genuine insert → always journaled). If a duplicate-id
        // create is ever introduced, the engine IGNORES the insert and declares
        // it irreversible — that path would then need to skip this snapshot to
        // match, mirroring the join/slash-delete leaf gates.
        self.push_undo_snapshot();
        // The org lens applies to a block BORN from UI input exactly as it does
        // to one edited (`set_block_content`): the creation slot commits typed
        // text, so raw `[[Page]]` / `*bold*` markup arrives here and the store
        // adopts it into (label, marks) at the write boundary. Without this the
        // oracle carried raw markup with `marks = None` and no link case was
        // expressible as a hand-authored regression at all.
        let (content, marks) = super::types::normalize_content_for_org_roundtrip_with(
            content,
            ContentType::Text,
            &self.harness.link_classifier,
        );
        self.insert_block_under_no_snapshot(parent, &content, new_id.clone());
        if let Some(b) = self.domain.block_state.blocks.get_mut(&new_id) {
            b.marks = marks;
        }
        self.recanon_and_rebuild();
    }

    /// The creation-slot GESTURE's reference effect, which is not one op but
    /// two, because a creation affordance is not a block (ruling C, 2026-08-08,
    /// with sub-ruling B, 2026-08-09: "a creation slot becomes a real born
    /// block the moment it can receive input").
    ///
    /// Focus reaching the affordance births an EMPTY block under `parent` as an
    /// authority-only, non-user operation — `OpOrigin::Rule`, which by
    /// definition pushes no undo entry (ADR 0030 D1: empty content is a valid
    /// contract value, so the birth is total in one firing). The text the user
    /// then types is an ordinary undo-visible content write. So
    /// `UndoLastMutation` after this gesture reverts the TEXT and leaves
    /// the empty block standing — it is not the user's create to undo. The
    /// reaper collects such a block when focus leaves; the reference models
    /// no reaper, so the empty block simply persists in the prediction,
    /// which is the SUT's state before reaping.
    ///
    /// The born-equal arm ([`create_block_under_with_id`]) is NOT this: an
    /// explicit id means the caller dispatched `block.create` directly, with no
    /// affordance and no gesture, so it stays one user-origin create.
    pub fn birth_block_under_slot(&mut self, parent: &EntityUri, content: &str) -> EntityUri {
        let new_id = EntityUri::block(&format!(":create-{}", self.domain.block_state.next_id));

        // The birth: undo-INVISIBLE, so no snapshot. Canonicalize before the
        // snapshot so undo restores a canonical state.
        self.insert_block_under_no_snapshot(parent, "", new_id.clone());
        self.recanon_and_rebuild();

        // The user's first keystroke: one undo-visible content write, snapshotted
        // over a state that ALREADY contains the empty block. The second pass
        // must NOT mint: the gesture creates exactly one block, so exactly one
        // synthetic id is burned — a second `recanon_and_rebuild` here would
        // advance the allocator twice and shift every later `create-N` /
        // `split-N` id out from under the pinned cases.
        self.push_undo_snapshot();
        let (content, marks) = super::types::normalize_content_for_org_roundtrip_with(
            content,
            ContentType::Text,
            &self.harness.link_classifier,
        );
        let block = self
            .domain
            .block_state
            .blocks
            .get_mut(&new_id)
            .expect("birth_block_under_slot: the block just born must exist");
        block.content = content;
        block.marks = marks;
        self.recanon_without_minting();

        // The gesture MOVES the caret: focus reaching the affordance is what
        // births the block, and the birth seats focus + caret in it (offset 0).
        // The global in-memory focus mirror (ADR 0010) must follow, or
        // `inv-focus-matches-ref` compares the pre-gesture focus root against
        // the SUT's newborn.
        self.ui.tab.focused_block = Some(new_id.clone());
        // The focus authority moves to the newborn, so any editor open over
        // ANOTHER block is blurred — prod's on_blur commits its user-authored
        // pending text, exactly as at the other two authority-move sites
        // (`transitions::model_chord_click_focus`, `FocusEditableText`).
        commit_active_editor_if_dirty(self);
        // The editor must then be CLOSED, not left standing. The birth mounts
        // no ref editor over the newborn (see below), so a surviving
        // `active_editor` names a block prod is no longer editing, and every
        // reader of that field is then wrong about a live editor: it made
        // `model_chord_click_focus` skip a click prod performs, leaving the ref
        // caret a whole block-length behind the SUT
        // (docs/Testing/bugfunnel/entries/
        // 2026-09-02-slot-birth-leaves-a-stale-ref-editor-that-suppresses-the-chord-click.md).
        self.ui.tab.active_editor = None;
        // Mounting a ref editor over the NEWBORN is the remaining gap, and it
        // is deliberate: prod does seat a caret there, so `inv-editor-text/
        // mirror` and `inv-editor-caret/mirror` sit Unobservable (both sides
        // absent) across this gesture — which is what the close above restores.
        // Opening one here instead reds `inv-editor-text/mirror` (ref "a" vs
        // SUT MutableText "") because the driver commits the text as a
        // `set_field` and seeds the mirror from a cell that has not received
        // the write yet. Closing THAT needs the driver to TYPE through the
        // editor keystroke sink, which changes what the transition drives.
        new_id
    }

    /// [`recanon_and_rebuild`](Self::recanon_and_rebuild) without advancing the
    /// synthetic-id allocator. For the second pass of a multi-op gesture that
    /// mints only ONE block.
    fn recanon_without_minting(&mut self) {
        let mut blocks: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        crate::assign_reference_sequences_canonical(&mut blocks);
        self.domain.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        self.rebuild_profile_tracking();
    }

    /// Insert one text block under `parent` with `new_id`, WITHOUT pushing an
    /// undo snapshot and WITHOUT re-canonicalizing. The snapshot + recanon
    /// wrapper lives in the callers: `create_block_under_with_id` (one
    /// block = one undo boundary) and `apply_instantiate_template` (a
    /// multi-block composite that snapshots + recanons ONCE, so one undo
    /// removes the whole instantiation).
    fn insert_block_under_no_snapshot(
        &mut self,
        parent: &EntityUri,
        content: &str,
        new_id: EntityUri,
    ) {
        use holon_orgmode::models::OrgBlockExt;

        let mut new_block = Block::new_text(new_id.clone(), parent.clone(), content.to_string());
        let max_seq = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == *parent)
            .map(|b| b.sequence())
            .max();
        new_block.set_sequence(max_seq.map_or(0, |s| s + 1));

        // The new block belongs to the DOCUMENT that contains it, mirroring
        // `split_block`. When the focus-root `parent` is itself a page /
        // document root (its own `block_documents` entry is the sentinel
        // `no_parent` — a page IS its own doc), the child lives in that page's
        // document, i.e. the page id ITSELF — not the sentinel. Writing the
        // sentinel here would make `seed_block_ids` misclassify the created
        // block as a seed and `RefBackend::org_blocks` drop it from the org
        // projection (while the SQL projection keeps it), diverging `/org` only.
        // A regular-block parent contributes its own document unchanged.
        let doc_uri = match self.domain.block_state.block_documents.get(parent) {
            Some(doc) if doc.is_no_parent() || doc.is_sentinel() => parent.clone(),
            Some(doc) => doc.clone(),
            None => parent.clone(),
        };
        self.domain
            .block_state
            .block_documents
            .insert(new_id.clone(), doc_uri);
        self.domain
            .block_state
            .blocks
            .insert(new_id.clone(), new_block);
    }

    /// Reference effect for `InstantiateTemplate`: mint the instance subtree
    /// (root + child) under `target_parent` as ONE undoable unit, mirroring the
    /// SUT's composite-undo group so one `UndoLastMutation` removes every
    /// instance block. `inst_root_id`/`inst_child_id` are the production
    /// deterministic instance ids the caller computed — born-equal with
    /// `plan_instantiation`'s `(template_id, context_key, node.id)`, so no
    /// synthetic→real reconcile.
    pub fn apply_instantiate_template(
        &mut self,
        target_parent: &EntityUri,
        inst_root_id: EntityUri,
        inst_child_id: EntityUri,
        root_content: &str,
        child_content: &str,
        child_marks: Option<Vec<holon_api::MarkSpan>>,
        template_id: &str,
    ) {
        self.push_undo_snapshot();
        self.insert_block_under_no_snapshot(target_parent, root_content, inst_root_id.clone());
        self.insert_block_under_no_snapshot(&inst_root_id, child_content, inst_child_id.clone());
        // An instance inherits the definition node's marks, remapped across the
        // `{{var}}` substitution (`plan_instantiation` → `remap_marks`); a
        // freshly minted plain block would model an instantiation that silently
        // flattens the template's rich text.
        if let Some(b) = self.domain.block_state.blocks.get_mut(&inst_child_id) {
            b.marks = child_marks;
        }
        // The engine stamps the instance ROOT (only) with the persisted
        // `instance_of` provenance property (`template_instantiation.rs`); it
        // org-round-trips like any property, so the oracle carries it or
        // `inv-blocks-match-ref/*` diverges.
        if let Some(b) = self.domain.block_state.blocks.get_mut(&inst_root_id) {
            b.properties.insert(
                holon_api::INSTANCE_OF_PROPERTY.to_string(),
                holon_api::Value::String(template_id.to_string()),
            );
        }
        self.recanon_and_rebuild();
    }

    /// `BlockToPage` (Option B) reference effect — the ref-side mirror of the
    /// engine-level `convert_block_to_page` compound. Mints a NEW page block
    /// `page_id` (Page-tagged, its own document) as the last child of
    /// `destination_parent`, carrying the origin's ORIGINAL content and marks;
    /// re-homes the origin's direct children (and their non-page subtrees)
    /// under it; and rewrites the origin in place as a non-page whose marks
    /// become a single full-span `[[page_id]]` Link. The origin's own
    /// content, parent, tags and document are untouched — it stays a
    /// non-page under its old ancestor.
    ///
    /// The born-equal `page_id` (a `PageId::for_path` hash the caller computed
    /// from the SAME title path the backend planner uses) and the reparented
    /// children / origin-link marks make every field the block-comparison
    /// invariants read agree with the SUT with no synthetic→real reconcile.
    pub fn apply_block_to_page(
        &mut self,
        origin: &EntityUri,
        page_id: EntityUri,
        destination_parent: &EntityUri,
    ) {
        use holon_api::inline_mark::EntityRef;
        use holon_api::inline_mark::InlineMark;
        use holon_api::inline_mark::MarkSpan;
        use holon_orgmode::models::OrgBlockExt;

        // RECOGNITION refusal (resolve-before-mint, ADR 0029): if `page_id` is
        // ALREADY held by a DIFFERENT-titled entity — the state a `RenamePage`
        // leaves (title changed, id preserved) — production REFUSES the convert
        // fail-loud rather than clobber. Model the refusal: no page minted, no
        // re-home, no origin rewrite, and NO undo entry (checked BEFORE the
        // snapshot). Uses the SAME `recognize_derived_id` as the SUT seam in
        // `run_convert_block_to_page`; the SUT driver mirrors it by tolerating the
        // `IdentityCollision`. Free / same-title ids fall through and convert.
        let origin_content_for_recognition = self
            .domain
            .block_state
            .blocks
            .get(origin)
            .expect("apply_block_to_page: origin block must exist (precondition)")
            .content
            .clone();
        let holder_title = self
            .domain
            .block_state
            .blocks
            .get(&page_id)
            .map(|b| b.content.clone());
        // SINGLE-SOURCE the requested title through `sanitize_page_title` — the
        // SAME sanitize the SUT planner applied to `plan.origin_content` (the
        // title run_convert_block_to_page recognizes with). Recognizing the RAW
        // content here would DIVERGE from the SUT for a trailing-slash title
        // (normalize_for_hash keeps '/'). Fall back to raw only when sanitize
        // yields nothing (empty content — unreachable past the planner's guard).
        let requested_title = holon_api::sanitize_page_title(&origin_content_for_recognition)
            .unwrap_or_else(|| origin_content_for_recognition.clone());
        if let holon_api::Recognition::Collision(_) =
            holon_api::recognize_derived_id(&page_id, holder_title.as_deref(), &requested_title)
        {
            return;
        }

        // Undo-stack correspondence: the compound records ONE User-origin undo
        // entry, so the ref snapshots exactly once here (mirrors
        // `create_block_under_with_id`).
        self.push_undo_snapshot();

        let origin_block = self
            .domain
            .block_state
            .blocks
            .get(origin)
            .expect("apply_block_to_page: origin block must exist (precondition)")
            .clone();
        let origin_content = origin_block.content.clone();
        let origin_marks = origin_block.marks.clone();

        // Origin's DIRECT children in sort order, captured before mutating.
        let children: Vec<EntityUri> = self
            .sorted_children_of(origin)
            .into_iter()
            .map(|b| b.id.clone())
            .collect();

        // 1. Mint page P as the last child of `destination_parent`. The page TITLE
        //    mirrors the backend sanitize (trailing `/` stripped; land 866977e85e) so
        //    P's content + id + filename agree — the origin block and its `[[P]]` link
        //    label below keep the RAW content (the backend leaves the origin text
        //    unchanged). Marks carry over from the origin.
        let page_content =
            crate::pbt::transitions::block_to_page::sanitize_page_leaf(&origin_content)
                .unwrap_or_else(|| origin_content.clone());
        let mut page = Block::new_text(page_id.clone(), destination_parent.clone(), page_content);
        page.set_page(true);
        page.marks = origin_marks;
        let max_seq = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == *destination_parent)
            .map(|b| b.sequence())
            .max();
        page.set_sequence(max_seq.map_or(0, |s| s + 1));
        self.domain
            .block_state
            .block_documents
            .insert(page_id.clone(), page_id.clone());
        self.domain.block_state.blocks.insert(page_id.clone(), page);

        // 2. Re-home the origin's direct children under P, preserving order.
        for (i, child) in children.iter().enumerate() {
            let b = self
                .domain
                .block_state
                .blocks
                .get_mut(child)
                .expect("apply_block_to_page: origin child must exist");
            b.parent_id = page_id.clone();
            b.set_sequence(i as i64);
        }

        // 3. Re-home the document of every non-page block in the moved subtrees to P
        //    (they now live in P's file). A nested page owns its own file and
        //    terminates the walk — its subtree stays homed to it.
        let mut stack: Vec<EntityUri> = children.clone();
        while let Some(id) = stack.pop() {
            let is_page = self
                .domain
                .block_state
                .blocks
                .get(&id)
                .is_some_and(|b| b.is_page());
            if is_page {
                continue;
            }
            self.domain
                .block_state
                .block_documents
                .insert(id.clone(), page_id.clone());
            let grandchildren: Vec<EntityUri> = self
                .sorted_children_of(&id)
                .into_iter()
                .map(|b| b.id.clone())
                .collect();
            stack.extend(grandchildren);
        }

        // 4. Leave a full-span `[[P]]` link on the origin — content unchanged, marks
        //    replaced by exactly the one Link the backend's `set_field` writes (label =
        //    origin content).
        let link_mark = MarkSpan::new(
            0,
            origin_content.chars().count(),
            InlineMark::Link {
                target: EntityRef::from_uri(&page_id.clone()),
                label: origin_content.clone(),
            },
        );
        self.domain
            .block_state
            .blocks
            .get_mut(origin)
            .expect("apply_block_to_page: origin block must exist")
            .marks = Some(vec![link_mark]);

        self.recanon_and_rebuild();
    }

    // ── Page identity (PageIdentityDeterminism.md 5.3) ────────────────────
    //
    // Page ids are `blake3(normalized path)`. A rename is an ordinary edit to
    // the existing entity -- the id does NOT re-mint -- which means a rename
    // FREES a path while leaving its blake3 id occupied. The two methods below
    // are the reference halves of the transitions that reach that state.

    /// The `/`-joined page path (root->leaf) of an existing page block.
    ///
    /// Mirrors the backend planner's `page_path_of` (and the twin in
    /// `transitions::block_to_page`) so the ref hashes the SAME string the
    /// writer does. `None` when `id` is not a page, or any page in its chain
    /// has empty content (the backend would then produce an empty path segment
    /// and `PageId::for_path` would reject it).
    pub fn page_path_of_ref(&self, id: &EntityUri) -> Option<String> {
        let mut segments: Vec<String> = Vec::new();
        let mut seen: BTreeSet<EntityUri> = BTreeSet::new();
        let mut cursor = Some(id.clone());
        while let Some(cur) = cursor {
            if !seen.insert(cur.clone()) {
                break;
            }
            let block = self.domain.block_state.blocks.get(&cur)?;
            if !block.is_page() {
                break;
            }
            let title = block.content.trim().to_string();
            if title.is_empty() {
                return None;
            }
            segments.push(title);
            cursor = Some(block.parent_id.clone())
                .filter(|p| self.domain.block_state.blocks.contains_key(p));
        }
        if segments.is_empty() {
            return None;
        }
        segments.reverse();
        Some(segments.join("/"))
    }

    /// Reference mirror of `SqlOperationProvider::resolve_page_name` -- the
    /// title-only lookup `create_page_from_link` uses to decide whether a
    /// segment already exists.
    ///
    /// The production query matches Page-tagged blocks on `content = leaf`
    /// GLOBALLY (no parent constraint), orders by "parent's content equals the
    /// hint's second-to-last segment" first, then by id ascending, and takes
    /// the first row. Mirroring the ordering exactly is what lets the reference
    /// predict WHICH segments the op reuses and which it mints.
    pub fn ref_resolve_page_name(&self, hint: &str) -> Option<EntityUri> {
        let mut segs = hint.rsplit('/');
        let leaf = segs.next().unwrap_or(hint).trim();
        if leaf.is_empty() {
            return None;
        }
        let parent_hint = segs.next().map(|s| s.trim().to_string());
        let mut hits: Vec<(u8, EntityUri)> = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.is_page() && b.content == leaf)
            .map(|b| {
                let parent_matches = parent_hint.as_deref().is_some_and(|h| {
                    self.domain
                        .block_state
                        .blocks
                        .get(&b.parent_id)
                        .is_some_and(|p| p.content == h)
                });
                (u8::from(!parent_matches), b.id.clone())
            })
            .collect();
        hits.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.as_str().cmp(b.1.as_str())));
        hits.into_iter().next().map(|(_, id)| id)
    }

    /// Page paths a rename has vacated and that NO page currently occupies.
    ///
    /// A path leaves the pool the moment some page re-occupies it (the
    /// `CreatePageAtFreedPath` this ledger feeds, or a rename back), so the
    /// generator never offers a name that is already taken.
    pub fn freed_page_paths_ref(&self) -> Vec<String> {
        let occupied: BTreeSet<String> = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.is_page())
            .filter_map(|b| self.page_path_of_ref(&b.id))
            .collect();
        let mut out: Vec<String> = self
            .renamed_away_page_paths
            .iter()
            .filter(|p| !occupied.contains(*p))
            .cloned()
            .collect();
        out.sort();
        out.dedup();
        out
    }

    /// `RenamePage` reference effect -- 5.3: "a rename is an ordinary edit to
    /// the existing entity", so ONLY the content changes. The id, parent, tags,
    /// document and children are untouched, and the page's OLD path enters the
    /// freed-path ledger.
    pub fn apply_page_rename(&mut self, page_id: &EntityUri, new_title: &str) {
        // One User-origin `set_field("content")` => one undo entry, exactly like
        // the content-mutation path.
        self.push_undo_snapshot();
        let old_path = self.page_path_of_ref(page_id).unwrap_or_else(|| {
            panic!("apply_page_rename: {page_id} must be a page with a well-formed path")
        });
        let block = self
            .domain
            .block_state
            .blocks
            .get_mut(page_id)
            .expect("apply_page_rename: page block must exist (precondition)");
        block.content = new_title.to_string();
        self.renamed_away_page_paths.push(old_path);
        self.recanon_and_rebuild();
    }

    /// `CreatePageAtFreedPath` reference effect -- the ref-side mirror of the
    /// production `block.create_page_from_link(target)` op.
    ///
    /// Walks `path` segment by segment exactly as the op does: resolve the
    /// accumulated hint through [`Self::ref_resolve_page_name`]; on a hit reuse
    /// that page as the parent, on a miss mint a page titled by the segment
    /// under the current parent.
    ///
    /// The minted id is where the ORACLE ENCODES THE SPEC. When
    /// `PageId::for_path(seg_path)` is FREE the page is minted there. When it
    /// is already occupied -- the state a `RenamePage` leaves behind (title
    /// changed, id preserved) -- the INTERIM identity policy (plan §5) has
    /// production REFUSE the `create` FAIL LOUD rather than let its
    /// `ON CONFLICT(id) DO UPDATE` clobber the renamed page. So the reference
    /// models the refusal: no new page, no undo entry, no state change. The SUT
    /// driver mirrors it by tolerating the `IdentityCollision`.
    ///
    /// END-STATE (plan §5, ruled 2026-07-26): a recreate at a freed path will
    /// mint a DISTINCT id and bind the NAME to it. When that lands, replace the
    /// refusal below with the unique-mint and single-source the rule with the
    /// writer the way `BlockToPage` single-sources `PageId::for_page_under`, so
    /// oracle and writer stay born-equal.
    pub fn apply_create_page_at_path(&mut self, path: &str) {
        use holon_api::link_parser::PageId;
        use holon_orgmode::models::OrgBlockExt;

        // `create_page_from_link` returns `declared_irreversible` -- it records
        // NO undo entry -- so the reference must not push one either.
        let mut parent = EntityUri::no_parent();
        let mut accumulated = String::new();
        for (i, seg) in path.split('/').enumerate() {
            let trimmed = seg.trim();
            assert!(
                !trimmed.is_empty(),
                "apply_create_page_at_path: empty segment in {path:?} (generator must gate this \
                 out -- the op errors on it)"
            );
            let hint = if i == 0 {
                trimmed.to_string()
            } else {
                format!("{accumulated}/{trimmed}")
            };
            let seg_path = if accumulated.is_empty() {
                trimmed.to_string()
            } else {
                format!("{accumulated}/{trimmed}")
            };
            match self.ref_resolve_page_name(&hint) {
                Some(existing) => parent = existing,
                None => {
                    let id = PageId::for_path(&seg_path)
                        .unwrap_or_else(|e| {
                            panic!("apply_create_page_at_path: PageId::for_path({seg_path:?}): {e}")
                        })
                        .into_entity_uri();
                    // INTERIM identity policy (plan §5): a derived id already held
                    // by a DIFFERENT entity — exactly the state a `RenamePage`
                    // leaves (title changed, id preserved) — makes production's
                    // `create` FAIL LOUD. The op is REFUSED: nothing created,
                    // nothing clobbered, no undo entry. Model that refusal (no
                    // state change) and stop; the SUT driver mirrors it by
                    // tolerating the `IdentityCollision`. (Only the leaf is ever
                    // minted here — the generator gates every strict prefix to
                    // resolve — so a refused leaf refuses the whole op.)
                    //
                    // END-STATE (plan §5, ruled 2026-07-26): a recreate at a
                    // freed path mints a DISTINCT id and binds the NAME to it.
                    // When that lands, replace this early return with the
                    // unique-mint and single-source the rule with the writer.
                    if self.domain.block_state.blocks.contains_key(&id) {
                        return;
                    }
                    let mut page = Block::new_text(id.clone(), parent.clone(), trimmed.to_string());
                    page.set_page(true);
                    let max_seq = self
                        .domain
                        .block_state
                        .blocks
                        .values()
                        .filter(|b| b.parent_id == parent)
                        .map(|b| b.sequence())
                        .max();
                    page.set_sequence(max_seq.map_or(0, |s| s + 1));
                    self.domain
                        .block_state
                        .block_documents
                        .insert(id.clone(), id.clone());
                    self.domain.block_state.blocks.insert(id.clone(), page);
                    parent = id;
                }
            }
            accumulated = seg_path;
        }
        self.recanon_and_rebuild();
    }

    /// Join `block_id` into its merge target.
    ///
    /// Two cases, both triggered by Backspace at position 0:
    ///   1. **Previous sibling exists** (target = the block ABOVE in the
    ///      visible outline — see `join_merge_target`; the previous sibling's
    ///      deepest last visible descendant, which is the sibling itself only
    ///      when it is collapsed or childless):
    ///      - target.content = target.content + block.content
    ///      - re-parent block's children to target, appended after target's
    ///        existing children
    ///      - delete block
    ///   2. **No previous sibling, parent is text** (target = parent;
    ///      child→parent join):
    ///      - parent.content = parent.content + block.content
    ///      - re-parent block's children to parent, placed at block's old slot
    ///        (before block's old siblings)
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
            Some(_) => holon_pbt_core::capabilities::join_merge_target(block_id, self)
                .expect("join_block: a previous sibling yields a merge target"),
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

    /// Apply a mutation to the block state, re-canonicalize, and rebuild
    /// profiles.
    pub fn apply_mutation(&mut self, event: &holon_pbt_core::types::MutationEvent) {
        let mut blocks: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        event.mutation.apply_to(&mut blocks);
        self.domain.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        self.recanon_and_rebuild();
    }

    /// Pad the shadow primary to the latest fed SUT Lamport height, then
    /// mirror the ref block map into it (membership/parent/content diff).
    /// No-op until the mesh exists (first `AddPeer`). Runs after EVERY ref
    /// transition (the `declare_e2e_transitions!` post-dispatch hook) so a
    /// primary edit inside a peer-concurrency window lands in the shadow at
    /// the same Lamport the SUT's edit lands at — which is what makes the
    /// shadow's concurrent-merge interleaving prediction exact (walking
    /// skeleton #2, `shadow_mesh_predicts_concurrent_primary_peer_merge`).
    pub fn shadow_catch_up_primary(&self) {
        self.loro
            .shadow_catch_up_primary(&self.domain.block_state.blocks);
    }

    /// Re-canonicalize sequences and rebuild profile tracking.
    pub fn recanon_and_rebuild(&mut self) {
        let mut blocks: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        crate::assign_reference_sequences_canonical(&mut blocks);
        self.domain.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        self.rebuild_profile_tracking();
        self.domain.block_state.next_id += 1;
    }

    /// Returns the set of block IDs that should appear in `focus_roots` for a
    /// region. Mirrors `schema/matview_focus_roots.sql`: a flat projection
    /// of `navigation_history WHERE closed_at IS NULL`, excluding home rows
    /// (block_id NULL — they don't JOIN against `root.id` in the consumer GQL).
    ///
    /// This is the region's OPEN SET, not what the panel shows. `PinBlock`
    /// grows it for Region::RightSidebar and `navigation.open_tab` grows it
    /// for Region::Main (background tabs); even at boot it is not a singleton.
    /// For what the main panel actually RENDERS, use
    /// [`Self::rendered_focus_root`]. Consumers use CHILD_OF*0..N to expand to
    /// root + descendants.
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

    /// The region's RENDERED focus root — what the main panel actually shows,
    /// as opposed to [`Self::expected_focus_root_ids`]'s open SET.
    ///
    /// Prod's main-panel query ends
    /// `JOIN navigation_cursor nc ON nc.region = fr.region AND nc.history_id =
    /// fr.history_id` (`assets/default/index.org`,
    /// `default-main-panel::src::0`), so exactly one open row projects: the
    /// cursor's. An open row that is not the cursor's is a BACKGROUND tab —
    /// present in `focus_roots`, absent from the panel. A cursor sitting on a
    /// row that is no longer open projects nothing at all (the blank-panel
    /// mode `navigate_back_keeps_panel_populated` locks down).
    ///
    /// Returned as a set because every consumer expands it through
    /// `is_descendant_of_any` / `rendered_block_ids`, which take a root set.
    pub fn rendered_focus_root(&self, region: Region) -> BTreeSet<EntityUri> {
        let open = self.expected_focus_root_ids(region);
        match self.current_focus(region) {
            Some(cursor) if open.contains(&cursor) => BTreeSet::from([cursor]),
            _ => BTreeSet::new(),
        }
    }

    /// Check if `block_id` is a descendant of any block in `roots` (or is
    /// itself in `roots`).
    pub fn is_descendant_of_any(
        &self,
        block_id: &EntityUri,
        roots: &std::collections::BTreeSet<EntityUri>,
    ) -> bool {
        self.domain
            .block_state
            .is_descendant_of_any(block_id, roots)
    }

    pub fn has_blocks_profile(&self) -> bool {
        self.domain.has_blocks_profile()
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
    /// Composition-root method: clones `domain.block_state` onto the
    /// `action.undo_stack`, crossing the action↔domain fragment boundary, so it
    /// must live on `ReferenceState` (not on either fragment alone). Enabling
    /// this (was a no-op while the engine returned
    /// `OperationResult::irreversible()` for every operation) activates the
    /// keystone undo rung: the 8 mutating transitions that call it now
    /// record snapshots, and `UndoLastMutation`
    /// (gated on `has_undo_history`) can pop them back.
    pub fn push_undo_snapshot(&mut self) {
        self.action.undo_stack.push(self.domain.block_state.clone());
        self.action.redo_stack.clear();
    }

    /// Undo: snapshot current state onto redo stack, restore from undo stack.
    ///
    /// Id-minting is MONOTONIC across undo/redo: prod never reuses a burned
    /// block id, so restoring an earlier tree must not roll back the synthetic
    /// id-mint high-water mark (`block_state.next_id`, source of `split-N` /
    /// `create-N`). Rolling it back re-mints an id the insert-only harness
    /// resolver still maps to a now-deleted real block, tripping the
    /// `per-tick reconcile: one synthetic per minted real id` desync
    /// (SplitBlock → UndoLastMutation → SplitBlock). This mirrors
    /// `next_doc_id`, which already lives outside the snapshotted
    /// `block_state` for the same reason.
    pub fn pop_undo_to_redo(&mut self) {
        let pre_restore = self.domain.block_state.clone();
        self.action.redo_stack.push(pre_restore.clone());
        let id_hwm = self.domain.block_state.next_id;
        let mut restored = self.action.undo_stack.pop().expect("undo stack is empty");
        restored.next_id = restored.next_id.max(id_hwm);
        self.domain.block_state = restored;
        self.rematerialize_file_ingested(&pre_restore);
        self.recompute_derived();
        self.clear_focus_for_blocks_dropped_by_restore(&pre_restore);
    }

    /// Redo: snapshot current state onto undo stack, restore from redo stack.
    /// Preserves the monotonic id-mint high-water mark — see
    /// [`Self::pop_undo_to_redo`].
    pub fn pop_redo_to_undo(&mut self) {
        let pre_restore = self.domain.block_state.clone();
        self.action.undo_stack.push(pre_restore.clone());
        let id_hwm = self.domain.block_state.next_id;
        let mut restored = self.action.redo_stack.pop().expect("redo stack is empty");
        restored.next_id = restored.next_id.max(id_hwm);
        self.domain.block_state = restored;
        // REDO-direction hazard: `pre_restore` still holds a block this redo is
        // supposed to re-delete, so re-materialising blindly from it would
        // RESURRECT a user-deleted block (spurious `only_in_ref`). What keeps it
        // sound is that a removing mutation un-marks the id
        // (`apply_content_mutation`), so a user-deleted block is no longer
        // file-backed and never enters the loop.
        self.rematerialize_file_ingested(&pre_restore);
        self.recompute_derived();
        self.clear_focus_for_blocks_dropped_by_restore(&pre_restore);
    }

    /// A block a snapshot restore dropped is gone from the tree, so
    /// [`Self::clear_focus_if_deleted`]'s contract applies to it: no focus and
    /// no editor survives it.
    ///
    /// Runs AFTER `rematerialize_file_ingested`, which puts file-ingested
    /// blocks back — those were never dropped.
    fn clear_focus_for_blocks_dropped_by_restore(&mut self, pre_restore: &BlockState) {
        let dropped: Vec<EntityUri> = pre_restore
            .blocks
            .keys()
            .filter(|id| !self.domain.block_state.blocks.contains_key(*id))
            .cloned()
            .collect();
        for id in &dropped {
            self.clear_focus_if_deleted(id);
        }
    }

    /// Prod's `engine.undo()` reverts only USER-origin ops
    /// (`operation_engine.rs:1220` — "only User-origin operations push undo
    /// entries"); an INGEST-origin file-ingested doc page (minted by the
    /// `FileSyncController` watcher for a `CreateDocument`) is NOT on the undo
    /// stack, so a doc created before an undo PERSISTS in prod. The oracle,
    /// however, snapshots the WHOLE `block_state`, so restoring a pre-doc
    /// snapshot would drop the doc page the SUT still holds — surfacing it as a
    /// phantom (`inv-viewmodel-entity-ids-subset-of-data` /
    /// `inv-blocks-match-ref` spurious). `files.documents` lives OUTSIDE the
    /// snapshot (like `next_doc_id`, see `pop_undo_to_redo`), so it is the
    /// authority for which docs exist; re-materialise every doc page the
    /// restore dropped, mirroring prod.
    ///
    /// The same argument covers the file's CONTENT blocks, not just its root:
    /// what the watcher parses out of an org file is INGEST-origin too, so
    /// `WriteOrgFile`/`BulkExternalAdd` blocks also survive prod's undo. They
    /// are tracked in `files.ingest_origin_blocks` (also outside the
    /// snapshot) and restored VERBATIM from `pre_restore` — the state just
    /// before this undo — because that is exactly what prod's undo leaves
    /// untouched. A block a user genuinely deleted is absent from
    /// `pre_restore` and correctly stays gone, and one edited by a later
    /// user op was re-snapshotted by that op, so it is in `restored`
    /// already and never reaches this path.
    ///
    /// A doc root that is in `files.documents` but in NEITHER state is rebuilt
    /// from the filename (title = file stem, `Page`) — the `insert_document`
    /// shape, for docs whose block was dropped before this undo.
    fn rematerialize_file_ingested(&mut self, pre_restore: &BlockState) {
        for id in &self.files.ingest_origin_blocks {
            if self.domain.block_state.blocks.contains_key(id) {
                continue;
            }
            let Some(block) = pre_restore.blocks.get(id) else {
                continue;
            };
            self.domain
                .block_state
                .blocks
                .insert(id.clone(), block.clone());
            if let Some(doc) = pre_restore.block_documents.get(id) {
                self.domain
                    .block_state
                    .block_documents
                    .insert(id.clone(), doc.clone());
            }
        }

        let docs: Vec<(EntityUri, String)> = self
            .files
            .documents
            .iter()
            .map(|(uri, name)| (uri.clone(), name.clone()))
            .collect();
        for (doc_uri, file_name) in docs {
            if self.domain.block_state.blocks.contains_key(&doc_uri) {
                continue;
            }
            let path = std::path::Path::new(&file_name);
            let doc_name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&file_name)
                .to_string();
            // A file in a SUBDIRECTORY (e.g. `Journals/2026-01-16.org`) is a page
            // NESTED under its folder-page, not a top-level doc-root — the
            // name-chain nesting the original create used. The only such folder
            // today is the `journals` companion (the auto-create rule emits
            // `place: page(journals)`), whose seed id is `block:journals`. Preserve
            // that parent on undo re-materialisation so the rule-created journal
            // day-block returns under `block:journals` (where the SUT keeps it),
            // not as a phantom top-level doc-root. Non-subdir docs stay `no_parent`.
            let is_journal_subdir = path
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|s| s.to_str())
                .is_some_and(|dir| dir == "Journals");
            let parent_uri = if is_journal_subdir {
                EntityUri::block("journals")
            } else {
                EntityUri::no_parent()
            };
            let mut doc_block = Block::new_text(doc_uri.clone(), parent_uri, doc_name);
            doc_block.set_page(true);
            // Match the SUT's created-last sibling order under `block:journals`
            // (see `RefClockMut::advance_day`): a re-materialised journal appends
            // after every current sibling.
            if is_journal_subdir {
                use holon_orgmode::models::OrgBlockExt;
                let journals = EntityUri::block("journals");
                let next_seq = self
                    .domain
                    .block_state
                    .blocks
                    .values()
                    .filter(|b| b.parent_id == journals)
                    .map(|b| b.sequence())
                    .max()
                    .unwrap_or(0)
                    + 1;
                doc_block.set_sequence(next_seq);
            }
            self.domain
                .block_state
                .blocks
                .insert(doc_uri.clone(), doc_block);
            self.domain
                .block_state
                .block_documents
                .insert(doc_uri.clone(), doc_uri);
        }
    }

    /// Recompute derived fields (profiles, render expressions) after undo/redo
    /// restore.
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
        self.domain.main_panel_block_id()
    }

    /// Get the main panel's render expression (the render source child of the
    /// main panel headline).
    pub fn main_panel_render_expr(&self) -> Option<&RenderExpr> {
        self.domain.main_panel_render_expr()
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

/// Convert a Block to a DataRow (HashMap<String, Value>) for ViewModel
/// construction.
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
    // document_id removed from Block struct; looked up via block_documents map if
    // needed
    if let Some(Value::String(ts)) = block.properties.get("task_state") {
        row.insert("task_state".into(), Value::String(ts.clone()));
    }
    if let Some(sl) = &block.source_language {
        row.insert("source_language".into(), Value::String(sl.to_string()));
    }
    row.insert("widget_only".into(), Value::Boolean(block.widget_only));
    row
}

impl holon_frontend::reactive::BuilderServices for ReferenceState {
    /// ReferenceState is a test double with no backend to await: it dispatches
    /// and reports that nothing was proven, rather than inheriting a claim
    /// it cannot make.
    fn dispatch_intent_awaitable(
        &self,
        intent: holon_frontend::operations::OperationIntent,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = anyhow::Result<holon_core::Delivery>> + Send + 'static,
        >,
    > {
        self.dispatch_intent(intent);
        Box::pin(std::future::ready(Ok(holon_core::Delivery::Unproven {
            detail: "ReferenceState: test double, no delivery to prove".to_string(),
        })))
    }

    fn interpret(
        &self,
        expr: &RenderExpr,
        ctx: &holon_frontend::RenderContext,
    ) -> holon_frontend::ReactiveViewModel {
        self.harness.interpreter.interpret(expr, ctx, self)
    }

    /// The reference model is the proptest state machine's owned, mutating
    /// state — a handle would have to be a COPY, and a lazy slot materialising
    /// against a copy would render a snapshot the model has already moved past,
    /// silently diverging from the SUT it exists to judge.
    fn clone_arc(&self) -> Arc<dyn holon_frontend::reactive::BuilderServices> {
        unimplemented!(
            "ReferenceState::clone_arc — the ref model cannot hand out a \
             handle; a lazy widget reached the reference render path"
        )
    }

    fn link_classifier(&self) -> &holon_api::link_parser::LinkTargetClassifier {
        &self.harness.link_classifier
    }

    fn get_block_data(
        &self,
        id: &EntityUri,
    ) -> (RenderExpr, Vec<Arc<holon_api::widget_spec::DataRow>>) {
        // Find render source child of this block in layout_blocks.
        // Two distinct "no expr" cases, previously conflated by a silent
        // `table()` fallback:
        // - a render-source child IS tracked but `render_expressions` has no entry →
        //   ref bookkeeping inconsistency, fail loud;
        // - no render-source child at all → the ref genuinely doesn't track a render
        //   for this block; fall back to `table()` like the prod stub services, but
        //   DISCLOSED (warn) so a vacuous comparison is attributable in the log.
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
                        "[ref get_block_data] block {id} has a tracked render-source child {rid} \
                         but no compiled entry in render_expressions — reference-model \
                         bookkeeping inconsistency"
                    )
                })
                .clone(),
            None => {
                tracing::warn!(
                    "[ref get_block_data] no render-source child tracked for {id}; falling back \
                     to table() (untracked-by-reference render)"
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
        let engine = self.profile_engine();
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
        // The reference model must never panic on a prod render path. An empty
        // stream is the faithful mirror: the keystone oracle asserts on
        // inv-blocks-match-ref, not on the watch stream itself.
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
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
mod main_panel_visibility_tests {
    use holon_orgmode::models::OrgBlockExt;

    use super::*;
    use crate::pbt::composed::wide_e2e::wide_e2e_ref;

    /// F5 `state-toggle-row-absent`: the main panel stops descending at a
    /// NON-ROOT page, so a block behind one renders no row and is neither a
    /// legal click target nor a row any invariant may demand. A bare ancestor
    /// walk says the opposite — the two must not be confused.
    ///
    /// Locked as a unit test, not a keystone JSONL case: the fix is a
    /// PRECONDITION / candidate-set narrowing, and the hand-authored runner
    /// replays its sequence verbatim without evaluating preconditions, so a
    /// composed case naming the unrenderable target reproduces the signature
    /// but can never go green. Disclosed deviation, same shape as the
    /// `org-render-echo-loop` lock.
    #[test]
    fn main_panel_visibility_stops_at_a_non_root_page() {
        let mut state = wide_e2e_ref();
        let roots = state.rendered_focus_root(Region::Main);
        let root = roots
            .iter()
            .next()
            .cloned()
            .expect("the default layout gives Main a focus root");

        let page = EntityUri::block("vis-mid-page");
        let leaf = EntityUri::block("vis-leaf");
        let mut page_block = Block::new_text(page.clone(), root.clone(), "a nested page");
        page_block.set_page(true);
        state
            .domain
            .block_state
            .blocks
            .insert(page.clone(), page_block);
        state.domain.block_state.blocks.insert(
            leaf.clone(),
            Block::new_text(leaf.clone(), page.clone(), "leaf"),
        );

        assert!(
            state.is_descendant_of_any(&leaf, &roots),
            "precondition of the test: the bare ancestor walk DOES reach the leaf"
        );
        assert!(
            !state.main_panel_renders(&leaf),
            "a block behind a non-root page renders no main-panel row, so it must not be \
             reported as visible"
        );
        assert!(
            !state.main_editable_descendants().contains(&leaf),
            "the candidate set must not offer a click target the panel does not paint"
        );
    }
}

#[cfg(test)]
mod ingest_origin_undo_tests {
    use holon_pbt_core::capabilities::RefApplyMutationMut;
    use holon_pbt_core::capabilities::RefDocumentsMut;
    use holon_pbt_core::types::Mutation;

    use super::*;
    use crate::pbt::composed::wide_e2e::wide_e2e_ref;

    fn ingest_one_block(state: &mut ReferenceState, id: &EntityUri) {
        let placeholder =
            EntityUri::block(crate::pbt::transitions::write_org_file::GEN_PLACEHOLDER);
        let block = Block::new_text(id.clone(), placeholder, "external alpha");
        state.seed_org_file("ingest_doc.org", std::slice::from_ref(&block), None);
    }

    /// The F2 fix itself, at the ref model alone: a file-ingested block is
    /// INGEST-origin, so an undo that restores a pre-file snapshot must NOT
    /// drop it — prod's undo stack never held it.
    #[test]
    fn undo_keeps_file_ingested_block_the_snapshot_predates() {
        let mut state = wide_e2e_ref();
        let ext = EntityUri::block("ext-a");

        // A User-origin op snapshots the pre-file state, THEN the file arrives.
        state.push_undo_snapshot();
        ingest_one_block(&mut state, &ext);
        assert!(state.domain.block_state.blocks.contains_key(&ext));

        state.pop_undo_to_redo();

        assert!(
            state.domain.block_state.blocks.contains_key(&ext),
            "undo over-reverted a file-ingested block: prod's undo stack holds only \
             User-origin ops, so the ingest survives `engine.undo()`"
        );
    }

    /// The REDO-direction twin, and the reason a removing mutation must un-mark
    /// its ids. Un-constructable as a composed keystone case: deleting a block
    /// that lives on disk trips the ADR-0025 write-back removal guard
    /// (`quarantine_writeback`,
    /// `crates/holon-filesystem/src/file_sync_controller.rs`),
    /// whose `tracing::error!` reds `inv-no-errors` first — the case would go
    /// red for the quarantine's reason, not this one. So the lock is here.
    #[test]
    fn redo_does_not_resurrect_a_user_deleted_file_ingested_block() {
        let mut state = wide_e2e_ref();
        let ext = EntityUri::block("ext-a");

        ingest_one_block(&mut state, &ext);
        assert!(state.domain.block_state.blocks.contains_key(&ext));

        // A User-origin delete of that ingested block: snapshot, then remove.
        state.push_undo_snapshot();
        state.apply_content_mutation(&Mutation::Delete { id: ext.clone() }, false);
        assert!(!state.domain.block_state.blocks.contains_key(&ext));

        state.pop_undo_to_redo();
        assert!(
            state.domain.block_state.blocks.contains_key(&ext),
            "undo of the delete must bring the block back"
        );

        state.pop_redo_to_undo();
        assert!(
            !state.domain.block_state.blocks.contains_key(&ext),
            "redo RESURRECTED a user-deleted file-ingested block — `rematerialize_file_ingested` \
             re-added it from the pre-redo state because the delete never un-marked it as \
             file-backed"
        );
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
            native_homed: Default::default(),
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
