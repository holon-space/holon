use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use futures_signals::signal::ReadOnlyMutable;
use holon_api::render_types::OperationWiring;
use holon_api::widget_spec::DataRow;

use crate::reactive::BuilderServices;

/// Viewport or container allocation in logical + physical pixels.
///
/// Refined per subtree during render interpretation: each layout container
/// that declares a `child_space` clause (e.g. `columns`) partitions its
/// incoming `available_space` among its children, and the refined value
/// flows into `pick_active_variant` so profile variants can select different
/// renders based on how much room they have — CSS container queries.
///
/// Logical pixels are the primary input (already DPI-normalized by the UI
/// framework); physical pixels and `scale_factor` are secondary signals for
/// density-aware decisions.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AvailableSpace {
    pub width_px: f32,
    pub height_px: f32,
    pub width_physical_px: f32,
    pub height_physical_px: f32,
    pub scale_factor: f32,
}

/// Hint from a widget to its parent layout container about how much space it
/// needs.
///
/// Maps directly to CSS flex properties:
/// - `Flex { weight }` → `flex: weight` (proportional share of remaining space)
/// - `Fixed { px }` → `flex: 0 0 Npx` (exact allocation, no grow/shrink)
///
/// Overlay drawers use `Fixed { px: 0.0 }` because they don't participate in
/// flow layout — they float above siblings without consuming horizontal space.
///
/// `PinnedToEnd` is the vertical-flow counterpart (CSS `position: sticky` at
/// the trailing edge / Flutter's `Expanded`-sibling pinning): the child
/// declares WHERE it sits, its container decides whether it can honour that.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LayoutHint {
    /// Takes a proportional share of remaining space after Fixed children.
    Flex { weight: f32 },
    /// Takes exactly this many pixels. Does not grow or shrink.
    Fixed { px: f32 },
    /// Sits at the TRAILING edge of a vertical flow container, outside the
    /// scrolling region: the siblings above scroll, this child stays put and
    /// shrinks to its content.
    ///
    /// Carries no size: how tall the pinned child may grow is its own concern
    /// (the `accordion` caps its body at `max_height_fraction` for every
    /// placement, pinned or not), while WHERE it sits is what the container
    /// must act on. Horizontal containers (`columns`) have no trailing edge to
    /// pin to and treat it as an unweighted flex child.
    PinnedToEnd,
}

impl Default for LayoutHint {
    fn default() -> Self {
        Self::Flex { weight: 1.0 }
    }
}

impl LayoutHint {
    /// The exact pixel width this child claims from a horizontal container, if
    /// any.
    pub fn fixed_px(self) -> Option<f32> {
        match self {
            Self::Fixed { px } => Some(px),
            Self::Flex { .. } | Self::PinnedToEnd => None,
        }
    }

    /// The proportional share this child claims from a horizontal container,
    /// or `None` when it claims a fixed width instead.
    ///
    /// `PinnedToEnd` speaks about the vertical flow only and makes no width
    /// claim, so horizontal containers give it the default share.
    pub fn flex_weight(self) -> Option<f32> {
        match self {
            Self::Flex { weight } => Some(weight),
            Self::PinnedToEnd => Some(1.0),
            Self::Fixed { .. } => None,
        }
    }
}

/// What the IMMEDIATE parent container can honour for the child being built —
/// a capability the container declares about ITSELF, never an identity the
/// child has to recognise.
///
/// Scoped to exactly one level by construction: `RenderInterpreter::dispatch`
/// hands the value to the builder through `BuilderArgs::parent_capability` and
/// strips it from the `RenderContext` that same builder passes on, so a widget
/// two levels down (an `accordion` inside a `row` inside a `column`) can never
/// see it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ContainerCapability {
    /// Nothing beyond ordinary in-flow stacking.
    #[default]
    None,
    /// Vertical flow container that renders [`LayoutHint::PinnedToEnd`]
    /// children at its trailing edge while the rest scrolls (`column`).
    PinToEnd,
    /// Scroll region that hosts in-flow and sticky sections (`section_stack`).
    ScrollSections,
}

static EMPTY_ROW: std::sync::LazyLock<Arc<DataRow>> =
    std::sync::LazyLock::new(|| Arc::new(HashMap::new()));

/// Pure data context passed through the render tree during interpretation.
///
/// Two levels of row binding:
///
/// - `data_rows` — the container's full row set. Set once when entering a
///   block's scope (e.g. via `interpret_pure` or `with_data_rows`). Iterated by
///   the `Collection` macro expansion (static arm), `shared_tree_build`, and
///   `pref_field`. Never modified by per-row binding.
///
/// - `current_row` — the single row bound by a collection's per-item loop. Set
///   by `with_row(row)`. Leaf widgets read this via `row()`.
///
/// `row()` prefers `current_row` and falls back to `data_rows[0]`.
/// This makes the "which row am I looking at?" question explicit: after
/// `with_row`, leaves see the bound row; before `with_row`, they see the
/// first container row (or `EMPTY_ROW` if nothing was bound).
#[derive(Clone, Default)]
pub struct RenderContext {
    /// Shared slice: `RenderContext` is cloned per `with_*` builder call —
    /// twice per row in the collection pipeline — so this must be a
    /// refcount bump, not an O(rows) Vec clone (O(N²) per interpretation
    /// otherwise).
    pub data_rows: Arc<[Arc<DataRow>]>,
    /// The single row bound by a collection's per-item loop. Set by `with_row`.
    /// Takes precedence over `data_rows[0]` in `row()` / `row_arc()`.
    pub current_row: Option<Arc<DataRow>>,
    /// Shared read-only handle for the bound row's data, supplied by the
    /// collection driver when CDC owns the data. Leaf builders clone this
    /// handle into `ReactiveViewModel::data` so parent and children share
    /// one `MutableState<Arc<DataRow>>` — a single CDC `.set()` (only
    /// callable through `ReactiveRowSet::apply_change`'s private `Mutable`
    /// handle) lights up every subscribed leaf without any tree walk. The
    /// `ReadOnlyMutable` type makes "downstream tries to mutate row data" a
    /// **compile error**, which is what keeps the one-writer invariant from
    /// drifting back to multi-writer in some future refactor. Absent for
    /// interpret-only call sites (MCP, PBT snapshots, design gallery), in
    /// which case `data_mutable()` synthesises a one-shot
    /// `Mutable::new(snapshot).read_only()`.
    pub current_row_mutable: Option<ReadOnlyMutable<Arc<DataRow>>>,
    /// The operations offered to the bound row: [`Self::offered_operations`]
    /// as the write tier judges that row. Every widget reads its offers here.
    pub operations: Vec<OperationWiring>,
    /// The operations this context offers before the write tier judges a row.
    /// Each rebinding of the row judges them again, so an item is offered what
    /// ITS row is offered, not what the enclosing row was.
    pub offered_operations: Vec<OperationWiring>,
    /// Input triggers derived from operations. Propagated to ViewModel nodes
    /// so frontends can check them locally on each keystroke.
    pub triggers: Vec<crate::input_trigger::InputTrigger>,
    /// Nesting depth (for indentation in block builders)
    pub depth: usize,
    /// Query nesting depth — tracks recursive query execution to prevent stack
    /// overflow.
    pub query_depth: usize,
    /// Live data source for collection builders. When set, shadow builders
    /// create `ReactiveView::new_collection()` with a live streaming
    /// pipeline instead of static snapshots. Set by `watch_live()` before
    /// calling `interpret_fn`. `None` for headless/snapshot consumers (MCP,
    /// PBT, TUI).
    pub data_source: Option<Arc<dyn holon_api::ReactiveRowProvider>>,
    /// Container-query allocation: how much space THIS subtree was allotted by
    /// its parent. Refined by layout containers via `with_available_space`
    /// before recursing into children. `None` outside a partitioning
    /// container; the
    // ALLOW(fallback): describing the global-viewport merge in pick_active_variant
    /// global viewport is merged in as a fallback by `pick_active_variant`.
    pub available_space: Option<AvailableSpace>,
    /// What the immediate parent container offers this child — see
    /// [`ContainerCapability`]. Set by the container via [`Self::offering`],
    /// read by the child through `BuilderArgs::parent_capability`, and stripped
    /// by the interpreter so it never reaches a grandchild.
    pub parent_capability: ContainerCapability,
    /// Render-context flags that participate in `pick_active_variant`
    /// evaluation alongside row columns and `available_space`. Well-known
    /// flags: `role` (e.g. `"page_title"`), `view_mode`, `embed_depth`. Set
    /// on a child context by callers that want the resolved render to
    /// dispatch on context (e.g. `live_block(id, #{role: "page_title"})`);
    /// cleared inside the resolved block's own descendants so flags scope
    /// only to the immediate target.
    pub flags: HashMap<String, holon_api::Value>,
    /// The id of the entity whose resolved render this context interprets —
    /// set by `live_block` / `watch_live` / `live_query` when they mount a
    /// block's render expression over that block's data rows. Collection
    /// builders read it (via `render_interpreter::collection_context_root_id`)
    /// to mark the row whose id equals it as the tree's context root
    /// (`is_context_root` positional key). NOT derivable from `row()`: a
    /// mounted render context has no bound row, and `row()` then reads
    /// `data_rows.first()` — an arbitrary result row.
    pub context_entity: Option<String>,
}

impl RenderContext {
    /// The current row's data for ColumnRef resolution.
    ///
    /// Reads `current_row` (set by `with_row` inside a collection's per-item
    /// loop), falling back to `data_rows[0]`, falling back to `EMPTY_ROW`.
    pub fn row(&self) -> &DataRow {
        self.current_row
            .as_deref()
            .or_else(|| self.data_rows.first().map(|a| a.as_ref()))
            .unwrap_or(&EMPTY_ROW)
    }

    /// The current row as an Arc (cheap clone for entity attachment).
    pub fn row_arc(&self) -> Arc<DataRow> {
        self.current_row
            .clone()
            .or_else(|| self.data_rows.first().cloned())
            .unwrap_or_else(|| Arc::clone(&EMPTY_ROW))
    }

    /// Shared `ReadOnlyMutable<Arc<DataRow>>` for the bound row.
    ///
    /// Returns the CDC-owned read-only handle when the collection driver
    /// supplied one (`for_row_mutable`), so all leaves rendered for the
    /// same row share the same `Arc<MutableState>`. The `ReadOnlyMutable`
    /// type makes leaf-side mutation a compile error — only
    /// `ReactiveRowSet::apply_change`, which holds the writable `Mutable`,
    /// can update row data. Falls back to a one-shot
    /// `Mutable::new(row_arc()).read_only()` for interpret-only call sites
    /// with no live data source (MCP, PBT snapshots, design gallery) —
    /// those leaves are rendered once and never updated.
    pub fn data_mutable(&self) -> ReadOnlyMutable<Arc<DataRow>> {
        self.current_row_mutable
            .clone()
            .unwrap_or_else(|| Mutable::new(self.row_arc()).read_only())
    }

    /// Create a child context with new operations.
    /// Automatically derives default input triggers from the operations.
    /// Joins keybindings from the services registry into each
    /// OperationDescriptor.
    ///
    /// This is the ONE point every widget's offers pass through — each reads
    /// them back as `ctx.operations` — so the write tier is applied here: a row
    /// the dispatcher refuses is offered none of the operations the tier judges
    /// (D46). `ops_of` withholds the same operations on its own path
    /// ([`crate::value_fns::ops_of`]), because it offers a subject no row bound
    /// here names.
    pub fn with_operations(
        &self,
        mut operations: Vec<OperationWiring>,
        services: &dyn BuilderServices,
    ) -> Self {
        let bindings = services.key_bindings_snapshot();
        if !bindings.is_empty() {
            for op in &mut operations {
                if let Some(chord) = bindings.get(&op.descriptor.name) {
                    op.descriptor.trigger = Some(holon_api::Trigger::KeyChord {
                        chord: chord.clone(),
                    });
                }
            }
        }
        Self {
            offered_operations: operations,
            ..self.clone()
        }
        .judged_for_row(services)
    }

    /// `operations` and `triggers` from `offered_operations`, as the write
    /// tier judges the bound row.
    fn judged_for_row(mut self, services: &dyn BuilderServices) -> Self {
        let judged = |w: &OperationWiring| {
            w.descriptor.entity_name.as_str() == holon_core::WRITE_TIER_ENTITY
        };
        // The subject the dispatcher judges is the id a click sends, which is
        // this context's bound row. A row that names no entity has no subject,
        // so it is offered nothing the tier judges, like a refused one.
        let mut operations = self.offered_operations.clone();
        if operations.iter().any(judged) {
            let admitted = holon_api::row_id_of(self.row())
                .entity()
                .is_some_and(|s| services.write_tier_admits(&s));
            if !admitted {
                operations.retain(|w| !judged(w));
            }
        }
        self.triggers = crate::input_trigger::default_triggers_for_operations(&operations);
        self.operations = operations;
        self
    }

    /// Bind a single row for leaf widget resolution.
    ///
    /// Sets `current_row` — takes precedence over `data_rows[0]` in `row()`.
    /// Preserves the parent's `data_rows` (container rows stay available for
    /// builders like `pref_field` that search the full set).
    ///
    /// Clears `current_row_mutable` because the snapshot path has no shared
    /// CDC handle. Live drivers must use `for_row_mutable` instead.
    ///
    /// Judges the offered operations again for the new row.
    pub fn with_row(&self, row: Arc<DataRow>, services: &dyn BuilderServices) -> Self {
        Self {
            current_row: Some(row),
            current_row_mutable: None,
            ..self.clone()
        }
        .judged_for_row(services)
    }

    /// A context with `row` bound and nothing offered yet.
    pub fn for_row(row: Arc<DataRow>) -> Self {
        Self {
            current_row: Some(row),
            ..Self::default()
        }
    }

    /// A context with a row bound via a CDC-owned `ReadOnlyMutable` handle,
    /// and nothing offered yet.
    ///
    /// All leaves rendered under this context share the same
    /// `Arc<MutableState>` — when `ReactiveRowSet::apply_change` writes the
    /// row through its private writable `Mutable`, every leaf that holds a
    /// clone of this read-only handle observes the new value via signal
    /// subscription. The `ReadOnlyMutable` type guarantees no leaf can
    /// write — that is the type-level enforcement of "one writer =
    /// `ReactiveRowSet`". The `current_row` snapshot is also set so
    /// synchronous reads (`row()`, `row_arc()`) resolve immediately during
    /// interpretation.
    pub fn for_row_mutable(handle: ReadOnlyMutable<Arc<DataRow>>) -> Self {
        Self {
            current_row: Some(handle.get_cloned()),
            current_row_mutable: Some(handle),
            ..Self::default()
        }
    }

    /// Create a child context with the given container rows.
    ///
    /// Clears `current_row` — the new context is at the container level,
    /// not inside a per-row binding.
    pub fn with_data_rows(&self, data_rows: impl Into<Arc<[Arc<DataRow>]>>) -> Self {
        Self {
            data_rows: data_rows.into(),
            current_row: None,
            ..self.clone()
        }
    }

    /// Bind the entity whose resolved render this context interprets — see
    /// the `context_entity` field.
    pub fn with_context_entity(&self, id: impl Into<String>) -> Self {
        Self {
            context_entity: Some(id.into()),
            ..self.clone()
        }
    }

    /// Create a child context with incremented query depth.
    pub fn deeper_query(&self) -> Self {
        Self {
            query_depth: self.query_depth + 1,
            ..self.clone()
        }
    }

    /// Create a child context with incremented nesting depth.
    pub fn indented(&self) -> Self {
        Self {
            depth: self.depth + 1,
            ..self.clone()
        }
    }

    /// Refine the container-query allocation for this subtree.
    ///
    /// Layout containers call this before recursing into their children so
    /// profile variants can key on `available_width_px` / `available_height_px`
    /// for the child's actual allotted space, not the parent's.
    pub fn with_available_space(&self, space: AvailableSpace) -> Self {
        Self {
            available_space: Some(space),
            ..self.clone()
        }
    }

    /// Declare what this container can honour for the child about to be built.
    ///
    /// Every direct child gets the same offer — the container describes its own
    /// capability and never inspects what the child is. A child whose
    /// declared layout needs the capability (an `accordion` asking to pin to
    /// the trailing edge) errors loudly when the offer is absent.
    pub fn offering(&self, capability: ContainerCapability) -> Self {
        Self {
            parent_capability: capability,
            ..self.clone()
        }
    }

    /// Set render-context flags for the immediate child render. Replaces the
    /// existing flag map. Used by `live_block(id, #{role: ...})` and tree
    /// builder rule evaluation; descendants inside the resolved block reset
    /// to `flags: HashMap::new()` so flags scope only to the target.
    pub fn with_flags(&self, flags: HashMap<String, holon_api::Value>) -> Self {
        Self {
            flags,
            ..self.clone()
        }
    }

    /// Drop all render-context flags. Used when descending into the resolved
    /// body of a `live_block` so its own children render as ordinary content.
    pub fn without_flags(&self) -> Self {
        Self {
            flags: HashMap::new(),
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_space() -> AvailableSpace {
        AvailableSpace {
            width_px: 800.0,
            height_px: 600.0,
            width_physical_px: 1600.0,
            height_physical_px: 1200.0,
            scale_factor: 2.0,
        }
    }

    #[test]
    fn with_available_space_then_with_row_preserves_both() {
        let space = sample_space();
        let row: Arc<DataRow> = Arc::new(HashMap::new());
        let ctx = RenderContext::default()
            .with_available_space(space)
            .with_row(row.clone(), &crate::reactive::StubBuilderServices::new());
        assert_eq!(ctx.available_space, Some(space));
        assert!(ctx.current_row.is_some());
    }

    #[test]
    fn with_row_then_with_available_space_preserves_both() {
        let space = sample_space();
        let row: Arc<DataRow> = Arc::new(HashMap::new());
        let ctx = RenderContext::for_row(row).with_available_space(space);
        assert_eq!(ctx.available_space, Some(space));
        assert!(ctx.current_row.is_some());
    }

    #[test]
    fn default_context_has_no_available_space() {
        assert!(RenderContext::default().available_space.is_none());
    }

    /// D46 at the one point every widget reads its offers from: the write tier
    /// refuses the bound row, so the operations it judges are not offered —
    /// while an operation of another entity still is.
    #[test]
    fn a_refused_row_is_offered_none_of_the_operations_the_write_tier_judges() {
        let subject = holon_api::EntityUri::parse("block:recipe-step").expect("parses");
        let row: Arc<DataRow> = Arc::new(HashMap::from([(
            "id".to_string(),
            holon_api::Value::String(subject.to_string()),
        )]));
        let ops = vec![
            wiring(holon_core::WRITE_TIER_ENTITY, "set_field"),
            wiring("navigation", "focus"),
        ];

        let admitting = crate::reactive::StubBuilderServices::new();
        let refusing = crate::reactive::StubBuilderServices::new().with_write_refused(subject);
        let offered = |services: &dyn BuilderServices| -> Vec<String> {
            RenderContext::for_row(Arc::clone(&row))
                .with_operations(ops.clone(), services)
                .operations
                .iter()
                .map(|w| w.descriptor.name.clone())
                .collect()
        };

        assert_eq!(offered(&admitting), vec!["set_field", "focus"]);
        assert_eq!(offered(&refusing), vec!["focus"]);
    }

    /// Drag and drop are offers too: `draggable` and `drop_zone` bind the
    /// `move_block` their row is offered, so a writable row keeps both and a
    /// refused row binds neither.
    #[test]
    fn drag_and_drop_bind_the_move_their_row_is_offered() {
        let subject = holon_api::EntityUri::parse("block:recipe-step").expect("parses");
        let row: Arc<DataRow> = Arc::new(HashMap::from([(
            "id".to_string(),
            holon_api::Value::String(subject.to_string()),
        )]));
        let ops = vec![
            wiring(holon_core::WRITE_TIER_ENTITY, "move_block"),
            wiring(holon_core::WRITE_TIER_ENTITY, "set_field"),
        ];
        let expr =
            holon_api::render_dsl::parse_render_dsl(r#"column(draggable(text("x")), drop_zone())"#)
                .expect("parses");
        let bound =
            |services: &crate::reactive::StubBuilderServices| -> Vec<(String, Vec<String>)> {
                let ctx =
                    RenderContext::for_row(Arc::clone(&row)).with_operations(ops.clone(), services);
                let column = services.interpret(&expr, &ctx);
                column
                    .children
                    .iter()
                    .map(|node| {
                        (
                            node.widget_name().expect("a widget"),
                            node.operations
                                .iter()
                                .map(|w| w.descriptor.name.clone())
                                .collect(),
                        )
                    })
                    .collect()
            };

        let admitting = crate::reactive::StubBuilderServices::new();
        let refusing = crate::reactive::StubBuilderServices::new().with_write_refused(subject);
        assert_eq!(
            bound(&admitting),
            vec![
                ("draggable".to_string(), vec!["move_block".to_string()]),
                ("drop_zone".to_string(), vec!["move_block".to_string()]),
            ]
        );
        assert_eq!(
            bound(&refusing),
            vec![
                ("draggable".to_string(), vec![]),
                ("drop_zone".to_string(), vec![]),
            ]
        );
    }

    /// `render_entity` unions every offered row operation onto the root of
    /// the profile render. A drag/drop widget at that root still offers only
    /// the operation it binds.
    #[test]
    fn a_drag_drop_widget_at_a_profile_root_offers_only_its_own_operation() {
        for render in [r#"draggable(text("x"))"#, "drop_zone()"] {
            let subject = holon_api::EntityUri::parse("block:writable").expect("parses");
            let profile = holon_api::RenderProfile {
                name: "block".to_string(),
                render: holon_api::render_dsl::parse_render_dsl(render).expect("parses"),
                operations: ["move_block", "set_field", "indent"]
                    .into_iter()
                    .map(|name| wiring(holon_core::WRITE_TIER_ENTITY, name).descriptor)
                    .collect(),
                variants: vec![],
            };
            let row: Arc<DataRow> = Arc::new(HashMap::from([(
                "id".to_string(),
                holon_api::Value::String(subject.to_string()),
            )]));
            let entity =
                holon_api::render_dsl::parse_render_dsl("render_entity()").expect("parses");
            let root = |services: crate::reactive::StubBuilderServices| {
                services.interpret(&entity, &RenderContext::for_row(Arc::clone(&row)))
            };

            let writable =
                root(crate::reactive::StubBuilderServices::new().with_profile(profile.clone()));
            assert_eq!(
                crate::user_driver::node_drag_drop_operation(&writable)
                    .map(|w| w.descriptor.name.as_str()),
                Some("move_block"),
                "{render} on a writable row"
            );
            let refused = root(
                crate::reactive::StubBuilderServices::new()
                    .with_profile(profile)
                    .with_write_refused(subject),
            );
            assert_eq!(
                crate::user_driver::node_drag_drop_operation(&refused)
                    .map(|w| w.descriptor.name.as_str()),
                None,
                "{render} on a refused row"
            );
        }
    }

    /// A `card` carries the operations its row is offered, as `render_entity`
    /// does, so a board over `card(...)` items knows which cards may drag.
    #[test]
    fn a_card_carries_the_operations_its_row_is_offered() {
        let subject = holon_api::EntityUri::parse("block:card-row").expect("parses");
        let row: Arc<DataRow> = Arc::new(HashMap::from([(
            "id".to_string(),
            holon_api::Value::String(subject.to_string()),
        )]));
        let ops = vec![
            wiring(holon_core::WRITE_TIER_ENTITY, "move_block"),
            wiring(holon_core::WRITE_TIER_ENTITY, "set_field"),
            wiring("navigation", "focus"),
        ];
        let expr = holon_api::render_dsl::parse_render_dsl(r#"card(text("x"))"#).expect("parses");
        let carried = |services: &crate::reactive::StubBuilderServices| -> Vec<String> {
            let ctx =
                RenderContext::for_row(Arc::clone(&row)).with_operations(ops.clone(), services);
            services
                .interpret(&expr, &ctx)
                .operations
                .iter()
                .map(|w| w.descriptor.name.clone())
                .collect()
        };

        let admitting = crate::reactive::StubBuilderServices::new();
        let refusing = crate::reactive::StubBuilderServices::new().with_write_refused(subject);
        assert_eq!(
            carried(&admitting),
            vec!["move_block", "set_field", "focus"]
        );
        assert_eq!(carried(&refusing), vec!["focus"]);
    }

    /// A collection item judged against the write tier is judged as ITSELF,
    /// not as the row whose context it renders in: binding the item's row
    /// re-judges the operations the enclosing context carries.
    #[test]
    fn a_static_board_item_is_offered_what_its_own_row_is_offered() {
        crate::shadow_builders::register_render_dsl_widget_names();
        let parent = holon_api::EntityUri::parse("block:board-parent").expect("parses");
        let read_only = holon_api::EntityUri::parse("block:read-only-item").expect("parses");
        let writable = holon_api::EntityUri::parse("block:writable-item").expect("parses");
        let parent_row: Arc<DataRow> = Arc::new(HashMap::from([(
            "id".to_string(),
            holon_api::Value::String(parent.to_string()),
        )]));
        let ops = vec![
            wiring(holon_core::WRITE_TIER_ENTITY, "move_block"),
            wiring(holon_core::WRITE_TIER_ENTITY, "set_field"),
        ];
        let offered_per_item = |services: &crate::reactive::StubBuilderServices,
                                tmpl: &str|
         -> Vec<(Option<String>, Vec<String>)> {
            let ctx = RenderContext::for_row(Arc::clone(&parent_row))
                .with_operations(ops.clone(), services);
            let src = format!(
                r#"board(#{{lane_field: "task_state", item_template: {tmpl}, rows: [#{{id: "{read_only}", task_state: "TODO"}}, #{{id: "{writable}", task_state: "TODO"}}, #{{task_state: "TODO", content: "no subject"}}]}})"#
            );
            let expr = holon_api::render_dsl::parse_render_dsl(&src).expect("parses");
            let board = services.interpret(&expr, &ctx);
            let lane = board.children.first().expect("one lane");
            lane.children
                .iter()
                .map(|item| {
                    (
                        item.prop_str("row_id"),
                        item.operations
                            .iter()
                            .map(|w| w.descriptor.name.clone())
                            .collect(),
                    )
                })
                .collect()
        };

        for (tmpl, expected) in [
            (r#"card(text("x"))"#, vec!["move_block", "set_field"]),
            (r#"draggable(text("x"))"#, vec!["move_block"]),
            ("drop_zone()", vec!["move_block"]),
        ] {
            let expected: Vec<String> = expected.into_iter().map(String::from).collect();
            let want = vec![
                (Some(read_only.to_string()), Vec::<String>::new()),
                (Some(writable.to_string()), expected),
                (None, Vec::<String>::new()),
            ];
            let under_writable_parent =
                crate::reactive::StubBuilderServices::new().with_write_refused(read_only.clone());
            assert_eq!(
                offered_per_item(&under_writable_parent, tmpl),
                want,
                "{tmpl} under a writable parent"
            );
            let under_read_only_parent = crate::reactive::StubBuilderServices::new()
                .with_write_refused(read_only.clone())
                .with_write_refused(parent.clone());
            assert_eq!(
                offered_per_item(&under_read_only_parent, tmpl),
                want,
                "{tmpl} under a read-only parent"
            );
        }
    }

    fn wiring(entity_name: &str, name: &str) -> OperationWiring {
        OperationWiring {
            modified_param: String::new(),
            descriptor: holon_api::OperationDescriptor {
                entity_name: entity_name.into(),
                entity_short_name: entity_name.to_string(),
                id_column: "id".to_string(),
                name: name.to_string(),
                display_name: name.to_string(),
                description: String::new(),
                required_params: vec![],
                optional_params: vec![],
                affected_fields: vec![],
                param_mappings: vec![],
                target_scope: holon_api::TargetScope::Global,
                boundary_behavior: holon_api::BoundaryBehavior::Unclassified,
                menu_exposure: holon_api::MenuExposure::NotListed {
                    surface: holon_api::NonMenuSurface::PointerGesture,
                },
                trigger: None,
                bound_params: Default::default(),
                guard: holon_api::pattern::OpGuard::None,
                marking_delta: holon_api::marking::MarkingDelta::Undeclared,
                arcs: holon_api::arcs::TransitionArcs::Declared {
                    reads: vec![],
                    emits: vec![],
                },
            },
        }
    }
}
