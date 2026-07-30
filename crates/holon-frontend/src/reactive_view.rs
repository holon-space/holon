//! Self-managing reactive view — unified replacement for ReactiveCollection +
//! external wiring.
//!
//! `ReactiveView` owns its streaming pipeline and lifecycle. Collection drivers
//! are spawned internally via `start()` and cleaned up on `Drop`.
//!
//! ```text
//! ReactiveRenderedRows → ReactiveView (owns driver) → MutableVec<Arc<ReactiveViewModel>>
//!                                                       ↓
//!                                               Frontend shell subscribes
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use futures::future::AbortHandle;
use futures_signals::signal::Mutable;
use futures_signals::signal::SignalExt;
use futures_signals::signal_vec::MutableVec;
use futures_signals::signal_vec::SignalVecExt;
use futures_signals::signal_vec::VecDiff;
use holon_api::EntityUri;
use holon_api::ReactiveRowProvider;
use holon_api::render_types::RenderExpr;

use crate::reactive_view_model::CollectionVariant;
use crate::reactive_view_model::InterpretFn;
use crate::reactive_view_model::ReactiveViewModel;
use crate::render_context::AvailableSpace;
use crate::render_context::LayoutHint;
use crate::view_model::ViewModel;

/// Build a per-row `RenderContext` for interpreting a collection's item
/// template.
///
/// Resolves the row's entity profile and attaches its operations so builders
/// like `state_toggle` and `editable_text` get wired up even when the item
/// template is a custom expression (e.g.
/// `row(state_toggle(col("task_state")))`) rather than the default
/// `live_block()`.
///
/// `parent_space` is the container-query allocation this subtree was allotted
/// by its parent. It flows into `pick_active_variant` via `ctx.available_space`
/// so profile variants can key on `available_width_px` etc. Passing `None`
/// means "no refined allocation; fall back to global viewport." **Threading
/// this explicitly is the landmine fix** — an earlier draft used
/// `RenderContext::default().with_row(row)` which silently dropped any
/// `available_space` set by the containing builder.
pub(crate) fn row_render_context(
    row: Arc<holon_api::widget_spec::DataRow>,
    handle: Option<futures_signals::signal::ReadOnlyMutable<Arc<holon_api::widget_spec::DataRow>>>,
    services: &dyn crate::reactive::BuilderServices,
    parent_space: Option<AvailableSpace>,
) -> crate::RenderContext {
    let mut base = match handle {
        Some(h) => crate::RenderContext::default().with_row_mutable(h),
        None => crate::RenderContext::default().with_row(row),
    };
    if let Some(space) = parent_space {
        base = base.with_available_space(space);
    }
    let ops: Vec<holon_api::render_types::OperationWiring> = services
        .resolve_profile(base.row())
        .map(|p| {
            p.operations
                .into_iter()
                .map(|d| d.to_default_wiring())
                .collect()
        })
        .unwrap_or_default();
    if ops.is_empty() {
        base
    } else {
        base.with_operations(ops, services)
    }
}

// ── ReactiveView ────────────────────────────────────────────────────────

/// A self-managing reactive view that owns its data pipeline.
///
/// Replaces the old pattern of `ReactiveCollection` + external
/// `wire_collection_drivers`. The driver is spawned internally and stopped on
/// Drop (or explicit `stop()`).
pub struct ReactiveView {
    inner: ReactiveViewInner,
    pub items: MutableVec<Arc<ReactiveViewModel>>,
    driver_handle: Mutex<Option<AbortHandle>>,
}

/// Virtual child slot: entity profile defaults + parent context.
#[derive(Clone, Debug)]
pub struct VirtualChildSlot {
    pub defaults: std::collections::HashMap<String, holon_api::Value>,
    pub parent_id: holon_api::EntityUri,
    /// Opt-in (`creation_slot: true`) that permits the top-level "create a new
    /// root entity" slot for a flat forest parented to the `no_parent`
    /// sentinel. Read-only navigation lists (the Pages sidebar) leave this
    /// `false` so they render no phantom `sentinel:__virtual:no_parent` row
    /// (BugFunnel #61). The single-root main-panel slot is unaffected by
    /// this flag.
    pub allow_root_creation: bool,
}

/// Where an [`AppendedRowsProvider`]'s suffix row comes from — the only thing
/// that varies between the empty-collection creation slot and a display-placed
/// occurrence.
enum SuffixSource {
    /// The empty-collection creation slot, parented to the query's focus root
    /// (bug 2A). The parent is NOT static — it is resolved at render time from
    /// `inner`'s rows via `row_origin::resolve_creation_parent`, so a new block
    /// created at the bottom of a panel is parented to the focused block
    /// (`fr.root_id`), not the panel container. Re-emits whenever the rowset
    /// changes (focus navigation arrives as a Data event, not a Structure
    /// rebuild, so the slot's parent MUST track it). Emits nothing while the
    /// rowset is not yet resolvable (transient-empty on first load).
    CreationSlot {
        defaults: std::collections::HashMap<String, holon_api::Value>,
        container: holon_api::EntityUri,
        allow_root_creation: bool,
        inner: Arc<dyn ReactiveRowProvider>,
    },
    /// A LIVE row derived from a canonical block's row cell (ADR 0015 P2
    /// display placement). The `id` stays the canonical id → edits route to
    /// canonical; `anchor` is the display-local `parent_id`. Re-emits
    /// whenever `source` changes, so the placed occurrence stays converged
    /// with the canonical block by construction (ADR 0015 §1a shared cell).
    /// `occurrence` is the display coordinate that rides the row-identity
    /// key (never the `id` string — ADR 0015 rule 4), so a same-collection
    /// second occurrence keys distinctly.
    LiveCell {
        key: holon_api::EntityUri,
        occurrence: holon_api::OccurrenceId,
        anchor: EntityUri,
        source: futures_signals::signal::ReadOnlyMutable<Arc<holon_api::widget_spec::DataRow>>,
    },
}

/// Build the keyed synthetic creation-slot row for a resolved parent. The
/// `:__virtual:` id makes `RowOrigin::from_id` yield `CreationPlaceholder` so
/// the editor's submit handler materializes a real entity under `parent`.
fn creation_slot_keyed_row(
    parent: &holon_api::EntityUri,
    defaults: &std::collections::HashMap<String, holon_api::Value>,
) -> (holon_api::RowKey, Arc<holon_api::widget_spec::DataRow>) {
    use holon_api::Value;
    let virtual_id = crate::row_origin::RowOrigin::creation_placeholder_id(parent);
    // Defaults FIRST: they carry the entity's declared schema (Null-seeded for
    // columns the profile does not set), so the structural columns below must
    // overwrite them, never the other way round.
    let mut row: std::collections::HashMap<String, holon_api::Value> = defaults.clone();
    row.insert("id".to_string(), Value::String(virtual_id));
    row.insert(
        "parent_id".to_string(),
        Value::String(parent.as_str().to_string()),
    );
    // Max-scalar string sentinel so the slot sorts last (see `Static` note).
    row.insert(
        "sort_key".to_string(),
        Value::String("\u{10FFFF}".to_string()),
    );
    let key =
        holon_api::data_row_entity_uri(&row).expect("creation-slot row carries an 'id' column");
    ((key, holon_api::Occurrence::Canonical), Arc::new(row))
}

impl SuffixSource {
    /// The current suffix row(s), keyed — for a synchronous snapshot.
    fn current_keyed(&self) -> Vec<(holon_api::RowKey, Arc<holon_api::widget_spec::DataRow>)> {
        match self {
            SuffixSource::CreationSlot {
                defaults,
                container,
                allow_root_creation,
                inner,
            } => {
                match crate::row_origin::resolve_creation_parent(
                    &inner.rows_snapshot(),
                    container,
                    *allow_root_creation,
                ) {
                    Some(parent) => vec![creation_slot_keyed_row(&parent, defaults)],
                    None => vec![],
                }
            }
            SuffixSource::LiveCell {
                key,
                occurrence,
                anchor,
                source,
            } => vec![(
                (
                    key.clone(),
                    holon_api::Occurrence::Placed(occurrence.clone()),
                ),
                placed_occurrence_row(&source.get_cloned(), anchor),
            )],
        }
    }

    /// The live keyed suffix signal — `always` for `Static`, cell-derived for
    /// `LiveCell`.
    fn keyed_signal_vec(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn futures_signals::signal_vec::SignalVec<
                    Item = (holon_api::RowKey, Arc<holon_api::widget_spec::DataRow>),
                > + Send,
        >,
    > {
        use futures_signals::signal::SignalExt;
        use futures_signals::signal_vec::SignalVecExt;
        match self {
            SuffixSource::CreationSlot {
                defaults,
                container,
                allow_root_creation,
                inner,
            } => {
                let defaults = defaults.clone();
                let container = container.clone();
                let allow_root_creation = *allow_root_creation;
                Box::pin(
                    inner
                        .rows_signal_vec()
                        .to_signal_cloned()
                        .map(move |rows| {
                            match crate::row_origin::resolve_creation_parent(
                                &rows,
                                &container,
                                allow_root_creation,
                            ) {
                                Some(parent) => {
                                    vec![creation_slot_keyed_row(&parent, &defaults)]
                                }
                                None => vec![],
                            }
                        })
                        .to_signal_vec(),
                )
            }
            SuffixSource::LiveCell {
                key,
                occurrence,
                anchor,
                source,
            } => {
                let row_key = (
                    key.clone(),
                    holon_api::Occurrence::Placed(occurrence.clone()),
                );
                let anchor = anchor.clone();
                Box::pin(
                    source
                        .signal_cloned()
                        .map(move |src| {
                            vec![(row_key.clone(), placed_occurrence_row(&src, &anchor))]
                        })
                        .to_signal_vec(),
                )
            }
        }
    }
}

/// ONE injector for every "append row(s) after a collection's real rows" case:
/// the empty-collection creation slot (`SuffixSource::Static`, formerly
/// `VirtualChildRowProvider`) AND a display-placed second occurrence
/// (`SuffixSource::LiveCell`, formerly `PlacedRowProvider`). A provider-wrapper
/// that `.chain()`s a keyed suffix onto `inner`; only the suffix's shape
/// varies.
///
/// The behavior that forks — submit-to-create vs edit-canonical — is dispatched
/// by `RowOrigin` off the row's `id` column (`view_event_handler`), NOT here,
/// so this single type honors ADR 0015 §5 ("shared render path, separate type":
/// the verbs stay in `RowOrigin`, the render path is shared).
struct AppendedRowsProvider {
    inner: Arc<dyn ReactiveRowProvider>,
    suffix: SuffixSource,
}

impl AppendedRowsProvider {
    /// The empty-collection creation slot. Its `:__virtual:` id makes the
    /// editor's submit handler (`RowOrigin::from_id`) materialize a real entity
    /// on first edit. The parent is resolved reactively from `inner`'s rows
    /// (`SuffixSource::CreationSlot`) — the query's focus root, NOT the static
    /// container id `slot.parent_id` (bug 2A). `slot.parent_id` is threaded on
    /// only as the container hint used to recognise the flat `from children`
    /// shape; the max-scalar `sort_key` that keeps the slot last is applied in
    /// `creation_slot_keyed_row`.
    fn creation_slot(inner: Arc<dyn ReactiveRowProvider>, slot: &VirtualChildSlot) -> Self {
        Self {
            inner: inner.clone(),
            suffix: SuffixSource::CreationSlot {
                defaults: slot.defaults.clone(),
                container: slot.parent_id.clone(),
                allow_root_creation: slot.allow_root_creation,
                inner,
            },
        }
    }

    /// A display-placed second occurrence of `placed_id` under `anchor`,
    /// tracking the canonical block's live row cell. Formerly
    /// `PlacedRowProvider::new`.
    ///
    /// The suffix keys `(placed_id, Occurrence::Placed(occ))` where `occ` is
    /// minted deterministically from `(placed_id, anchor)` — so the occurrence
    /// coordinate lives in the row-identity key (ADR 0015 rule 4: never an
    /// id-infix) and a same-collection second occurrence of `placed_id` no
    /// longer collides with the canonical row under the widened driver
    /// keyspace.
    // Wired into collection assembly in Increment B step-6; proven by
    // `tests::appended_rows_provider_injects_live_second_occurrence`.
    #[allow(dead_code)]
    pub(crate) fn placement(
        inner: Arc<dyn ReactiveRowProvider>,
        placed_id: EntityUri,
        anchor: EntityUri,
        source: futures_signals::signal::ReadOnlyMutable<Arc<holon_api::widget_spec::DataRow>>,
    ) -> Self {
        let occurrence = holon_api::OccurrenceId::for_placement(&placed_id, &anchor);
        Self {
            inner,
            suffix: SuffixSource::LiveCell {
                key: placed_id,
                occurrence,
                anchor,
                source,
            },
        }
    }
}

impl ReactiveRowProvider for AppendedRowsProvider {
    fn rows_snapshot(&self) -> Vec<Arc<holon_api::widget_spec::DataRow>> {
        let mut rows = self.inner.rows_snapshot();
        rows.extend(self.suffix.current_keyed().into_iter().map(|(_, r)| r));
        rows
    }

    fn rows_signal_vec(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn futures_signals::signal_vec::SignalVec<Item = Arc<holon_api::widget_spec::DataRow>>
                + Send,
        >,
    > {
        use futures_signals::signal_vec::SignalVecExt;
        let suffix = self.suffix.keyed_signal_vec().map(|(_, r)| r);
        Box::pin(self.inner.rows_signal_vec().chain(suffix))
    }

    fn keyed_rows_signal_vec(
        &self,
    ) -> std::pin::Pin<
        Box<
            dyn futures_signals::signal_vec::SignalVec<
                    Item = (holon_api::RowKey, Arc<holon_api::widget_spec::DataRow>),
                > + Send,
        >,
    > {
        use futures_signals::signal_vec::SignalVecExt;
        Box::pin(
            self.inner
                .keyed_rows_signal_vec()
                .chain(self.suffix.keyed_signal_vec()),
        )
    }

    fn cache_identity(&self) -> u64 {
        self.inner.cache_identity()
    }
}

/// Derive a display-placed occurrence's `DataRow` from the canonical block's
/// live row. Keeps the `id` column = the canonical id (so `view_event_handler`
/// routes edits to canonical — ADR 0015 rule 3), overrides `parent_id` to the
/// display-local anchor (display-only placement — rule 1), and stamps a
/// sentinel `sort_key` so the occurrence sorts last (mirrors the creation-slot
/// convention). Content and every other field are copied from the live source,
/// so the placed row tracks the canonical block by construction. Used by
/// `SuffixSource::LiveCell`.
fn placed_occurrence_row(
    source: &Arc<holon_api::widget_spec::DataRow>,
    anchor: &EntityUri,
) -> Arc<holon_api::widget_spec::DataRow> {
    use holon_api::Value;
    let mut row = (**source).clone();
    row.insert(
        "parent_id".to_string(),
        Value::String(anchor.as_str().to_string()),
    );
    row.insert(
        "sort_key".to_string(),
        Value::String("\u{10FFFF}".to_string()),
    );
    Arc::new(row)
}

/// The READ-ONLY template a woven advice row (`Occurrence::Placed`) renders as
/// (ADR 0021 v1 read-only children). Deliberately NOT the collection's
/// `item_template`: an advice row is a display-only relevance suggestion, never
/// an editable block. Built as a plain `RenderExpr` tree (no Rhai) — note the
/// `navigation.focus` / `dismiss_advice` names must be the resolved dot/verb
/// forms the interpreter expects, since we bypass parse-time aliasing.
///
/// Shape: `selectable(row(text(col("content")), op_button("dismiss_advice")),
/// action: navigation.focus(block_id: col("id")))`.
/// - `text(col("content"))` — read-only lesson content (NO `editable_text`).
/// - `op_button("dismiss_advice")` — the dismiss affordance; GPUI maps the op
///   name to an icon. The synthesized row carries `target_id`/`anchor_id`
///   columns so dispatch can bind `dismiss_advice`'s `anchor_id` + `lesson_id`.
/// - `selectable(action: navigation.focus)` — click-through to the canonical
///   lesson (`col("id")` is the canonical lesson id).
pub(crate) fn advice_readonly_template() -> holon_api::render_types::RenderExpr {
    use holon_api::Value;
    use holon_api::render_types::Arg;
    use holon_api::render_types::RenderExpr;
    fn call(name: &str, args: Vec<Arg>) -> RenderExpr {
        RenderExpr::FunctionCall {
            name: name.to_string(),
            args,
        }
    }
    fn col(name: &str) -> RenderExpr {
        RenderExpr::ColumnRef {
            name: name.to_string(),
        }
    }
    fn pos(value: RenderExpr) -> Arg {
        Arg { name: None, value }
    }
    fn named(name: &str, value: RenderExpr) -> Arg {
        Arg {
            name: Some(name.to_string()),
            value,
        }
    }
    fn lit(s: &str) -> RenderExpr {
        RenderExpr::Literal {
            value: Value::String(s.to_string()),
        }
    }

    let content = call("text", vec![pos(col("content"))]);
    let dismiss = call("op_button", vec![pos(lit("dismiss_advice"))]);
    let inner = call("row", vec![pos(content), pos(dismiss)]);
    let action = call("navigation.focus", vec![named("block_id", col("id"))]);
    call("selectable", vec![pos(inner), named("action", action)])
}

/// Configuration for creating a collection ReactiveView.
pub struct CollectionConfig {
    pub layout: CollectionVariant,
    pub item_template: RenderExpr,
    pub sort_key: Option<String>,
    /// When set, the driver appends a virtual editable placeholder after all
    /// real rows. The virtual entity is rendered through the normal entity
    /// profile pipeline via `render_entity()`.
    pub virtual_child: Option<VirtualChildSlot>,
    /// `rules:` arg parsed at builder construction; the driver evaluates
    /// each rule's predicate per row and merges matching `override` maps
    /// into the row's `ctx.flags`. Empty = no overrides applied. See
    /// `crate::row_pipeline` for the per-row pipeline.
    ///
    /// Streaming rules see the row's own columns plus, on the TREE driver,
    /// the `level`/`depth` positional keys (computed from the rowset's
    /// parent chain). position/count/is_first/is_last are NOT injected
    /// (the driver receives rows incrementally via VecDiff and the
    /// collection size shifts with each event).
    pub rules: Vec<holon_api::render_types::RuleSpec>,
}

/// Pure function that partitions a parent's container-query allocation
/// among `count` equally-important children. Used by layout containers
/// (currently only `columns`) to refine `available_space` as it flows from
/// a parent into its children.
///
/// Must be a pure function: no services, no context, no signals. This is
/// enforced structurally (no capture of anything reactive) so that the
/// signal-cascade model stays acyclic.
pub type ChildSpaceFn = dyn Fn(AvailableSpace, usize) -> AvailableSpace + Send + Sync;

/// Interprets one row at a known tree depth + display occurrence into a
/// view model and its resolved props map.
type InterpretRowFn = dyn Fn(
        Arc<holon_api::widget_spec::DataRow>,
        usize,
        holon_api::Occurrence,
    ) -> (Arc<ReactiveViewModel>, HashMap<String, holon_api::Value>)
    + Send
    + Sync;

/// Shared, lock-guarded snapshot of the rows currently backing a flat
/// driver, keyed for stable-identity lookups on the next diff.
type RowEntries = Arc<Mutex<Vec<(holon_api::RowKey, Arc<holon_api::widget_spec::DataRow>)>>>;

enum ReactiveViewInner {
    /// A block with its own watcher and child management.
    Block {
        _block_id: EntityUri,
        data_source: Arc<dyn ReactiveRowProvider>,
        item_template: RenderExpr,
        /// Container-query allocation this view was given by its parent.
        /// Drivers read this at interpret time and pass it into
        /// `row_render_context` so per-row `pick_active_variant` sees the
        /// refined space. Initially `None`; updated reactively by the
        /// enclosing layout container (e.g. `columns`) or by the root
        /// bootstrap wiring it to `UiState::viewport`.
        space: Mutable<Option<AvailableSpace>>,
    },
    /// A collection rendering rows from a parent's data source.
    Collection {
        layout: CollectionVariant,
        data_source: Arc<dyn ReactiveRowProvider>,
        item_template: RenderExpr,
        /// Shared template Mutable — cloned into the flat driver so a single
        /// `set_template()` call re-interprets all items' props in place.
        template_mutable: Mutable<RenderExpr>,
        /// When set, the driver sorts incoming rows by this column name
        /// before pushing them into `items`. Only plain `col(name)` sort keys
        /// are supported — expression-derived keys are a compile error.
        sort_key: Option<String>,
        /// Container-query allocation for this collection's subtree.
        /// See `Block::space` for the full story.
        space: Mutable<Option<AvailableSpace>>,
        /// Optional partition function applied per-child. When `Some`, the
        /// flat driver computes `child_space = child_space_fn(space, count)`
        /// once per re-interpret and passes that refined value to every
        /// row's `row_render_context`. When `None`, children inherit the
        /// parent's space unchanged (the "non-partitioning container"
        /// default used by `list`, `table`, `outline`, `tree`).
        child_space_fn: Option<Arc<ChildSpaceFn>>,
        /// When set, the driver appends one virtual editable placeholder
        /// after all real rows, rendered via `render_entity()`.
        virtual_child: Option<VirtualChildSlot>,
        /// `rules:` arg from the DSL; the driver applies each rule's
        /// `when` predicate per row and merges matching `override` maps
        /// into the row's `ctx.flags`. Empty = no rules effect. See
        /// `crate::row_pipeline::apply_rules_and_interpret_with_ctx`.
        rules: Vec<holon_api::render_types::RuleSpec>,
    },
    /// A grouped collection (board, future calendar, …): rows are
    /// partitioned by a row column at runtime and the result is a list of
    /// "lane" view models, each containing the cards that fall into that
    /// group. Driven by a SINGLE upstream subscription that owns the
    /// partitioning, so cross-lane row movement is atomic from the GPUI
    /// render's perspective — there is no race between independent
    /// per-lane filters.
    Grouped {
        layout: CollectionVariant,
        data_source: Arc<dyn ReactiveRowProvider>,
        /// Per-card render expression. Each row that lands in a lane is
        /// interpreted through this template.
        item_template: RenderExpr,
        /// `rules:` arg from the DSL (FU-6). Driver applies each rule's
        /// predicate per card and merges matching `override` maps into
        /// the card's `ctx.flags`. Empty = no overrides.
        rules: Vec<holon_api::render_types::RuleSpec>,
        /// Column name used to bucket rows into lanes (e.g. `task_state`).
        lane_field: String,
        /// Title for the lane that collects rows whose `lane_field` is
        /// missing or empty.
        lane_label_default: String,
        /// Caller-preferred lane order. Lanes not listed are appended in
        /// lexicographic order — matches the static-path semantics.
        lane_order: Vec<String>,
        /// Per-card sort column inside a lane. Rows missing this column
        /// fall back to insertion order.
        sort_key: Option<String>,
        /// Container-query allocation for this collection's subtree.
        space: Mutable<Option<AvailableSpace>>,
    },
    /// Static content — no driver, no signals.
    Static,
    /// Static collection with a layout variant (for snapshot consumers).
    StaticCollection { layout: CollectionVariant },
    /// Positional heterogeneous children with reactive space propagation.
    ///
    /// Unlike `Collection` (homogeneous rows + single item_template),
    /// each child has its own `RenderExpr` and `LayoutHint`. The driver
    /// watches `parent_space` and recomputes the Fixed/Flex partition,
    /// re-interpreting only children whose allocated space changed.
    ///
    /// Created by `columns()` Branch A when `available_space` is known.
    /// Falls back to `StaticCollection` for headless/snapshot consumers.
    PartitionedStatic {
        layout: CollectionVariant,
        /// Per-child render expression + layout hint (from Phase 1).
        children_config: Vec<(RenderExpr, LayoutHint)>,
        parent_space: Mutable<Option<AvailableSpace>>,
        gap: f32,
    },
}

impl ReactiveView {
    /// Create a view for a block (owns its watcher).
    pub fn new_block(
        block_id: EntityUri,
        data_source: Arc<dyn ReactiveRowProvider>,
        item_template: RenderExpr,
        initial_space: Option<AvailableSpace>,
    ) -> Self {
        Self {
            inner: ReactiveViewInner::Block {
                _block_id: block_id,
                data_source,
                item_template,
                space: Mutable::new(initial_space),
            },
            items: MutableVec::new(),
            driver_handle: Mutex::new(None),
        }
    }

    /// Create a view for a collection (table/tree/list/outline/columns).
    pub fn new_collection(
        config: CollectionConfig,
        data_source: Arc<dyn ReactiveRowProvider>,
        initial_space: Option<AvailableSpace>,
        child_space_fn: Option<Arc<ChildSpaceFn>>,
    ) -> Self {
        let template_mutable = Mutable::new(config.item_template.clone());
        let sort_key = config.sort_key;
        Self {
            inner: ReactiveViewInner::Collection {
                layout: config.layout,
                data_source,
                item_template: config.item_template,
                template_mutable,
                sort_key,
                space: Mutable::new(initial_space),
                child_space_fn,
                virtual_child: config.virtual_child,
                rules: config.rules,
            },
            items: MutableVec::new(),
            driver_handle: Mutex::new(None),
        }
    }

    /// SignalVec of the collection's real `items` — the children to render.
    ///
    /// The stable "children to render" seam. The creation slot is injected
    /// upstream as a real row (streaming path: `AppendedRowsProvider`), so it
    /// arrives through `items` like any other row — there is no ViewModel-level
    /// suffix to chain.
    pub fn children_signal_vec(
        &self,
    ) -> std::pin::Pin<
        Box<dyn futures_signals::signal_vec::SignalVec<Item = Arc<ReactiveViewModel>> + Send>,
    > {
        Box::pin(self.items.signal_vec_cloned())
    }

    /// Eager snapshot of the collection's real `items` — the children to
    /// render.
    ///
    /// Used by initial-render sites that read the current children
    /// synchronously.
    pub fn children_snapshot(&self) -> Vec<Arc<ReactiveViewModel>> {
        self.items.lock_ref().iter().cloned().collect()
    }

    /// Handle to the container-query space `Mutable` for this view, if the
    /// variant supports it. Static variants have no space (they're
    /// interpretation-time snapshots).
    ///
    /// Used by the enclosing layout container (e.g. `columns` in Phase 3)
    /// to push refined partitioned space into child views when its own space
    /// changes.
    pub fn space_mutable(&self) -> Option<&Mutable<Option<AvailableSpace>>> {
        match &self.inner {
            ReactiveViewInner::Block { space, .. }
            | ReactiveViewInner::Collection { space, .. }
            | ReactiveViewInner::Grouped { space, .. } => Some(space),
            ReactiveViewInner::PartitionedStatic { parent_space, .. } => Some(parent_space),
            ReactiveViewInner::Static | ReactiveViewInner::StaticCollection { .. } => None,
        }
    }

    /// Update the container-query allocation for this view. Uses
    /// `Mutable::set_neq` so a no-op update is free at the signal level.
    pub fn set_space(&self, space: Option<AvailableSpace>) {
        if let Some(m) = self.space_mutable() {
            m.set_neq(space);
        }
    }

    /// Create a partitioned static view for positional heterogeneous children.
    ///
    /// Each child has its own `RenderExpr` and `LayoutHint`. The driver watches
    /// `parent_space` and recomputes the partition on space changes.
    /// `initial_items` are the Phase 1 results (already correctly interpreted
    /// for the initial viewport).
    pub fn new_partitioned_static(
        initial_items: Vec<ReactiveViewModel>,
        children_config: Vec<(RenderExpr, LayoutHint)>,
        gap: f32,
        initial_space: Option<AvailableSpace>,
        layout: CollectionVariant,
    ) -> Self {
        let arced: Vec<Arc<ReactiveViewModel>> = initial_items.into_iter().map(Arc::new).collect();
        Self {
            inner: ReactiveViewInner::PartitionedStatic {
                layout,
                children_config,
                parent_space: Mutable::new(initial_space),
                gap,
            },
            items: MutableVec::new_with_values(arced),
            driver_handle: Mutex::new(None),
        }
    }

    /// Create a grouped view (board, future calendar, …) — partitioning is
    /// owned by ONE driver so cross-lane row movement is atomic from
    /// GPUI's render perspective.
    #[allow(clippy::too_many_arguments)]
    pub fn new_grouped(
        layout: CollectionVariant,
        data_source: Arc<dyn ReactiveRowProvider>,
        item_template: RenderExpr,
        lane_field: String,
        lane_label_default: String,
        lane_order: Vec<String>,
        sort_key: Option<String>,
        initial_space: Option<AvailableSpace>,
        rules: Vec<holon_api::render_types::RuleSpec>,
    ) -> Self {
        Self {
            inner: ReactiveViewInner::Grouped {
                layout,
                data_source,
                item_template,
                rules,
                lane_field,
                lane_label_default,
                lane_order,
                sort_key,
                space: Mutable::new(initial_space),
            },
            items: MutableVec::new(),
            driver_handle: Mutex::new(None),
        }
    }

    /// Create a static view (no driver, items populated once).
    pub fn new_static(items: Vec<ReactiveViewModel>) -> Self {
        let arced: Vec<Arc<ReactiveViewModel>> = items.into_iter().map(Arc::new).collect();
        Self {
            inner: ReactiveViewInner::Static,
            items: MutableVec::new_with_values(arced),
            driver_handle: Mutex::new(None),
        }
    }

    /// Create a static view with a specific layout variant.
    pub fn new_static_with_layout(
        items: Vec<ReactiveViewModel>,
        layout: CollectionVariant,
    ) -> Self {
        let arced: Vec<Arc<ReactiveViewModel>> = items.into_iter().map(Arc::new).collect();
        Self {
            inner: ReactiveViewInner::StaticCollection { layout },
            items: MutableVec::new_with_values(arced),
            driver_handle: Mutex::new(None),
        }
    }

    /// The collection layout variant, if this is a collection.
    pub fn layout(&self) -> Option<CollectionVariant> {
        match &self.inner {
            ReactiveViewInner::Collection { layout, .. }
            | ReactiveViewInner::Grouped { layout, .. }
            | ReactiveViewInner::StaticCollection { layout }
            | ReactiveViewInner::PartitionedStatic { layout, .. } => Some(layout.clone()),
            _ => None,
        }
    }

    /// Underlying row-set provider for streaming variants (`Block` /
    /// `Collection`). Returns `None` for static variants. Used by PBT
    /// invariants that walk the reactive tree to assert cache identity
    /// / arg variance of value-fn providers (`focus_chain`, `ops_of`,
    /// `chain_ops`).
    pub fn data_source(&self) -> Option<&Arc<dyn ReactiveRowProvider>> {
        match &self.inner {
            ReactiveViewInner::Block { data_source, .. }
            | ReactiveViewInner::Collection { data_source, .. } => Some(data_source),
            _ => None,
        }
    }

    /// Set the collection's item template at runtime.
    ///
    /// The template driver (spawned by `start()`) watches this Mutable and
    /// re-interprets all items' props in place when it changes. GPUI's props
    /// watchers detect the change and re-render — no full rebuild needed.
    pub fn set_template(&self, new_template: RenderExpr) {
        match &self.inner {
            ReactiveViewInner::Collection {
                template_mutable, ..
            } => {
                template_mutable.set(new_template);
            }
            _ => {
                tracing::warn!("[ReactiveView::set_template] called on non-collection variant");
            }
        }
    }

    /// Per-row template expression for streaming variants. Used together
    /// with `data_source()` to spot collections whose rows are produced
    /// by a given value function.
    pub fn item_template(&self) -> Option<&RenderExpr> {
        match &self.inner {
            ReactiveViewInner::Block { item_template, .. }
            | ReactiveViewInner::Collection { item_template, .. } => Some(item_template),
            _ => None,
        }
    }

    /// Stable identity for entity caching across structural rebuilds.
    ///
    /// When a block's interpreted tree is structurally rebuilt, a brand-new
    /// `Arc<ReactiveView>` is created but it wraps the same underlying
    /// `Arc<ReactiveRenderedRows>` (the block's data source) and the same
    /// item template. We derive the cache key from those — so a downstream
    /// consumer (`frontends/gpui/src/render/builders/mod.rs`) can reuse the
    /// same GPUI entity across rebuilds and preserve its `ListState`
    /// (scroll position, measured row heights).
    ///
    /// For the Static variant there's no data source; we fall back to the
    /// pointer of `items`, which is stable for static content.
    pub fn stable_cache_key(&self) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hash;
        use std::hash::Hasher;
        let mut h = DefaultHasher::new();
        // Include the layout variant so two collections that share a data
        // source + item_template but differ in variant (e.g. a `table_view`
        // and a `tree_view` on the same block, swapped via
        // `view_mode_switcher`) hash differently. Without this, the GPUI
        // entity cache returns the stale shell from the previous mode and
        // the user sees table-layout rows even after clicking "tree".
        // Regression-guarded by `layout_pbt.rs`'s shared-data_source
        // LiveBlock arm.
        format!("{:?}", self.layout()).hash(&mut h);
        match &self.inner {
            ReactiveViewInner::Block {
                data_source,
                item_template,
                ..
            }
            | ReactiveViewInner::Collection {
                data_source,
                item_template,
                ..
            } => {
                // `cache_identity()` — trait method. Stable for the
                // provider's lifetime; a concrete `ReactiveRenderedRows`
                // hashes its inner `ReactiveRowSet`, so two QRs wrapping
                // the same row set share identity (synthetic providers
                // define their own identity policy).
                data_source.cache_identity().hash(&mut h);
                format!("{:?}", item_template).hash(&mut h);
            }
            ReactiveViewInner::Grouped {
                data_source,
                item_template,
                lane_field,
                ..
            } => {
                data_source.cache_identity().hash(&mut h);
                format!("{:?}", item_template).hash(&mut h);
                lane_field.hash(&mut h);
            }
            ReactiveViewInner::PartitionedStatic {
                children_config, ..
            } => {
                for (expr, hint) in children_config {
                    format!("{:?}", expr).hash(&mut h);
                    format!("{:?}", hint).hash(&mut h);
                }
            }
            ReactiveViewInner::Static | ReactiveViewInner::StaticCollection { .. } => {
                (&self.items as *const _ as usize).hash(&mut h);
            }
        }
        h.finish()
    }

    /// Start the streaming pipeline. Spawns the driver internally, stores
    /// AbortHandle. No-op for Static variant.
    pub fn start(
        &self,
        services: Arc<dyn crate::reactive::BuilderServices>,
        rt: &tokio::runtime::Handle,
    ) {
        if matches!(
            self.inner,
            ReactiveViewInner::Static | ReactiveViewInner::StaticCollection { .. }
        ) {
            tracing::debug!("[ReactiveView::start] skipped — static variant");
            return;
        }

        tracing::debug!(
            "[ReactiveView::start] starting driver, layout={:?}",
            self.layout()
        );

        // Stop any existing driver first
        self.stop();

        let driver = self.create_driver(services);
        let (abort_handle, abort_reg) = AbortHandle::new_pair();
        let abortable = futures::future::Abortable::new(driver, abort_reg);

        rt.spawn(async move {
            let _ = abortable.await; // Ok(()) on completion, Err on abort —
            // both fine
        });

        *self.driver_handle.lock().unwrap() = Some(abort_handle);
    }

    /// Stop the driver. Called on Drop, or explicitly before replacing.
    pub fn stop(&self) {
        if let Some(handle) = self.driver_handle.lock().unwrap().take() {
            handle.abort();
        }
    }

    /// Create the driver future for this view.
    fn create_driver(
        &self,
        services: Arc<dyn crate::reactive::BuilderServices>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        match &self.inner {
            ReactiveViewInner::Block {
                data_source,
                item_template,
                space,
                ..
            } => {
                let block_template = Mutable::new(item_template.clone());
                self.create_flat_driver(
                    data_source,
                    item_template,
                    &block_template,
                    space,
                    None,
                    services,
                )
            }
            ReactiveViewInner::Collection {
                data_source,
                item_template,
                template_mutable,
                space,
                child_space_fn,
                layout,
                virtual_child,
                ..
            } => {
                // Advice weave (ADR 0022) is NOT wired here: it lives in the
                // session-level sidecar (`crate::advice_weaver`) that the pure
                // interpret path reads synchronously via
                // `BuilderServices::advice_children`, so it is observable on the
                // snapshot/MCP path — not only on this streaming driver. See the
                // static collection builders (`shadow_builders::weave_advice`).
                let effective_source: Arc<dyn ReactiveRowProvider> = match virtual_child {
                    Some(slot) => Arc::new(AppendedRowsProvider::creation_slot(
                        data_source.clone(),
                        slot,
                    )),
                    None => data_source.clone(),
                };
                let is_tree = layout.is_hierarchical();
                if is_tree {
                    self.create_tree_driver(&effective_source, item_template, space, services)
                } else {
                    self.create_flat_driver(
                        &effective_source,
                        item_template,
                        template_mutable,
                        space,
                        child_space_fn.clone(),
                        services,
                    )
                }
            }
            ReactiveViewInner::PartitionedStatic {
                children_config,
                parent_space,
                gap,
                ..
            } => self.create_partitioned_driver(children_config, parent_space, *gap, services),
            ReactiveViewInner::Grouped {
                data_source,
                item_template,
                rules,
                lane_field,
                lane_label_default,
                lane_order,
                sort_key,
                space,
                ..
            } => self.create_grouped_driver(
                data_source,
                item_template,
                rules,
                lane_field,
                lane_label_default,
                lane_order,
                sort_key,
                space,
                services,
            ),
            ReactiveViewInner::Static | ReactiveViewInner::StaticCollection { .. } => {
                Box::pin(std::future::pending())
            }
        }
    }

    /// Tree/Outline driver: uses MutableTree for parent-child structural
    /// updates.
    ///
    /// **v1 limitation**: this driver reads the container-query `space`
    /// once at startup and does not re-interpret on space changes. Trees
    /// inside blocks will not adapt to viewport changes until the next
    /// structural rebuild. The flat driver is space-reactive; trees are a
    /// follow-up because their keyed incremental diff model is harder to
    /// combine with a space signal.
    fn create_tree_driver(
        &self,
        data_source: &Arc<dyn ReactiveRowProvider>,
        item_template: &RenderExpr,
        space: &Mutable<Option<AvailableSpace>>,
        services: Arc<dyn crate::reactive::BuilderServices>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        use holon_api::widget_spec::data_row_parent_id as extract_parent_id;
        use holon_api::widget_spec::data_row_sort_key as extract_sort_key;

        use crate::mutable_tree::MutableTree;

        // `tree` and `key_index` need to be reachable from both the data
        // driver and the focus driver (added below); wrap in Arc<Mutex<_>> so
        // both drivers can mutate. `row_map` is new — it mirrors the rows
        // currently in the tree so the focus driver can re-interpret affected
        // rows without going back to the backend.
        let tree = Arc::new(Mutex::new(MutableTree::new(self.items.clone())));
        let key_index: Arc<Mutex<Vec<holon_api::RowKey>>> = Arc::new(Mutex::new(Vec::new()));
        let row_map: Arc<Mutex<HashMap<holon_api::RowKey, Arc<holon_api::widget_spec::DataRow>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        // Capture the focus signal before `services` is moved into the
        // interpret closures below.
        let focus_mutable = services.focused_block_mutable();
        let tmpl = item_template.clone();
        let space_handle = space.clone();

        let config_sort_key: Option<String> = match &self.inner {
            ReactiveViewInner::Collection { sort_key, .. } => sort_key.clone(),
            _ => None,
        };
        let config_rules: Vec<holon_api::render_types::RuleSpec> = match &self.inner {
            ReactiveViewInner::Collection { rules, .. } => rules.clone(),
            _ => Vec::new(),
        };

        let node_interpret_fn: InterpretFn = {
            let svc = services.clone();
            let space = space_handle.clone();
            let ds = data_source.clone();
            let fast_widget = match item_template {
                RenderExpr::FunctionCall { name, .. }
                    if crate::render_interpreter::is_props_only_widget(name) =>
                {
                    Some(name.clone())
                }
                _ => None,
            };
            Arc::new(move |expr, data| {
                let parent_space = space.get_cloned();
                if let Some(fast_widget) = fast_widget.as_ref() {
                    return crate::render_interpreter::resolve_props(
                        fast_widget,
                        expr,
                        data,
                        svc.as_ref(),
                        parent_space,
                    );
                }
                let handle =
                    holon_api::data_row_entity_uri(data).and_then(|uri| ds.row_mutable(&uri));
                let ctx = row_render_context(data.clone(), handle, svc.as_ref(), parent_space);
                let fresh = svc.interpret(expr, &ctx);
                fresh.props.get_cloned()
            })
        };

        // Interprets a row at a known tree depth. Injects the same `level` /
        // `depth` positional keys as the static tree path so rules like
        // `eq("level", 0)` fire on the streaming path too; the returned
        // override map is threaded into the TreeItem wrapper by MutableTree.
        // The virtual creation-slot row skips rules to mirror the static
        // path, which appends the slot after rule evaluation.
        // `occurrence` rides in as node metadata (ADR 0015 rule 4), NOT in the
        // id string — it is the display coordinate the row's identity key
        // carries, stamped onto the node so GPUI can suffix its per-row keys.
        // `Canonical` for every real row.
        let interpret_row: Arc<InterpretRowFn> = {
            let svc = services.clone();
            let space = space_handle.clone();
            let nif = node_interpret_fn;
            let ds = data_source.clone();
            let rules = config_rules;
            let advice_tmpl = advice_readonly_template();
            Arc::new(
                move |row: Arc<holon_api::widget_spec::DataRow>,
                      depth: usize,
                      occurrence: holon_api::Occurrence| {
                    let parent_space = space.get_cloned();
                    let handle =
                        holon_api::data_row_entity_uri(&row).and_then(|uri| ds.row_mutable(&uri));
                    let ctx = row_render_context(row.clone(), handle, svc.as_ref(), parent_space);
                    let is_virtual =
                        crate::row_origin::RowOrigin::from_row(&row).is_creation_placeholder();
                    // A woven advice row (ADR 0022) rides `Occurrence::Placed`
                    // in its identity key. It renders READ-ONLY (ADR 0021 v1) —
                    // the dismiss/click template, never the collection's editable
                    // `item_template` — and skips rules (like the virtual slot).
                    let is_advice = occurrence != holon_api::Occurrence::Canonical;
                    let template = if is_advice { &advice_tmpl } else { &tmpl };
                    let active_rules: &[holon_api::render_types::RuleSpec] =
                        if is_virtual || is_advice { &[] } else { &rules };
                    let positional = HashMap::from([
                        ("level".to_string(), holon_api::Value::Integer(depth as i64)),
                        ("depth".to_string(), holon_api::Value::Integer(depth as i64)),
                    ]);
                    let svc_for_interpret = svc.clone();
                    let (mut node, overrides) =
                        crate::row_pipeline::apply_rules_and_interpret_with_ctx(
                            ctx,
                            template,
                            active_rules,
                            &row,
                            positional,
                            move |expr, c| svc_for_interpret.interpret(expr, c),
                        );
                    node.interpret_fn = Some(nif.clone());
                    node.occurrence = occurrence;
                    (Arc::new(node), overrides)
                },
            )
        };

        // Depth computed the same way `MutableTree::insert` resolves
        // parenthood: walk stated parents while they are present in the
        // rowset. Bounded so a transient parent cycle can't hang the driver.
        // A stated parent always references a CANONICAL row (a display-placed
        // occurrence is never itself a parent target), so parent lookups key the
        // canonical occurrence.
        fn rowset_depth(
            row_map: &HashMap<holon_api::RowKey, Arc<holon_api::widget_spec::DataRow>>,
            row: &holon_api::widget_spec::DataRow,
        ) -> usize {
            let mut depth = 0usize;
            let mut cur = holon_api::widget_spec::data_row_parent_id(row);
            while depth <= row_map.len() {
                let parent_key = cur
                    .as_ref()
                    .map(|p| (p.clone(), holon_api::Occurrence::Canonical));
                match parent_key.and_then(|k| row_map.get(&k)) {
                    Some(parent_row) => {
                        depth += 1;
                        cur = holon_api::widget_spec::data_row_parent_id(parent_row);
                    }
                    None => break,
                }
            }
            // WP-F projection assertion (free — the loop above already counts the
            // walk): a chain longer than the rowset can only repeat a row, i.e. a
            // parent CYCLE. Previously this was swallowed by the `depth <= len`
            // bound and returned a bogus depth. The self-parented `sentinel:no_parent`
            // FK anchor never reaches this projection (filtered from the `block`
            // matview), so it cannot trip a false cycle. This closure returns
            // `usize` with no `Result` on the path → per the fail-loud directive a
            // `panic!` is used.
            if depth > row_map.len() {
                panic!(
                    "{}",
                    holon_api::ProjectionInvariantViolated {
                        detail: format!(
                            "rowset depth walk for row {:?} exceeded the {}-row set — parent cycle",
                            holon_api::widget_spec::data_row_parent_id(row),
                            row_map.len()
                        ),
                    }
                );
            }
            depth
        }

        let get_sort_key: Arc<dyn Fn(&holon_api::widget_spec::DataRow) -> String + Send + Sync> = {
            Arc::new(move |row: &holon_api::widget_spec::DataRow| -> String {
                match &config_sort_key {
                    Some(spec) => {
                        // Honor the `-`-prefixed DESCENDING convention (e.g.
                        // `sortkey: "-content"` for the newest-first journal
                        // feed). The tree sorts keys ascending, so a descending
                        // spec inverts the key ordering — mirroring the static
                        // path's `sorted_rows` reverse.
                        let (col, descending) = holon_api::render_eval::parse_sort_key(spec);
                        let key = holon_api::render_eval::sort_value(row.get(col));
                        if descending {
                            holon_api::render_eval::reverse_order_key(&key)
                        } else {
                            key
                        }
                    }
                    None => extract_sort_key(row),
                }
            })
        };

        let data_driver = {
            let tree = tree.clone();
            let key_index = key_index.clone();
            let row_map = row_map.clone();
            let interpret_row = interpret_row.clone();
            let get_sort_key = get_sort_key.clone();
            let ds_probe = data_source.clone();
            data_source.keyed_rows_signal_vec().for_each(move |diff| {
                let mut tree = tree.lock().unwrap();
                let mut key_index = key_index.lock().unwrap();
                let mut row_map = row_map.lock().unwrap();
                // Adopted orphans changed depth, so their interpreted widgets
                // (depth-dependent rule outcomes baked in) are stale —
                // re-interpret them at their post-adoption depth.
                // A row's stated parent is always canonical (see `rowset_depth`).
                let parent_key = |row: &holon_api::widget_spec::DataRow| {
                    extract_parent_id(row).map(|p| (p, holon_api::Occurrence::Canonical))
                };
                let reinterpret_adopted = |tree: &mut crate::mutable_tree::MutableTree,
                                           row_map: &HashMap<
                    holon_api::RowKey,
                    Arc<holon_api::widget_spec::DataRow>,
                >,
                                           adopted: Vec<holon_api::RowKey>,
                                           interpret_row: &InterpretRowFn,
                                           get_sort_key: &dyn Fn(
                    &holon_api::widget_spec::DataRow,
                ) -> String| {
                    for id in adopted {
                        let Some(row) = row_map.get(&id).cloned() else {
                            continue;
                        };
                        let parent = parent_key(&row);
                        let sk = get_sort_key(&row);
                        let depth = rowset_depth(row_map, &row);
                        let (w, ov) = interpret_row(row, depth, id.1.clone());
                        tree.update(&id, parent, sk, w, ov);
                    }
                };
                // `tree.remove` evicts the node's whole subtree, but upstream
                // only dropped the one key — the survivors are still live in
                // `row_map`/`key_index`. Re-insert them (DFS order, so parents
                // land before children); each becomes a root until its own
                // parent reappears, at which point `adopt_orphans` re-attaches
                // it. Skipping this desyncs `key_index` from upstream indices
                // and strands the survivors for `tree.update` to trip over.
                let reinstate_evicted = |tree: &mut crate::mutable_tree::MutableTree,
                                         row_map: &HashMap<
                    holon_api::RowKey,
                    Arc<holon_api::widget_spec::DataRow>,
                >,
                                         evicted: Vec<holon_api::RowKey>,
                                         interpret_row: &InterpretRowFn,
                                         get_sort_key: &dyn Fn(
                    &holon_api::widget_spec::DataRow,
                ) -> String| {
                    for id in evicted {
                        let row = row_map
                            .get(&id)
                            .unwrap_or_else(|| {
                                panic!(
                                    "tree evicted {id:?} as a descendant of a removed node, but \
                                     it is absent from row_map — the driver's row_map and the \
                                     tree have diverged"
                                )
                            })
                            .clone();
                        let parent = parent_key(&row);
                        let sk = get_sort_key(&row);
                        let depth = rowset_depth(row_map, &row);
                        let (w, ov) = interpret_row(row, depth, id.1.clone());
                        let adopted = tree.insert(id, parent, sk, w, ov);
                        for aid in adopted {
                            let arow = row_map
                                .get(&aid)
                                .unwrap_or_else(|| {
                                    panic!(
                                        "reinstated node adopted {aid:?}, which is absent from \
                                         row_map — the driver's row_map and the tree have diverged"
                                    )
                                })
                                .clone();
                            let aparent = parent_key(&arow);
                            let ask = get_sort_key(&arow);
                            let adepth = rowset_depth(row_map, &arow);
                            let (aw, aov) = interpret_row(arow, adepth, aid.1.clone());
                            tree.update(&aid, aparent, ask, aw, aov);
                        }
                    }
                };
                let diff_label = match &diff {
                    VecDiff::Replace { values } => format!("Replace({} rows)", values.len()),
                    VecDiff::InsertAt { index, value } => {
                        format!("InsertAt({index}, {})", value.0.0)
                    }
                    VecDiff::UpdateAt { index, value } => {
                        format!("UpdateAt({index}, {})", value.0.0)
                    }
                    VecDiff::RemoveAt { index } => format!("RemoveAt({index})"),
                    VecDiff::Push { value } => format!("Push({})", value.0.0),
                    VecDiff::Pop {} => "Pop".to_string(),
                    VecDiff::Clear {} => "Clear".to_string(),
                    VecDiff::Move {
                        old_index,
                        new_index,
                    } => {
                        format!("Move({old_index} -> {new_index})")
                    }
                };
                match diff {
                    VecDiff::Replace { values } => {
                        row_map.clear();
                        *key_index = values.iter().map(|(k, _)| k.clone()).collect();
                        // Fill row_map first: rowset_depth needs the FULL
                        // rowset to resolve parent chains regardless of row
                        // arrival order within the batch.
                        for (k, row) in &values {
                            row_map.insert(k.clone(), row.clone());
                        }
                        let entries: Vec<_> = values
                            .into_iter()
                            .map(|(k, row)| {
                                let parent = parent_key(&row);
                                let sk = get_sort_key(&row);
                                let depth = rowset_depth(&row_map, &row);
                                let (w, ov) = interpret_row(row, depth, k.1.clone());
                                (k, parent, sk, w, ov)
                            })
                            .collect();
                        tree.rebuild(entries);
                    }
                    VecDiff::InsertAt {
                        index,
                        value: (key, row),
                    } => {
                        row_map.insert(key.clone(), row.clone());
                        key_index.insert(index, key.clone());
                        let parent = parent_key(&row);
                        let sk = get_sort_key(&row);
                        let depth = rowset_depth(&row_map, &row);
                        let (w, ov) = interpret_row(row, depth, key.1.clone());
                        let adopted = tree.insert(key, parent, sk, w, ov);
                        reinterpret_adopted(
                            &mut tree,
                            &row_map,
                            adopted,
                            interpret_row.as_ref(),
                            get_sort_key.as_ref(),
                        );
                    }
                    VecDiff::UpdateAt {
                        index: _,
                        value: (key, row),
                    } => {
                        row_map.insert(key.clone(), row.clone());
                        let parent = parent_key(&row);
                        let sk = get_sort_key(&row);
                        let depth = rowset_depth(&row_map, &row);
                        let (w, ov) = interpret_row(row, depth, key.1.clone());
                        tree.update(&key, parent, sk, w, ov);
                    }
                    VecDiff::RemoveAt { index } => {
                        let key = key_index.remove(index);
                        row_map.remove(&key);
                        let evicted = tree.remove(&key);
                        reinstate_evicted(
                            &mut tree,
                            &row_map,
                            evicted,
                            interpret_row.as_ref(),
                            get_sort_key.as_ref(),
                        );
                    }
                    VecDiff::Push { value: (key, row) } => {
                        row_map.insert(key.clone(), row.clone());
                        key_index.push(key.clone());
                        let parent = parent_key(&row);
                        let sk = get_sort_key(&row);
                        let depth = rowset_depth(&row_map, &row);
                        let (w, ov) = interpret_row(row, depth, key.1.clone());
                        let adopted = tree.insert(key, parent, sk, w, ov);
                        reinterpret_adopted(
                            &mut tree,
                            &row_map,
                            adopted,
                            interpret_row.as_ref(),
                            get_sort_key.as_ref(),
                        );
                    }
                    VecDiff::Pop {} => {
                        if let Some(key) = key_index.pop() {
                            row_map.remove(&key);
                            let evicted = tree.remove(&key);
                            reinstate_evicted(
                                &mut tree,
                                &row_map,
                                evicted,
                                interpret_row.as_ref(),
                                get_sort_key.as_ref(),
                            );
                        }
                    }
                    VecDiff::Clear {} => {
                        key_index.clear();
                        row_map.clear();
                        tree.rebuild(vec![]);
                    }
                    VecDiff::Move { .. } => {}
                }
                // The tree renders exactly the rows the driver holds: both are
                // maintained under this one lock scope, so they must agree at
                // every diff boundary. A divergence is the dropped-row bug —
                // a row the panel was given that renders no node — and
                // recording it here names the diff that caused it.
                if crate::reactive::tree_desync::enabled() {
                    let tree_ids: std::collections::HashSet<EntityUri> =
                        tree.flat_ids().into_iter().map(|k| k.0).collect();
                    let row_ids: std::collections::HashSet<EntityUri> =
                        row_map.keys().map(|k| k.0.clone()).collect();
                    if tree_ids != row_ids {
                        crate::reactive::tree_desync::record(
                            &diff_label,
                            "row_map",
                            "tree",
                            &row_ids,
                            &tree_ids,
                        );
                    }
                    // The provider is the driver's only source of rows, so a
                    // row it holds that never reached `row_map` is a delivery
                    // gap in the signal-vec subscription — a different culprit
                    // from a tree that lost a row it was given.
                    let provider_ids: std::collections::HashSet<EntityUri> = ds_probe
                        .rows_snapshot()
                        .iter()
                        .filter_map(|r| holon_api::data_row_entity_uri(r))
                        .collect();
                    if provider_ids != row_ids {
                        crate::reactive::tree_desync::record(
                            &diff_label,
                            "provider",
                            "row_map",
                            &provider_ids,
                            &row_ids,
                        );
                    }
                }
                async {}
            })
        };

        // Focus driver: re-interpret affected rows when the focused block
        // changes. `pick_active_variant` reads `is_focused` from
        // `services.ui_state(id)` at interpret time; without this driver,
        // a focus change updates `UiState.focused_block` but rows keep
        // their stale variant — e.g. clicking a `rendered_text` block
        // never swaps it for the `editable_text` variant. Companion to
        // the existing `space_driver` / `profile_driver` pattern in
        // `create_flat_driver` (which re-interpret on viewport / profile
        // changes for the same reason).
        let focus_driver: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
            match focus_mutable {
                Some(m) => {
                    use futures_signals::signal::SignalExt;
                    let tree = tree.clone();
                    let row_map = row_map.clone();
                    let interpret_row = interpret_row.clone();
                    let get_sort_key = get_sort_key.clone();
                    let mut last_focus: Option<EntityUri> = m.get_cloned();
                    Box::pin(m.signal_cloned().for_each(move |new_focus| {
                        if new_focus != last_focus {
                            // Collect ids whose `is_focused` predicate
                            // flipped: the previously focused block (now
                            // false) and the newly focused block (now
                            // true). `last_focus`/`new_focus` are already
                            // canonical `EntityUri`s and the row_map is keyed
                            // by the same canonical id, so a bare-vs-schemed
                            // mismatch can't drop the lookup.
                            // Focus is still a bare `EntityUri` in this
                            // increment (Increment C widens it), so it flips
                            // only the CANONICAL occurrence of a block.
                            let mut affected: Vec<holon_api::RowKey> = Vec::new();
                            for uri in [last_focus.as_ref(), new_focus.as_ref()]
                                .into_iter()
                                .flatten()
                            {
                                let key = (uri.clone(), holon_api::Occurrence::Canonical);
                                if !affected.contains(&key) {
                                    affected.push(key);
                                }
                            }
                            // Snapshot rows under the row_map lock, then
                            // release before taking the tree lock to keep
                            // the lock order consistent with the data
                            // driver (which takes tree → key_index →
                            // row_map; we take row_map then tree, but the
                            // row_map lock is released first).
                            let updates: Vec<crate::mutable_tree::TreeEntry> = {
                                let rm = row_map.lock().unwrap();
                                affected
                                    .iter()
                                    .filter_map(|key| {
                                        rm.get(key).cloned().map(|row| {
                                            let parent = extract_parent_id(&row)
                                                .map(|p| (p, holon_api::Occurrence::Canonical));
                                            let sk = get_sort_key(&row);
                                            let depth = rowset_depth(&rm, &row);
                                            let (w, ov) = interpret_row(row, depth, key.1.clone());
                                            (key.clone(), parent, sk, w, ov)
                                        })
                                    })
                                    .collect()
                            };
                            if !updates.is_empty() {
                                let mut t = tree.lock().unwrap();
                                for (id, parent, sk, w, ov) in updates {
                                    t.update(&id, parent, sk, w, ov);
                                }
                            }
                            last_focus = new_focus;
                        }
                        async {}
                    }))
                }
                None => Box::pin(std::future::pending()),
            };

        Box::pin(async move {
            futures::future::join(data_driver, focus_driver).await;
        })
    }

    /// Flat collection driver: Table/List/Columns.
    ///
    /// Handles fine-grained VecDiff events from the data source incrementally
    /// (UpdateAt → set_cloned, InsertAt → insert_cloned, etc.) instead of
    /// rebuilding the entire MutableVec on every CDC change.
    ///
    /// A separate space driver triggers a full re-interpret when the
    /// container-query allocation changes (viewport resize, keyboard).
    ///
    /// With `sort_key`, incremental insert/remove/update falls back to a
    /// full rebuild since the sort position may change.
    fn create_flat_driver(
        &self,
        data_source: &Arc<dyn ReactiveRowProvider>,
        item_template: &RenderExpr,
        template_mutable: &Mutable<RenderExpr>,
        space: &Mutable<Option<AvailableSpace>>,
        child_space_fn: Option<Arc<ChildSpaceFn>>,
        services: Arc<dyn crate::reactive::BuilderServices>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        let sort_key = match &self.inner {
            ReactiveViewInner::Collection { sort_key, .. } => sort_key.clone(),
            _ => None,
        };
        let config_rules: Vec<holon_api::render_types::RuleSpec> = match &self.inner {
            ReactiveViewInner::Collection { rules, .. } => rules.clone(),
            _ => Vec::new(),
        };
        let has_sort = sort_key.is_some();

        let target = self.items.clone();
        let space_handle = space.clone();

        // Shared entries for the two concurrent drivers. Keyed by the widened
        // `RowKey`; the tie-break in `full_rebuild` compares the whole key. The
        // flat driver renders canonical rows only (display placement targets the
        // tree path first), so nodes keep the default `Canonical` occurrence.
        let entries: RowEntries = Arc::new(Mutex::new(Vec::new()));

        // Self-interpretation closure: captures services + space, recomputes
        // props from (expr, data) without creating a fresh ReactiveViewModel.
        //
        // For props_only widgets (text, badge, icon, etc.) we take a fast path
        // that resolves args and extracts props directly, bypassing the full
        // `services.interpret()` pipeline.
        let node_interpret_fn: InterpretFn = {
            let svc = services.clone();
            let space = space_handle.clone();
            let ds = data_source.clone();
            let fast_widget = match item_template {
                RenderExpr::FunctionCall { name, .. }
                    if crate::render_interpreter::is_props_only_widget(name) =>
                {
                    Some(name.clone())
                }
                _ => None,
            };
            Arc::new(move |expr, data| {
                let parent_space = space.get_cloned();
                if let Some(fast_widget) = fast_widget.as_ref() {
                    return crate::render_interpreter::resolve_props(
                        fast_widget,
                        expr,
                        data,
                        svc.as_ref(),
                        parent_space,
                    );
                }
                let handle =
                    holon_api::data_row_entity_uri(data).and_then(|uri| ds.row_mutable(&uri));
                let ctx = row_render_context(data.clone(), handle, svc.as_ref(), parent_space);
                let fresh = svc.interpret(expr, &ctx);
                fresh.props.get_cloned()
            })
        };

        // Helper: interpret a row and attach the self-interpretation closure.
        // Each interpreted node also carries its source row in `data` so
        // downstream renderers (e.g. the GPUI board's per-card lane lookup)
        // can read row columns without going back through context. Builders
        // that don't read `data` are unaffected — `with_entity` just sets a
        // ReadOnlyMutable cell that's never observed.
        let advice_tmpl = advice_readonly_template();
        let interpret_and_attach = {
            let svc = services.clone();
            let nif = node_interpret_fn.clone();
            let ds = data_source.clone();
            let rules = config_rules;
            let advice_tmpl = advice_tmpl.clone();
            move |tmpl: &RenderExpr,
                  row: Arc<holon_api::widget_spec::DataRow>,
                  child_space: Option<AvailableSpace>,
                  occurrence: holon_api::Occurrence|
                  -> Arc<ReactiveViewModel> {
                let handle =
                    holon_api::data_row_entity_uri(&row).and_then(|uri| ds.row_mutable(&uri));
                let ctx = row_render_context(row.clone(), handle, svc.as_ref(), child_space);
                // A woven advice row (`Occurrence::Placed`) renders READ-ONLY
                // (ADR 0021/0022): the dismiss/click template, never the
                // collection's editable `item_template`, and skips rules.
                let is_advice = occurrence != holon_api::Occurrence::Canonical;
                let template = if is_advice { &advice_tmpl } else { tmpl };
                let active_rules: &[holon_api::render_types::RuleSpec] =
                    if is_advice { &[] } else { &rules };
                // FU-6: rules: per-row. Streaming has no count/is_last so the
                // positional context is empty (column-only predicates fire).
                let positional = std::collections::HashMap::new();
                let svc_for_interpret = svc.clone();
                let (mut node, _) = crate::row_pipeline::apply_rules_and_interpret_with_ctx(
                    ctx,
                    template,
                    active_rules,
                    &row,
                    positional,
                    move |expr, c| svc_for_interpret.interpret(expr, c),
                );
                node.interpret_fn = Some(nif.clone());
                node.occurrence = occurrence;
                Arc::new(node)
            }
        };

        // Full rebuild: sort entries, interpret all, replace target.
        let full_rebuild = {
            let entries = entries.clone();
            let sort_key = sort_key.clone();
            let target = target.clone();
            let tmpl = item_template.clone();
            let space = space_handle.clone();
            let csf = child_space_fn.clone();
            let interpret = interpret_and_attach.clone();
            Arc::new(move || {
                let mut lock = entries.lock().unwrap();
                if let Some(ref spec) = sort_key {
                    // Honor the `-`-prefixed DESCENDING convention (e.g.
                    // `sortkey: "-content"` for the newest-first journal feed).
                    // The raw spec is a sort DIRECTIVE, not a column name — using
                    // it verbatim as `row.get("-content")` finds no column and
                    // silently degrades to the `ka.cmp(kb)` arrival-order tie
                    // (dogfood #6 row 34: feed rendered arrival-order). Mirrors
                    // the static `sorted_rows` + tree-driver `parse_sort_key`.
                    let (col, descending) = holon_api::render_eval::parse_sort_key(spec);
                    lock.sort_by(|(ka, a), (kb, b)| {
                        let ord = holon_api::render_eval::cmp_values(a.get(col), b.get(col));
                        let ord = if descending { ord.reverse() } else { ord };
                        ord.then_with(|| ka.cmp(kb))
                    });
                }
                let parent_space = space.get_cloned();
                let count = lock.len();
                let child_space = match (parent_space, csf.as_ref()) {
                    (Some(p), Some(f)) => Some(f(p, count)),
                    _ => parent_space,
                };
                let items: Vec<Arc<ReactiveViewModel>> = lock
                    .iter()
                    .map(|(k, row)| interpret(&tmpl, row.clone(), child_space, k.1.clone()))
                    .collect();
                tracing::trace!(
                    "[ReactiveView::flat_driver] rebuilt, len={}, child_space={:?}",
                    items.len(),
                    child_space,
                );
                drop(lock);
                target.lock_mut().replace_cloned(items);
            })
        };

        // Data driver: fine-grained VecDiff from the reactive row set.
        let data_driver = {
            let entries = entries.clone();
            let target = target.clone();
            let tmpl = item_template.clone();
            let space = space_handle.clone();
            let csf = child_space_fn.clone();
            let rebuild = full_rebuild.clone();
            let interpret = interpret_and_attach;

            data_source.keyed_rows_signal_vec().for_each(move |diff| {
                match diff {
                    VecDiff::Replace { values } => {
                        *entries.lock().unwrap() = values;
                        rebuild();
                    }
                    VecDiff::UpdateAt {
                        index,
                        value: (key, row),
                    } => {
                        let occ = key.1.clone();
                        entries.lock().unwrap()[index] = (key, row.clone());
                        if has_sort || csf.is_some() {
                            // Sort key may have changed → reorder.
                            rebuild();
                        } else {
                            // Re-interpret the row: variant selection can
                            // depend on row data (computed fields like
                            // `has_task_state`), so a data update can flip
                            // which variant is active and change the widget
                            // kind. Per-row signal cells handle leaf-level
                            // prop updates, but they can't switch the
                            // active variant — only re-interpret can.
                            let parent_space = space.get_cloned();
                            target
                                .lock_mut()
                                .set_cloned(index, interpret(&tmpl, row, parent_space, occ));
                        }
                    }
                    VecDiff::InsertAt {
                        index,
                        value: (key, row),
                    } => {
                        let occ = key.1.clone();
                        entries.lock().unwrap().insert(index, (key, row.clone()));
                        if has_sort || csf.is_some() {
                            rebuild();
                        } else {
                            let parent_space = space.get_cloned();
                            target
                                .lock_mut()
                                .insert_cloned(index, interpret(&tmpl, row, parent_space, occ));
                        }
                    }
                    VecDiff::RemoveAt { index } => {
                        entries.lock().unwrap().remove(index);
                        if has_sort || csf.is_some() {
                            rebuild();
                        } else {
                            target.lock_mut().remove(index);
                        }
                    }
                    VecDiff::Push { value: (key, row) } => {
                        let occ = key.1.clone();
                        entries.lock().unwrap().push((key, row.clone()));
                        if has_sort || csf.is_some() {
                            rebuild();
                        } else {
                            let parent_space = space.get_cloned();
                            target
                                .lock_mut()
                                .push_cloned(interpret(&tmpl, row, parent_space, occ));
                        }
                    }
                    VecDiff::Pop {} => {
                        entries.lock().unwrap().pop();
                        if has_sort || csf.is_some() {
                            rebuild();
                        } else {
                            target.lock_mut().pop();
                        }
                    }
                    VecDiff::Clear {} => {
                        entries.lock().unwrap().clear();
                        target.lock_mut().clear();
                    }
                    VecDiff::Move { .. } => {}
                }
                async {}
            })
        };

        // Space driver: full re-interpret when viewport/container-query changes.
        let space_driver = {
            let rebuild = full_rebuild.clone();
            let entries = entries.clone();
            let mut first = true;
            space_handle.signal().for_each(move |_| {
                if first {
                    first = false;
                } else if !entries.lock().unwrap().is_empty() {
                    rebuild();
                }
                async {}
            })
        };

        // Profile driver: full re-interpret when the profile cache changes.
        // `render_entity` resolves the per-row profile inside `interpret`,
        // but data-driven `interpret_row` only fires on row changes — so an
        // edit to an entity_profile_yaml block otherwise leaves rows of
        // OTHER entities frozen at the pre-mutation profile.
        let profile_driver = {
            let rebuild = full_rebuild.clone();
            let entries = entries.clone();
            let mut first = true;
            services
                .profile_signal()
                .signal_cloned()
                .for_each(move |_cache| {
                    if first {
                        first = false;
                    } else if !entries.lock().unwrap().is_empty() {
                        rebuild();
                    }
                    async {}
                })
        };

        // Focus driver: re-interpret items when the focused block changes.
        // `pick_active_variant` reads `is_focused` from `services.ui_state(id)`
        // at interpret time; without this driver, a focus change leaves all
        // rows on their stale variant — e.g. clicking a `rendered_text` block
        // never swaps it for the `editable_text` variant. Companion to the
        // `space_driver` / `profile_driver` pattern above.
        let focus_driver: std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> =
            match services.focused_block_mutable() {
                Some(m) => {
                    use futures_signals::signal::SignalExt;
                    let rebuild = full_rebuild.clone();
                    let entries = entries.clone();
                    let mut last_focus: Option<EntityUri> = m.get_cloned();
                    Box::pin(m.signal_cloned().for_each(move |new_focus| {
                        if new_focus != last_focus && !entries.lock().unwrap().is_empty() {
                            rebuild();
                        }
                        last_focus = new_focus;
                        async {}
                    }))
                }
                None => Box::pin(std::future::pending()),
            };

        // Template driver: re-interpret all items' props when the shared
        // template Mutable changes. Items are updated in place — no new
        // Arc<ReactiveViewModel>, no MutableVec signals. GPUI's props
        // watchers detect the changes and call cx.notify().
        let template_driver = {
            let target = target.clone();
            let interpret_fn = node_interpret_fn;
            let mut first = true;
            template_mutable.signal_cloned().for_each(move |new_tmpl| {
                if first {
                    first = false;
                } else {
                    let items = target.lock_ref();
                    for item in items.iter() {
                        let new_props = interpret_fn(&new_tmpl, &item.data.get_cloned());
                        item.props.set(new_props);
                    }
                }
                async {}
            })
        };

        Box::pin(async move {
            futures::future::join(
                futures::future::join(
                    futures::future::join(data_driver, space_driver),
                    futures::future::join(template_driver, profile_driver),
                ),
                focus_driver,
            )
            .await;
        })
    }

    /// Grouped driver: ONE subscription that owns lane partitioning.
    ///
    /// Subscribes to the upstream's `keyed_rows_signal_vec` once and, on
    /// each `VecDiff`, fully rebuilds the lane list (board → lanes → cards).
    /// Each rebuild atomically replaces `self.items` — there is no window
    /// where two lanes can be observed in inconsistent post-update states,
    /// which was the failure mode of N independent per-lane filtered
    /// providers (a row in transit could appear in BOTH source and target
    /// lanes for one frame).
    ///
    /// Cost: every row event re-interprets all cards. For typical kanban
    /// sizes (≤ 1k rows / ~5 lanes) this is fine; if it ever isn't, the
    /// rebuild can be replaced with fine-grained per-lane diff emission
    /// without changing the public surface.
    #[allow(clippy::too_many_arguments)]
    fn create_grouped_driver(
        &self,
        data_source: &Arc<dyn ReactiveRowProvider>,
        item_template: &RenderExpr,
        rules: &[holon_api::render_types::RuleSpec],
        lane_field: &str,
        lane_label_default: &str,
        lane_order: &[String],
        sort_key: &Option<String>,
        space: &Mutable<Option<AvailableSpace>>,
        services: Arc<dyn crate::reactive::BuilderServices>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        // Snapshot config that the closure needs.
        let target = self.items.clone();
        let space_handle = space.clone();
        let lane_field = lane_field.to_string();
        let lane_label_default = lane_label_default.to_string();
        let lane_order: Vec<String> = lane_order.to_vec();
        let sort_key = sort_key.clone();
        let tmpl = item_template.clone();
        let ds = data_source.clone();
        let svc = services;
        let rules: Vec<holon_api::render_types::RuleSpec> = rules.to_vec();

        // Per-row entry tracking. Mirrors flat_driver's `entries` shape so
        // we can rebuild lanes deterministically on every event.
        let entries: RowEntries = Arc::new(Mutex::new(Vec::new()));

        let lane_field_for_partition = lane_field.clone();
        let label_default_for_partition = lane_label_default.clone();
        let lane_order_for_partition = lane_order.clone();

        let rebuild = {
            let entries = entries.clone();
            let target = target.clone();
            let svc = svc.clone();
            let ds = ds.clone();
            let tmpl = tmpl.clone();
            let space_handle = space_handle.clone();
            let sort_key = sort_key.clone();
            let lane_field = lane_field_for_partition;
            let lane_label_default = label_default_for_partition;
            let lane_order = lane_order_for_partition;
            let rules = rules.clone();
            Arc::new(move || {
                let lock = entries.lock().unwrap();
                let parent_space = space_handle.get_cloned();

                // Bucket entries by lane_value (raw → label-substitute).
                let mut buckets: HashMap<String, Vec<Arc<holon_api::widget_spec::DataRow>>> =
                    HashMap::new();
                for (_key, row) in lock.iter() {
                    let raw = row
                        .get(&lane_field)
                        .and_then(|v| v.as_string())
                        .unwrap_or("");
                    let title = if raw.is_empty() {
                        lane_label_default.clone()
                    } else {
                        raw.to_string()
                    };
                    buckets.entry(title).or_default().push(row.clone());
                }

                // Sort within each bucket by sort_key when configured. The
                // spec is a sort DIRECTIVE, not a column name — a `-` prefix
                // means DESCENDING (mirrors the flat driver's `full_rebuild`
                // and the tree driver's `get_sort_key`); using it verbatim
                // finds no column and degrades to the id tie-break.
                if let Some(ref spec) = sort_key {
                    let (key, descending) = holon_api::render_eval::parse_sort_key(spec);
                    for rows in buckets.values_mut() {
                        rows.sort_by(|a, b| {
                            let ord = holon_api::render_eval::cmp_values(a.get(key), b.get(key));
                            let ord = if descending { ord.reverse() } else { ord };
                            ord.then_with(|| {
                                let id_a = a.get("id").and_then(|v| v.as_string()).unwrap_or("");
                                let id_b = b.get("id").and_then(|v| v.as_string()).unwrap_or("");
                                id_a.cmp(id_b)
                            })
                        });
                    }
                }

                // Order lanes: caller-preferred first (only those present),
                // remaining lex.
                let mut ordered: Vec<String> = Vec::new();
                for k in &lane_order {
                    if buckets.contains_key(k) {
                        ordered.push(k.clone());
                    }
                }
                let mut remaining: Vec<String> = buckets
                    .keys()
                    .filter(|k| !ordered.contains(k))
                    .cloned()
                    .collect();
                remaining.sort();
                ordered.extend(remaining);

                // Build lane VMs. Each lane is a `board_lane` with cards as
                // its `children`. The static-children shape works because
                // we replace the whole lane list atomically per event —
                // GPUI never sees mid-update state.
                let mut lane_vms: Vec<Arc<ReactiveViewModel>> = Vec::with_capacity(ordered.len());
                for title in ordered {
                    let rows = buckets.remove(&title).unwrap_or_default();
                    let cards: Vec<Arc<ReactiveViewModel>> = rows
                        .into_iter()
                        .map(|row| {
                            let handle = holon_api::data_row_entity_uri(&row)
                                .and_then(|uri| ds.row_mutable(&uri));
                            let ctx =
                                row_render_context(row.clone(), handle, svc.as_ref(), parent_space);
                            // FU-6: per-card rules: apply. Lane-position
                            // (which lane this card is in) is available via
                            // `lane` positional, alongside the card's own
                            // columns. Predicate authors can write
                            // `eq("lane", "Done")` to match cards in
                            // specific lanes.
                            let positional = std::collections::HashMap::from([(
                                "lane".to_string(),
                                holon_api::Value::String(title.clone()),
                            )]);
                            let svc_for_interpret = svc.clone();
                            let (node, _) = crate::row_pipeline::apply_rules_and_interpret_with_ctx(
                                ctx,
                                &tmpl,
                                &rules,
                                &row,
                                positional,
                                move |expr, c| svc_for_interpret.interpret(expr, c),
                            );
                            Arc::new(node)
                        })
                        .collect();

                    let mut lane_props = HashMap::new();
                    lane_props.insert("title".to_string(), holon_api::Value::String(title.clone()));
                    let lane_vm = ReactiveViewModel {
                        children: cards,
                        ..ReactiveViewModel::from_widget("board_lane", lane_props)
                    };
                    lane_vms.push(Arc::new(lane_vm));
                }

                drop(lock);
                target.lock_mut().replace_cloned(lane_vms);
            })
        };

        let initial_rebuild = rebuild.clone();

        let data_driver = ds.keyed_rows_signal_vec().for_each(move |diff| {
            match diff {
                VecDiff::Replace { values } => {
                    *entries.lock().unwrap() = values;
                }
                VecDiff::InsertAt {
                    index,
                    value: (key, row),
                } => {
                    entries.lock().unwrap().insert(index, (key, row));
                }
                VecDiff::UpdateAt {
                    index,
                    value: (key, row),
                } => {
                    entries.lock().unwrap()[index] = (key, row);
                }
                VecDiff::RemoveAt { index } => {
                    entries.lock().unwrap().remove(index);
                }
                VecDiff::Push { value: (key, row) } => {
                    entries.lock().unwrap().push((key, row));
                }
                VecDiff::Pop {} => {
                    entries.lock().unwrap().pop();
                }
                VecDiff::Move {
                    old_index,
                    new_index,
                } => {
                    let mut lock = entries.lock().unwrap();
                    let entry = lock.remove(old_index);
                    lock.insert(new_index, entry);
                }
                VecDiff::Clear {} => {
                    entries.lock().unwrap().clear();
                }
            }
            rebuild();
            futures::future::ready(())
        });

        // Trigger an initial rebuild so the first render after `start()`
        // sees lanes (driver futures don't poll until the runtime ticks).
        initial_rebuild();
        Box::pin(data_driver)
    }

    /// Partitioned driver for heterogeneous positional children.
    ///
    /// Watches `parent_space` and re-interprets children when the parent's
    /// allocation changes. Fixed children (drawers, spacers) keep their
    /// declared width; Flex children get a proportional share of the
    /// remaining space.
    fn create_partitioned_driver(
        &self,
        children_config: &[(RenderExpr, LayoutHint)],
        parent_space: &Mutable<Option<AvailableSpace>>,
        gap: f32,
        services: Arc<dyn crate::reactive::BuilderServices>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        let target = self.items.clone();
        let config: Vec<(RenderExpr, LayoutHint)> = children_config.to_vec();
        let space_signal = parent_space.signal();

        let driver = space_signal.for_each(move |parent_space| {
            let new_items: Vec<Arc<ReactiveViewModel>> = match parent_space {
                Some(parent) => {
                    let hints: Vec<LayoutHint> = config.iter().map(|(_, h)| *h).collect();

                    let flow_count = hints
                        .iter()
                        .filter(|h| !matches!(h, LayoutHint::Fixed { px } if *px == 0.0))
                        .count();
                    let gap_total = gap * flow_count.saturating_sub(1) as f32;

                    let fixed_total: f32 = hints
                        .iter()
                        .filter_map(|h| match h {
                            LayoutHint::Fixed { px } => Some(px),
                            _ => None,
                        })
                        .sum();
                    let flex_weight_total: f32 = hints
                        .iter()
                        .filter_map(|h| match h {
                            LayoutHint::Flex { weight } => Some(weight),
                            _ => None,
                        })
                        .sum();

                    let remaining = (parent.width_px - fixed_total - gap_total).max(0.0);

                    config
                        .iter()
                        .map(|(expr, hint)| {
                            let child_space = match *hint {
                                LayoutHint::Fixed { px } => AvailableSpace {
                                    width_px: px,
                                    width_physical_px: px * parent.scale_factor,
                                    ..parent
                                },
                                LayoutHint::Flex { weight } => {
                                    let w =
                                        remaining * weight / flex_weight_total.max(f32::EPSILON);
                                    AvailableSpace {
                                        width_px: w,
                                        width_physical_px: w * parent.scale_factor,
                                        ..parent
                                    }
                                }
                            };
                            let ctx = crate::RenderContext {
                                available_space: Some(child_space),
                                ..Default::default()
                            };
                            Arc::new(services.interpret(expr, &ctx))
                        })
                        .collect()
                }
                None => config
                    .iter()
                    .map(|(expr, _)| {
                        let ctx = crate::RenderContext::default();
                        Arc::new(services.interpret(expr, &ctx))
                    })
                    .collect(),
            };
            tracing::trace!(
                "[ReactiveView::partitioned_driver] rebuilt, len={}, parent_space={:?}",
                new_items.len(),
                parent_space,
            );
            target.lock_mut().replace_cloned(new_items);
            async {}
        });
        Box::pin(driver)
    }

    /// Snapshot into a static LazyChildren list.
    pub fn snapshot(&self) -> crate::view_model::LazyChildren {
        let items: Vec<ViewModel> = self
            .items
            .lock_ref()
            .iter()
            .map(|rvm| rvm.snapshot())
            .collect();
        crate::view_model::LazyChildren::fully_materialized(items)
    }

    /// Snapshot with resolved LiveBlock nodes.
    pub fn snapshot_resolved(
        &self,
        resolve_block: &dyn Fn(&EntityUri) -> ViewModel,
    ) -> crate::view_model::LazyChildren {
        let items: Vec<ViewModel> = self
            .items
            .lock_ref()
            .iter()
            .map(|rvm| rvm.snapshot_resolved(resolve_block))
            .collect();
        crate::view_model::LazyChildren::fully_materialized(items)
    }
}

impl Drop for ReactiveView {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Walk a ReactiveViewModel tree and start all ReactiveViews found within.
pub fn start_reactive_views(
    tree: &ReactiveViewModel,
    services: &Arc<dyn crate::reactive::BuilderServices>,
    rt: &tokio::runtime::Handle,
) {
    // Start this node's collection if it has one
    if let Some(ref view) = tree.collection {
        view.start(services.clone(), rt);
    }

    // Walk children recursively
    walk_children(tree, &|child| {
        start_reactive_views(child, services, rt);
    });
}

/// Walk immediate children of a ReactiveViewModel node.
fn walk_children(node: &ReactiveViewModel, f: &dyn Fn(&ReactiveViewModel)) {
    // Static children
    for child in &node.children {
        f(child);
    }

    // Reactive collection children
    if let Some(ref view) = node.collection {
        let items: Vec<Arc<ReactiveViewModel>> = view.items.lock_ref().iter().cloned().collect();
        for item in &items {
            f(item);
        }
    }

    // Slot content
    if let Some(ref slot) = node.slot {
        let guard = slot.content.lock_ref();
        f(&guard);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use holon_api::ChangeOrigin;
    use holon_api::Value;
    use holon_api::widget_spec::DataRow;
    use holon_api::widget_spec::EnrichedRow;

    use super::*;
    use crate::reactive::ReactiveRowSet;
    use crate::reactive::StubBuilderServices;
    use crate::reactive_view_model::CollectionVariant;

    fn make_row(id: &str, content: &str) -> DataRow {
        let mut row = DataRow::new();
        row.insert("id".to_string(), Value::String(id.to_string()));
        row.insert("content".to_string(), Value::String(content.to_string()));
        row
    }

    /// A minimal `ReactiveRowProvider` returning a fixed row set — for tests
    /// that need `inner` to hold specific rows (e.g. so the creation slot can
    /// resolve its focus-root parent).
    struct FixedRows(Vec<Arc<DataRow>>);
    impl ReactiveRowProvider for FixedRows {
        fn rows_snapshot(&self) -> Vec<Arc<DataRow>> {
            self.0.clone()
        }
        fn rows_signal_vec(
            &self,
        ) -> std::pin::Pin<
            Box<dyn futures_signals::signal_vec::SignalVec<Item = Arc<DataRow>> + Send>,
        > {
            Box::pin(futures_signals::signal_vec::always(self.0.clone()))
        }
        fn keyed_rows_signal_vec(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn futures_signals::signal_vec::SignalVec<Item = (holon_api::RowKey, Arc<DataRow>)>
                    + Send,
            >,
        > {
            let keyed: Vec<_> = self
                .0
                .iter()
                .map(|r| {
                    (
                        (
                            holon_api::data_row_entity_uri(r).expect("row has id"),
                            holon_api::Occurrence::Canonical,
                        ),
                        r.clone(),
                    )
                })
                .collect();
            Box::pin(futures_signals::signal_vec::always(keyed))
        }
        fn cache_identity(&self) -> u64 {
            0
        }
    }

    fn row_with_parent(id: &str, parent: &str) -> Arc<DataRow> {
        let mut r = DataRow::new();
        r.insert("id".to_string(), Value::String(id.to_string()));
        r.insert("parent_id".to_string(), Value::String(parent.to_string()));
        Arc::new(r)
    }

    fn appended_row_count(inner: Vec<Arc<DataRow>>, slot: &VirtualChildSlot) -> usize {
        let inner_len = inner.len();
        let provider = AppendedRowsProvider::creation_slot(Arc::new(FixedRows(inner)), slot);
        provider.rows_snapshot().len() - inner_len
    }

    /// The declared-schema defaults (Null-seeded, incl. `id`/`parent_id`/
    /// `sort_key`) must never overwrite the slot's structural identity.
    #[test]
    fn creation_slot_structural_columns_win_over_declared_defaults() {
        let parent = holon_api::EntityUri::block("journals");
        let defaults = HashMap::from([
            ("id".to_string(), holon_api::Value::Null),
            ("parent_id".to_string(), holon_api::Value::Null),
            ("sort_key".to_string(), holon_api::Value::Null),
            ("source_language".to_string(), holon_api::Value::Null),
        ]);
        let (_key, row) = creation_slot_keyed_row(&parent, &defaults);
        assert_eq!(
            row.get("id").and_then(|v| v.as_string()),
            Some("block:__virtual:journals")
        );
        assert_eq!(
            row.get("parent_id").and_then(|v| v.as_string()),
            Some("block:journals")
        );
        assert_eq!(
            row.get("sort_key").and_then(|v| v.as_string()),
            Some("\u{10FFFF}")
        );
        assert!(row.contains_key("source_language"));
    }

    /// BugFunnel #61: the Pages sidebar is a read-only navigation list — a flat
    /// forest of top-level pages each parented to the `no_parent` sentinel —
    /// that does NOT opt in (`allow_root_creation = false`). Its rendered
    /// rowset must equal its backing rows: NO virtual
    /// `sentinel:__virtual:no_parent` row is appended.
    #[test]
    fn sidebar_forest_without_optin_appends_no_creation_slot() {
        let sentinel = holon_api::EntityUri::no_parent();
        let inner = vec![
            row_with_parent("block:pageA", sentinel.as_str()),
            row_with_parent("block:pageB", sentinel.as_str()),
        ];
        let slot = VirtualChildSlot {
            defaults: HashMap::new(),
            parent_id: holon_api::EntityUri::block("journals"),
            allow_root_creation: false,
        };
        assert_eq!(appended_row_count(inner, &slot), 0);
    }

    /// BugFunnel #67: a NESTED-PAGE forest reaches the SAME read-only sidebar
    /// render path. A subdir page-file (`Journals/2026-07-10.org`) roots the
    /// date page under the `journals` folder-page, which is not itself a
    /// `Page`, so the `WHERE tag='Page'` rowset shows a mix: top-level
    /// pages at the `no_parent` sentinel PLUS a date page whose parent is
    /// filtered out. This USED TO PANIC at boot ("disjoint root rows").
    /// Read-only (`allow_root_creation = false`) must resolve to NO slot
    /// without panicking, and both pages stay in the rendered rowset.
    #[test]
    fn nested_page_sidebar_forest_boots_without_panic() {
        let sentinel = holon_api::EntityUri::no_parent();
        let inner = vec![
            row_with_parent("block:pageA", sentinel.as_str()),
            row_with_parent("block:journal-2026-07-10", "block:journals"), // parent filtered out
        ];
        let slot = VirtualChildSlot {
            defaults: HashMap::new(),
            parent_id: holon_api::EntityUri::block("journals-sidebar"),
            allow_root_creation: false,
        };
        // No panic; no creation slot appended.
        let provider =
            AppendedRowsProvider::creation_slot(Arc::new(FixedRows(inner.clone())), &slot);
        let snap = provider.rows_snapshot();
        assert_eq!(snap.len(), inner.len(), "no virtual row appended");
        // Both real pages survive into the rendered rowset.
        let ids: Vec<Option<&str>> = snap
            .iter()
            .map(|r| r.get("id").and_then(|v| v.as_string()))
            .collect();
        assert!(ids.contains(&Some("block:pageA")));
        assert!(ids.contains(&Some("block:journal-2026-07-10")));
    }

    /// The main panel is a single focus-rooted tree; its creation slot is
    /// PRESERVED regardless of the `allow_root_creation` opt-in — the fix must
    /// not regress it (BugFunnel #61).
    #[test]
    fn main_panel_single_root_still_appends_creation_slot() {
        let inner = vec![
            row_with_parent("block:page", "block:root-layout"), // focus root
            row_with_parent("block:c1", "block:page"),
        ];
        let slot = VirtualChildSlot {
            defaults: HashMap::new(),
            parent_id: holon_api::EntityUri::block("default-main-panel"),
            allow_root_creation: false,
        };
        assert_eq!(appended_row_count(inner, &slot), 1);
        // The appended row is a creation placeholder parented to the focus root.
        let provider = AppendedRowsProvider::creation_slot(
            Arc::new(FixedRows(vec![row_with_parent(
                "block:page",
                "block:root-layout",
            )])),
            &slot,
        );
        let snap = provider.rows_snapshot();
        let slot_row = snap
            .iter()
            .find(|r| crate::row_origin::RowOrigin::from_row(r).is_creation_placeholder())
            .expect("main-panel creation slot present");
        assert_eq!(
            slot_row.get("parent_id").and_then(|v| v.as_string()),
            Some("block:page")
        );
    }

    /// An editable top-level-pages list that DOES opt in (`creation_slot:
    /// true`) keeps the "create a new top-level page" slot at the
    /// `no_parent` sentinel.
    #[test]
    fn forest_with_optin_appends_root_creation_slot() {
        let sentinel = holon_api::EntityUri::no_parent();
        let inner = vec![
            row_with_parent("block:pageA", sentinel.as_str()),
            row_with_parent("block:pageB", sentinel.as_str()),
        ];
        let slot = VirtualChildSlot {
            defaults: HashMap::new(),
            parent_id: holon_api::EntityUri::block("journals"),
            allow_root_creation: true,
        };
        assert_eq!(appended_row_count(inner, &slot), 1);
    }

    fn enriched(row: DataRow) -> EnrichedRow {
        EnrichedRow::from_raw(row, |_| HashMap::new())
    }

    fn remote_origin() -> ChangeOrigin {
        ChangeOrigin::Remote {
            operation_id: None,
            trace_id: None,
        }
    }

    /// Simulate production sibling-key minting. Each position applies a
    /// `gen_key_between` against the current neighbours — exactly the call
    /// production makes when a real child is inserted between two siblings.
    /// Repeated insertions at the same slot grow longer hex keys ("7F80", …),
    /// tail insertions mint after-keys, giving a diverse, prod-faithful set.
    fn mint_fractional_keys(positions: &[usize]) -> Vec<String> {
        use holon_core::fractional_index::gen_key_between;
        let mut keys: Vec<String> = Vec::new();
        for &raw_pos in positions {
            let pos = raw_pos % (keys.len() + 1);
            let prev = if pos == 0 {
                None
            } else {
                Some(keys[pos - 1].as_str())
            };
            let next = keys.get(pos).map(|s| s.as_str());
            let k = gen_key_between(prev, next).expect("gen_key_between mints a valid key"); // ALLOW(order_minting): test-only reproduction of the order owner's sibling-key
            // minting to prove the virtual-slot sort contract
            keys.insert(pos, k);
        }
        keys
    }

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config {
            failure_persistence: None,
            ..proptest::test_runner::Config::default()
        })]

        /// Sort-order contract between the virtual-child creation slot and real
        /// `FractionalIndex` sibling keys.
        ///
        /// `AppendedRowsProvider` (creation-slot case) appends its creation row
        /// with a sentinel `sort_key` intending "sorts last". But the
        /// tree driver (`get_sort_key` → `mutable_tree::SortedChild`) orders
        /// siblings by the *string* produced by
        /// `holon_api::render_eval::sort_value`, which encodes a float via its
        /// IEEE-754 bits as a 20-digit decimal (f64::MAX → "18442240474082181119",
        /// leading '1'). Real rows carry raw hex `FractionalIndex` strings
        /// ("A0", "7F80", "80", …) whose leading bytes are '2'..'F'. Since
        /// '1' < '2'..'F' lexicographically, the virtual row sorts FIRST, not
        /// last — the creation slot jumps to the top of every child list.
        ///
        /// This test asserts the intended contract (virtual sorts AFTER any
        /// real key) and is RED until the sentinel-key fix lands.
        /// See devlog/2026-07-05-011500-gpui-dogfood-triage.md issue #4.
        #[test]
        fn virtual_child_sort_key_sorts_after_any_fractional_index_key(
            positions in proptest::collection::vec(0usize..8usize, 1..40),
        ) {
            use holon_api::render_eval::sort_value;

            // Pull the virtual row's sort_key from an ACTUAL constructed
            // provider so the test tracks the real production value (not a
            // hand-copied f64::MAX), then encode it through the SAME code path
            // the tree driver uses (`get_sort_key` → `sort_value`).
            let slot = VirtualChildSlot {
                defaults: HashMap::new(),
                parent_id: EntityUri::block("parent-under-test"),
                allow_root_creation: false,
            };
            // The creation slot resolves its parent from `inner`'s rows (bug 2A):
            // seed one row that is a direct child of the container so the flat
            // shape resolves and a slot row is produced. (An empty inner is
            // not-yet-resolvable → no slot — see `row_origin` tests.)
            let mut child = HashMap::new();
            child.insert("id".to_string(), Value::String("block:seed".to_string()));
            child.insert(
                "parent_id".to_string(),
                Value::String("block:parent-under-test".to_string()),
            );
            let inner: Arc<dyn ReactiveRowProvider> =
                Arc::new(FixedRows(vec![Arc::new(child)]));
            let provider = AppendedRowsProvider::creation_slot(inner, &slot);
            // The creation slot's row is appended after inner's rows; it is the
            // one whose id parses to a `CreationPlaceholder`.
            let snap = provider.rows_snapshot();
            let slot_row = snap
                .iter()
                .find(|r| {
                    crate::row_origin::RowOrigin::from_row(r).is_creation_placeholder()
                })
                .expect("creation slot row present");
            let virtual_encoded = sort_value(slot_row.get("sort_key"));

            let mut keys = mint_fractional_keys(&positions);
            // Always include the column default and an after-chain (highest keys
            // production mints), the hardest case for "sorts last" to satisfy.
            keys.push(holon_core::fractional_index::default_sort_key());
            let mut hi = holon_core::fractional_index::default_sort_key();
            for _ in 0..5 {
                hi = holon_core::fractional_index::gen_key_after(&hi).expect("gen_key_after mints a valid key"); // ALLOW(order_minting): test-only reproduction of the order owner's after-key minting to prove the virtual-slot sort contract
                keys.push(hi.clone());
            }

            for k in &keys {
                // SortedChild::cmp orders by `sort_key.cmp(other.sort_key)` first;
                // the id tiebreak only fires on equal keys, which never happens
                // here — so this String cmp IS the decisive sibling comparison.
                let real_encoded = sort_value(Some(&Value::String(k.clone())));
                let ord = virtual_encoded.cmp(&real_encoded);
                proptest::prop_assert_eq!(
                    ord,
                    std::cmp::Ordering::Greater,
                    "virtual creation-slot sort_key {:?} must sort AFTER real \
                     FractionalIndex key {:?} (encoded {:?}), but it sorts {:?}. \
                     f64::MAX encodes to a leading-'1' decimal that loses the \
                     lexicographic race against hex keys — the virtual row jumps \
                     to the TOP of the child list. RED until the sentinel fix lands.",
                    virtual_encoded,
                    k,
                    real_encoded,
                    ord
                );
            }
        }
    }

    /// Reproducer: a single CDC field update on one row should NOT produce a
    /// full `VecDiff::Replace` with all N rows. The flat driver converts
    /// fine-grained diffs to `to_signal_cloned()` which collapses every
    /// per-row update into a full-collection re-emit, causing downstream
    /// GPUI to reconcile the entire view on every minor change.
    #[tokio::test]
    async fn flat_driver_emits_replace_on_single_row_update() {
        let row_set = ReactiveRowSet::new();
        row_set.set_generation(1);

        // Seed 3 rows
        for (id, content) in [("a", "alpha"), ("b", "beta"), ("c", "gamma")] {
            row_set.apply_change(
                holon_api::Change::Created {
                    data: enriched(make_row(id, content)),
                    origin: remote_origin(),
                },
                1,
            );
        }

        let row_set = Arc::new(row_set);
        let data_source: Arc<dyn holon_api::ReactiveRowProvider> = row_set.clone();

        let view = ReactiveView::new_collection(
            CollectionConfig {
                layout: CollectionVariant::from_name("list", 0.0)
                    .expect("`list` layout is registered as a builtin"),
                item_template: RenderExpr::FunctionCall {
                    name: "row".to_string(),
                    args: vec![],
                },
                sort_key: None,
                virtual_child: None,
                rules: Vec::new(),
            },
            data_source,
            None,
            None,
        );

        let services: Arc<dyn crate::reactive::BuilderServices> =
            Arc::new(StubBuilderServices::new());

        view.start(services, &tokio::runtime::Handle::current());

        // Let the driver process the initial Replace.
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Subscribe to the items signal vec AFTER initial population.
        let signal = view.items.signal_vec_cloned();

        // Collect VecDiff events into a shared vec.
        let events: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let events_clone = events.clone();
        let collector = signal.for_each(move |diff| {
            let tag = match &diff {
                VecDiff::Replace { values } => format!("Replace({})", values.len()),
                VecDiff::InsertAt { index, .. } => format!("InsertAt({index})"),
                VecDiff::UpdateAt { index, .. } => format!("UpdateAt({index})"),
                VecDiff::RemoveAt { index } => format!("RemoveAt({index})"),
                VecDiff::Push { .. } => "Push".to_string(),
                VecDiff::Pop {} => "Pop".to_string(),
                VecDiff::Clear {} => "Clear".to_string(),
                VecDiff::Move { .. } => "Move".to_string(),
            };
            events_clone.lock().unwrap().push(tag);
            async {}
        });

        let _collector_handle = tokio::spawn(collector);

        // Let the collector subscribe and receive any initial snapshot.
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        events.lock().unwrap().clear();

        // Now apply a SINGLE field update to row "b".
        row_set.apply_change(
            holon_api::Change::Updated {
                id: "b".to_string(),
                data: enriched(make_row("b", "beta-updated")),
                origin: remote_origin(),
            },
            1,
        );

        // Let the driver process the CDC event.
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let collected = events.lock().unwrap().clone();
        eprintln!("[flat_driver_churn_test] events after single update: {collected:?}");

        let has_replace = collected.iter().any(|e| e.starts_with("Replace"));
        assert!(
            !has_replace,
            "Flat driver emitted VecDiff::Replace on a single-row update — expected a targeted \
             UpdateAt instead. Events: {collected:?}"
        );
        let has_update = collected.iter().any(|e| e.starts_with("UpdateAt"));
        assert!(
            has_update,
            "Expected flat driver to emit VecDiff::UpdateAt for a single-row update. Events: \
             {collected:?}"
        );

        view.stop();
    }

    /// dogfood #6 row 34 (flat-driver half): the STREAMING `list` collection —
    /// the journal feed's own render path (`block:journals::render::0`) — must
    /// honor the `-`-prefixed DESCENDING sort convention. Rows arrive
    /// ASCENDING; `sort_key = "-content"` must render them NEWEST-FIRST.
    ///
    /// RED before the `parse_sort_key` fix in `create_flat_driver`'s
    /// `full_rebuild`: it used the raw spec `"-content"` verbatim as a column
    /// name (`row.get("-content")` → no such column → `None` for every row →
    /// the `cmp_values` result is always `Equal` and the sort degrades to the
    /// `ka.cmp(kb)` arrival-order tie). The static `sorted_rows` and the tree
    /// driver already parse the prefix; only this flat streaming driver was
    /// missed by the A2/A3 landing.
    #[tokio::test]
    async fn flat_driver_honors_descending_sort_key_prefix() {
        crate::shadow_builders::register_render_dsl_widget_names();

        let row_set = ReactiveRowSet::new();
        row_set.set_generation(1);
        // Arrive in ascending / mixed order (10 last, as in the aged vault).
        for content in [
            "2026-07-11",
            "2026-07-12",
            "2026-07-13",
            "2026-07-15",
            "2026-07-16",
            "2026-07-10",
        ] {
            row_set.apply_change(
                holon_api::Change::Created {
                    data: enriched(make_row(content, content)),
                    origin: remote_origin(),
                },
                1,
            );
        }
        let row_set = Arc::new(row_set);
        let data_source: Arc<dyn holon_api::ReactiveRowProvider> = row_set.clone();

        let item_template = holon_api::render_dsl::parse_render_dsl(r#"text(col("content"))"#)
            .expect("item_template parses");

        let view = ReactiveView::new_collection(
            CollectionConfig {
                layout: CollectionVariant::from_name("list", 0.0).expect("`list` layout"),
                item_template,
                sort_key: Some("-content".to_string()),
                virtual_child: None,
                rules: Vec::new(),
            },
            data_source,
            None,
            None,
        );

        let services: Arc<dyn crate::reactive::BuilderServices> =
            Arc::new(StubBuilderServices::new());
        view.start(services, &tokio::runtime::Handle::current());
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let order: Vec<String> = view
            .items
            .lock_ref()
            .iter()
            .filter_map(|n| n.prop_str("content"))
            .collect();
        view.stop();

        assert_eq!(
            order,
            vec![
                "2026-07-16",
                "2026-07-15",
                "2026-07-13",
                "2026-07-12",
                "2026-07-11",
                "2026-07-10",
            ],
            "streaming `list` must sort NEWEST-FIRST for sort_key=\"-content\" (DESC by content); \
             got {order:?}"
        );
    }

    /// Increment B smallest-first-step (also validates Increment G's premise):
    /// the [`AppendedRowsProvider::placement`] (`LiveCell`) case injects a
    /// LIVE second occurrence of block `L` under an anchor collection that
    /// does not contain `L`, DERIVED FROM `L`'s own row cell
    /// — so a write to the canonical block propagates to the placed occurrence
    /// with no copy (the shared-cell model, ADR 0015 §1a). Proves the
    /// wrapper-injection mechanism and the data half of the identity
    /// reframe without touching the store.
    /// The read-only advice template (ADR 0021 v1): a `Placed` advice row must
    /// interpret to a selectable (click-through) row of read-only content + a
    /// `dismiss_advice` op_button — and MUST NOT contain an `editable_text` /
    /// `render_entity` (the collection's normal editable path).
    #[test]
    fn advice_readonly_template_interprets_without_editable_and_with_dismiss() {
        let services: Arc<dyn crate::reactive::BuilderServices> =
            Arc::new(StubBuilderServices::new());

        // A synthesized advice row carries the columns the template binds:
        // id (lesson, click-through target), content, target_id/anchor_id
        // (dismiss dispatch), parent_id (anchor placement).
        let mut row = DataRow::new();
        row.insert("id".into(), Value::String("block:lessonA".into()));
        row.insert("content".into(), Value::String("a woven lesson".into()));
        row.insert("target_id".into(), Value::String("block:lessonA".into()));
        row.insert("anchor_id".into(), Value::String("block:task1".into()));
        row.insert("parent_id".into(), Value::String("block:task1".into()));
        let row = Arc::new(row);

        let expr = advice_readonly_template();
        let ctx = crate::RenderContext::default().with_row(row);
        let node = services.interpret(&expr, &ctx);

        // Collect every widget name + find the op_button / selectable nodes.
        fn walk<'a>(
            n: &'a ReactiveViewModel,
            names: &mut Vec<String>,
            out: &mut Vec<&'a ReactiveViewModel>,
        ) {
            if let Some(name) = n.widget_name() {
                names.push(name);
            }
            out.push(n);
            for c in &n.children {
                walk(c, names, out);
            }
        }
        let mut names = Vec::new();
        let mut nodes = Vec::new();
        walk(&node, &mut names, &mut nodes);

        assert!(
            names.iter().any(|n| n == "selectable"),
            "has selectable: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "op_button"),
            "has op_button: {names:?}"
        );
        assert!(
            names.iter().any(|n| n == "text"),
            "has read-only text: {names:?}"
        );
        assert!(
            !names
                .iter()
                .any(|n| n == "editable_text" || n == "render_entity"),
            "read-only: NO editable_text / render_entity, got {names:?}"
        );

        // The op_button carries the dismiss op name.
        let dismiss = nodes
            .iter()
            .find(|n| n.widget_name().as_deref() == Some("op_button"))
            .expect("op_button node present");
        assert_eq!(
            dismiss
                .props
                .lock_ref()
                .get("op_name")
                .and_then(|v| v.as_string()),
            Some("dismiss_advice"),
            "op_button dispatches dismiss_advice"
        );

        // The selectable carries the click-through navigation.focus action
        // bound to the canonical lesson id.
        let selectable = nodes
            .iter()
            .find(|n| n.widget_name().as_deref() == Some("selectable"))
            .expect("selectable node present");
        let nav = selectable
            .operations
            .iter()
            .find(|w| w.descriptor.entity_name.as_str() == "navigation")
            .expect("selectable has a navigation action");
        assert_eq!(
            nav.descriptor.name, "focus",
            "click-through = navigation.focus"
        );
        assert_eq!(
            nav.descriptor
                .bound_params
                .get("block_id")
                .and_then(|v| v.as_string()),
            Some("block:lessonA"),
            "click-through targets the CANONICAL lesson id"
        );
    }

    #[tokio::test]
    async fn appended_rows_provider_injects_live_second_occurrence() {
        use futures::StreamExt;
        use futures_signals::signal::Mutable;
        use futures_signals::signal::SignalExt;
        use futures_signals::signal_vec::SignalVecExt;

        // A minimal anchor-collection provider holding ONE unrelated row (it does
        // NOT contain L → the bare-id suffix key cannot collide yet).
        struct MockProvider(Vec<Arc<DataRow>>);
        impl ReactiveRowProvider for MockProvider {
            fn rows_snapshot(&self) -> Vec<Arc<DataRow>> {
                self.0.clone()
            }
            fn rows_signal_vec(
                &self,
            ) -> std::pin::Pin<
                Box<dyn futures_signals::signal_vec::SignalVec<Item = Arc<DataRow>> + Send>,
            > {
                Box::pin(futures_signals::signal_vec::always(self.0.clone()))
            }
            fn keyed_rows_signal_vec(
                &self,
            ) -> std::pin::Pin<
                Box<
                    dyn futures_signals::signal_vec::SignalVec<
                            Item = (holon_api::RowKey, Arc<DataRow>),
                        > + Send,
                >,
            > {
                let keyed: Vec<_> = self
                    .0
                    .iter()
                    .map(|r| {
                        (
                            (
                                holon_api::data_row_entity_uri(r).expect("row has id"),
                                holon_api::Occurrence::Canonical,
                            ),
                            r.clone(),
                        )
                    })
                    .collect();
                Box::pin(futures_signals::signal_vec::always(keyed))
            }
            fn cache_identity(&self) -> u64 {
                0
            }
        }

        let anchor = EntityUri::block("panel-b");
        let inner: Arc<dyn ReactiveRowProvider> =
            Arc::new(MockProvider(vec![Arc::new(make_row("other", "other"))]));

        let l = EntityUri::block("c1");
        // L's canonical live row cell — what `ReactiveRowSet::row_mutable` hands out.
        let source = Mutable::new(Arc::new(make_row("c1", "c1")));

        let placed =
            AppendedRowsProvider::placement(inner, l.clone(), anchor.clone(), source.read_only());

        // ── Structure: snapshot = anchor row + one placed occurrence of L ──
        let snap = placed.rows_snapshot();
        assert_eq!(snap.len(), 2, "anchor row + placed occurrence");
        let placed_row = &snap[1];
        assert_eq!(
            holon_api::data_row_entity_uri(placed_row).as_ref(),
            Some(&l),
            "placed row id stays canonical L → edits route to canonical (ADR 0015 rule 3)"
        );
        assert_eq!(
            placed_row.get("parent_id").and_then(|v| v.as_string()),
            Some("block:panel-b"),
            "placed row parent_id = display-local anchor (rule 1, never merged)"
        );
        assert_eq!(
            placed_row.get("content").and_then(|v| v.as_string()),
            Some("c1"),
            "placed occurrence shows canonical content"
        );

        // ── Liveness (SIGNAL): a write to the canonical cell re-emits the suffix,
        //    so the render pipeline sees the placed occurrence update — the
        //    shared-cell dividend, no `converge_input` reconciliation. ──
        // The placed occurrence keys `(l, Placed(occ))`, so match on the entity
        // part of the widened key.
        let l_content = |v: &Vec<(holon_api::RowKey, Arc<DataRow>)>| {
            v.iter().find(|((id, _), _)| id == &l).and_then(|(_, r)| {
                r.get("content")
                    .and_then(|c| c.as_string())
                    .map(str::to_string)
            })
        };
        let mut stream = placed
            .keyed_rows_signal_vec()
            .to_signal_cloned()
            .to_stream();

        let initial = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("initial emission within 2s")
            .expect("stream open");
        assert_eq!(
            l_content(&initial).as_deref(),
            Some("c1"),
            "initial placed content mirrors canonical"
        );

        source.set(Arc::new(make_row("c1", "c1-edited")));

        let updated = tokio::time::timeout(std::time::Duration::from_secs(2), stream.next())
            .await
            .expect("re-emission after canonical write within 2s")
            .expect("stream open");
        assert_eq!(
            l_content(&updated).as_deref(),
            Some("c1-edited"),
            "placed occurrence tracks the canonical block's live cell (converged by construction)"
        );
    }

    /// A keyed provider whose diff stream the test scripts directly, so the
    /// tree driver sees an exact `VecDiff` sequence rather than whatever a
    /// `ReactiveRowSet` happens to emit.
    struct ScriptedRows(MutableVec<(holon_api::RowKey, Arc<DataRow>)>);

    impl ReactiveRowProvider for ScriptedRows {
        fn rows_snapshot(&self) -> Vec<Arc<DataRow>> {
            self.0.lock_ref().iter().map(|(_, r)| r.clone()).collect()
        }
        fn rows_signal_vec(
            &self,
        ) -> std::pin::Pin<
            Box<dyn futures_signals::signal_vec::SignalVec<Item = Arc<DataRow>> + Send>,
        > {
            use futures_signals::signal_vec::SignalVecExt;
            Box::pin(self.0.signal_vec_cloned().map(|(_, r)| r))
        }
        fn keyed_rows_signal_vec(
            &self,
        ) -> std::pin::Pin<
            Box<
                dyn futures_signals::signal_vec::SignalVec<Item = (holon_api::RowKey, Arc<DataRow>)>
                    + Send,
            >,
        > {
            Box::pin(self.0.signal_vec_cloned())
        }
        fn cache_identity(&self) -> u64 {
            0
        }
    }

    fn keyed(row: Arc<DataRow>) -> (holon_api::RowKey, Arc<DataRow>) {
        (
            (
                holon_api::data_row_entity_uri(&row).expect("row has id"),
                holon_api::Occurrence::Canonical,
            ),
            row,
        )
    }

    /// Boot-crash reproducer at the driver+tree PAIR level — the layer with no
    /// coverage until now (every `mutable_tree` test drives `MutableTree`
    /// directly, so the driver's bookkeeping was never exercised).
    ///
    /// `MutableTree::remove` evicts the node AND its whole subtree, but used to
    /// return `()`. The driver dropped exactly ONE key from `key_index`/
    /// `row_map`, so surviving descendants stayed live upstream while being
    /// gone from the tree. The next CDC touch of such a descendant arrives as
    /// `UpdateAt` (upstream still has the key) → `tree.update` → panic
    /// `MutableTree::update on unknown node`.
    #[tokio::test]
    async fn tree_driver_survives_update_of_child_whose_parent_was_removed() {
        crate::shadow_builders::register_render_dsl_widget_names();

        let parent = Arc::new(make_row("p", "parent"));
        let mut child_row = make_row("c", "child");
        child_row.insert("parent_id".to_string(), Value::String("p".to_string()));
        let child = Arc::new(child_row);

        let rows = MutableVec::new();
        let source = Arc::new(ScriptedRows(rows.clone()));
        let data_source: Arc<dyn holon_api::ReactiveRowProvider> = source.clone();

        let view = ReactiveView::new_collection(
            CollectionConfig {
                layout: CollectionVariant::from_name("tree", 0.0)
                    .expect("`tree` layout is registered as a builtin"),
                item_template: RenderExpr::FunctionCall {
                    name: "row".to_string(),
                    args: vec![],
                },
                sort_key: None,
                virtual_child: None,
                rules: Vec::new(),
            },
            data_source,
            None,
            None,
        );

        // The driver runs in a spawned task; a panic there aborts only that
        // task, so capture it through the panic hook to assert on it here.
        // (nextest gives each test its own process, so the global hook is safe.)
        let panics: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = panics.clone();
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            sink.lock().unwrap().push(info.to_string());
        }));

        let services: Arc<dyn crate::reactive::BuilderServices> =
            Arc::new(StubBuilderServices::new());
        view.start(services, &tokio::runtime::Handle::current());

        // Deterministic settle: yield to the spawned driver task until it has
        // fully applied the pending diff (observed via the flat `MutableVec`),
        // instead of a fixed sleep. A fixed sleep couples correctness to CPU
        // scheduling — under parallel-nextest load the driver task can be
        // starved past the deadline, dropping this test into a flaky failure.
        // Breaks early on an observed driver panic so the assertions below
        // report the real cause rather than a settle timeout.
        macro_rules! settle_until {
            ($ready:expr, $desc:expr) => {{
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    if !panics.lock().unwrap().is_empty() {
                        break; // driver panicked — let the assertions report it
                    }
                    if $ready {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "tree driver did not settle within 10s waiting for {}; no panic \
                         observed — the spawned driver task was starved or a diff was dropped",
                        $desc,
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                }
            }};
        }

        // InsertAt parent → InsertAt child → RemoveAt parent → UpdateAt child.
        rows.lock_mut().insert_cloned(0, keyed(parent));
        settle_until!(
            view.items.lock_ref().len() == 1,
            "parent inserted (1 flat item)"
        );
        rows.lock_mut().insert_cloned(1, keyed(child.clone()));
        settle_until!(
            view.items.lock_ref().len() == 2,
            "child nested under parent (2 flat items)"
        );
        rows.lock_mut().remove(0);
        settle_until!(
            view.items.lock_ref().len() == 1,
            "parent removed, child re-rooted (1 flat item)"
        );

        // The update keeps the flat length at 1, so wait on the item's Arc
        // identity changing — `MutableTree::update` installs a fresh `Arc` via
        // `set_cloned`, which never runs if the driver panics reconciling it.
        let before = view
            .items
            .lock_ref()
            .first()
            .map(std::sync::Arc::as_ptr)
            .expect("child must be present as a root before its content edit");
        let mut edited = (*child).clone();
        edited.insert(
            "content".to_string(),
            Value::String("child-edited".to_string()),
        );
        rows.lock_mut().set_cloned(0, keyed(Arc::new(edited)));
        settle_until!(
            view.items.lock_ref().first().map(std::sync::Arc::as_ptr) != Some(before),
            "child content update applied (item replaced)"
        );

        std::panic::set_hook(previous_hook);

        let observed = panics.lock().unwrap().clone();
        assert!(
            observed.is_empty(),
            "tree driver panicked reconciling an update to a child whose parent was removed — \
             the remove cascade evicted the child from the tree without telling the driver. \
             Panics: {observed:?}"
        );
        assert_eq!(
            view.items.lock_ref().len(),
            1,
            "the child is still in the upstream row set, so it must still render — as a root, \
             its removed parent gone"
        );

        view.stop();
    }
}
