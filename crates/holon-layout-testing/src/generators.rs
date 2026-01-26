//! Proptest strategies for generating random `ReactiveViewModel` trees and
//! `Scenario` values. All generators are frontend-agnostic — no GPUI dep.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use futures_signals::signal_vec::MutableVec;
use holon_api::render_types::{Arg, RenderExpr};
use holon_api::widget_spec::{DataRow, EnrichedRow};
use holon_api::{Change, ChangeOrigin, EntityUri, Value};
use holon_frontend::reactive::{BuilderServices, ReactiveQueryResults, StubBuilderServices};
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::{CollectionVariant, ReactiveViewModel};
use holon_frontend::RenderContext;
use proptest::prelude::*;

use crate::blueprint::{BlockHandle, Blueprint, DrawerHandle, Shape};
use crate::registry::BlockTreeThunk;
use crate::scenario::Scenario;
use crate::ui_interaction::UiInteraction;

// ── Thunk constructors ────────────────────────────────────────────────────

fn thunk(build: impl Fn() -> ReactiveViewModel + Send + Sync + 'static) -> Shape {
    Shape(Arc::new(build))
}

pub fn vm_text(s: String) -> Shape {
    Shape(Arc::new(move || ReactiveViewModel::text(&s)))
}

pub fn vm_badge(label: String) -> Shape {
    thunk(move || ReactiveViewModel::leaf("badge", Value::String(label.clone())))
}

pub fn vm_icon() -> Shape {
    thunk(|| ReactiveViewModel::leaf("icon", Value::String("info".to_string())))
}

pub fn vm_checkbox(checked: bool) -> Shape {
    thunk(move || ReactiveViewModel::leaf("checkbox", Value::Boolean(checked)))
}

pub fn vm_spacer(w: f32, h: f32) -> Shape {
    thunk(move || {
        let mut props = std::collections::HashMap::new();
        props.insert("width".to_string(), Value::Float(w as f64));
        props.insert("height".to_string(), Value::Float(h as f64));
        ReactiveViewModel::from_widget("spacer", props)
    })
}

fn materialize_children(children: &[Shape]) -> Vec<ReactiveViewModel> {
    children.iter().map(|c| c.materialize()).collect()
}

pub fn vm_col(children: Vec<Shape>) -> Shape {
    thunk(move || ReactiveViewModel::layout("column", materialize_children(&children)))
}

pub fn vm_row(children: Vec<Shape>) -> Shape {
    thunk(move || ReactiveViewModel::layout("row", materialize_children(&children)))
}

pub fn vm_card(children: Vec<Shape>) -> Shape {
    thunk(move || {
        let mut props = std::collections::HashMap::new();
        props.insert("accent".to_string(), Value::String("#5DBDBD".to_string()));
        ReactiveViewModel {
            children: materialize_children(&children)
                .into_iter()
                .map(Arc::new)
                .collect(),
            ..ReactiveViewModel::from_widget("card", props)
        }
    })
}

pub fn vm_chat_bubble(sender: String, time: String, children: Vec<Shape>) -> Shape {
    thunk(move || {
        let mut props = std::collections::HashMap::new();
        props.insert("sender".to_string(), Value::String(sender.clone()));
        props.insert("time".to_string(), Value::String(time.clone()));
        ReactiveViewModel {
            children: materialize_children(&children)
                .into_iter()
                .map(Arc::new)
                .collect(),
            ..ReactiveViewModel::from_widget("chat_bubble", props)
        }
    })
}

pub fn vm_focusable(child: Shape) -> Shape {
    thunk(move || {
        ReactiveViewModel::from_widget("focusable", std::collections::HashMap::new())
            .with_children(vec![child.materialize()])
    })
}

pub fn vm_draggable(child: Shape) -> Shape {
    thunk(move || {
        ReactiveViewModel::from_widget("draggable", std::collections::HashMap::new())
            .with_children(vec![child.materialize()])
    })
}

pub fn vm_selectable(child: Shape) -> Shape {
    thunk(move || {
        ReactiveViewModel::from_widget("selectable", std::collections::HashMap::new())
            .with_children(vec![child.materialize()])
    })
}

pub fn vm_pie_menu(child: Shape) -> Shape {
    thunk(move || {
        let mut props = std::collections::HashMap::new();
        props.insert("fields".to_string(), Value::String("[]".to_string()));
        ReactiveViewModel::from_widget("pie_menu", props).with_children(vec![child.materialize()])
    })
}

pub fn vm_view_mode_switcher(child: Shape) -> Shape {
    vm_view_mode_switcher_for(child, "block:pbt-vms".to_string(), vec!["list".to_string()])
}

/// Build a `ViewModeSwitcher` whose `entity_uri` and mode list are caller-
/// supplied. Used by `bp_view_mode_switcher` when the child is a
/// mode-switchable `LiveBlock`: the VMS must target the *same* block id
/// as the LiveBlock so clicking a VMS mode button routes
/// the VMS click handler to the same registry entry the scenario's
/// `SwitchViewMode` action names.
pub fn vm_view_mode_switcher_for(
    child: Shape,
    entity_uri: String,
    mode_names: Vec<String>,
) -> Shape {
    let modes_json = {
        let parts: Vec<String> = mode_names
            .iter()
            .map(|n| format!(r#"{{"name":"{n}","icon":"list"}}"#))
            .collect();
        format!("[{}]", parts.join(","))
    };
    let default_mode = mode_names
        .first()
        .cloned()
        .unwrap_or_else(|| "tree".to_string());
    thunk(move || {
        let mut props = std::collections::HashMap::new();
        props.insert("entity_uri".to_string(), Value::String(entity_uri.clone()));
        props.insert("modes".to_string(), Value::String(modes_json.clone()));
        ReactiveViewModel {
            slot: Some(holon_frontend::ReactiveSlot::new(child.materialize())),
            ..ReactiveViewModel::from_widget("view_mode_switcher", props)
        }
    })
}

pub fn vm_drawer(block_id: impl Into<String>, child: Shape) -> Shape {
    vm_drawer_with_mode(
        block_id,
        holon_frontend::view_model::DrawerMode::Shrink,
        child,
    )
}

pub fn vm_drawer_overlay(block_id: impl Into<String>, child: Shape) -> Shape {
    vm_drawer_with_mode(
        block_id,
        holon_frontend::view_model::DrawerMode::Overlay,
        child,
    )
}

pub fn vm_drawer_with_mode(
    block_id: impl Into<String>,
    mode: holon_frontend::view_model::DrawerMode,
    child: Shape,
) -> Shape {
    let id = block_id.into();
    thunk(move || {
        ReactiveViewModel::drawer(
            id.clone(),
            mode,
            holon_frontend::DEFAULT_DRAWER_WIDTH,
            child.materialize(),
        )
    })
}

pub fn vm_reactive<F>(n: usize, layout: CollectionVariant, mk_item: F) -> Shape
where
    F: Fn(usize) -> ReactiveViewModel + Send + Sync + 'static,
{
    thunk(move || {
        let items: Vec<ReactiveViewModel> = (0..n).map(&mk_item).collect();
        ReactiveViewModel::static_collection(layout_name(&layout), items, layout_gap(&layout))
    })
}

fn layout_name(layout: &CollectionVariant) -> &str {
    layout.name()
}

fn layout_gap(layout: &CollectionVariant) -> f32 {
    layout.gap
}

pub fn vm_reactive_text_items(n: usize, layout: CollectionVariant) -> Shape {
    vm_reactive(n, layout, |i| ReactiveViewModel::text(format!("item {i}")))
}

pub fn vm_reactive_list_of(n: usize, template: Shape) -> Shape {
    vm_reactive(n, CollectionVariant::list(0.0), move |_| {
        template.materialize()
    })
}

pub fn vm_columns(children: Vec<Shape>) -> Shape {
    thunk(move || {
        let items: Vec<ReactiveViewModel> = children.iter().map(|c| c.materialize()).collect();
        ReactiveViewModel::static_collection("columns", items, 4.0)
    })
}

// ── Data source helpers ───────────────────────────────────────────────────

pub fn populate_data_source(data_source: &Arc<ReactiveQueryResults>, n: usize) {
    data_source.set_generation(0);
    for i in 0..n {
        let id = format!("pbt-row-{i}");
        let mut data: std::collections::HashMap<String, Value> = std::collections::HashMap::new();
        data.insert("id".to_string(), Value::String(id.clone()));
        data.insert("content".to_string(), Value::String(format!("row {i}")));
        data.insert("sequence".to_string(), Value::Integer(i as i64));
        let enriched = EnrichedRow::from_raw(data, |_| std::collections::HashMap::new());
        data_source.apply_change(
            Change::Created {
                data: enriched,
                origin: ChangeOrigin::Local {
                    operation_id: None,
                    trace_id: None,
                },
            },
            0,
        );
    }
}

fn text_item_template_expr() -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "text".to_string(),
        args: vec![Arg {
            name: Some("content".to_string()),
            value: RenderExpr::Literal {
                value: Value::String("row".to_string()),
            },
        }],
    }
}

fn collection_expr(widget: &str) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: widget.to_string(),
        args: vec![
            Arg {
                name: Some("gap".to_string()),
                value: RenderExpr::Literal {
                    value: Value::Float(0.0),
                },
            },
            Arg {
                name: Some("item_template".to_string()),
                value: text_item_template_expr(),
            },
        ],
    }
}

pub fn vm_shared_collection(
    services: Arc<StubBuilderServices>,
    data_source: Arc<ReactiveQueryResults>,
    render_expr: RenderExpr,
) -> Shape {
    Shape(Arc::new(move || {
        let ctx = RenderContext {
            data_rows: Vec::new(),
            data_source: Some(data_source.clone()),
            ..Default::default()
        };
        let tree = services.interpret(&render_expr, &ctx);
        if let Some(ref view) = tree.collection {
            let rows: Vec<Arc<DataRow>> = data_source.snapshot().1;
            let layout = view.layout();
            let is_tree_variant = layout
                .as_ref()
                .map(|v| v.is_hierarchical())
                .unwrap_or(false);
            let tmpl = text_item_template_expr();
            let items: Vec<Arc<ReactiveViewModel>> = rows
                .iter()
                .map(|row| {
                    let row_ctx = RenderContext {
                        data_rows: vec![row.clone()],
                        data_source: None,
                        ..Default::default()
                    };
                    let leaf = services.interpret(&tmpl, &row_ctx);
                    let wrapped = if is_tree_variant {
                        ReactiveViewModel::tree_item(leaf, 0, false)
                    } else {
                        leaf
                    };
                    Arc::new(wrapped)
                })
                .collect();
            view.items.lock_mut().replace_cloned(items);
        }
        tree
    }))
}

// ── Blueprint constructors ────────────────────────────────────────────────

pub fn bp_col(children: Vec<Blueprint>) -> Blueprint {
    Blueprint::with_children(children, vm_col)
}
pub fn bp_row(children: Vec<Blueprint>) -> Blueprint {
    Blueprint::with_children(children, vm_row)
}
pub fn bp_card(children: Vec<Blueprint>) -> Blueprint {
    Blueprint::with_children(children, vm_card)
}
pub fn bp_chat_bubble(sender: String, time: String, children: Vec<Blueprint>) -> Blueprint {
    Blueprint::with_children(children, move |shapes| {
        vm_chat_bubble(sender.clone(), time.clone(), shapes)
    })
}
pub fn bp_focusable(child: Blueprint) -> Blueprint {
    child.map_shape(vm_focusable)
}
pub fn bp_draggable(child: Blueprint) -> Blueprint {
    child.map_shape(vm_draggable)
}
pub fn bp_selectable(child: Blueprint) -> Blueprint {
    child.map_shape(vm_selectable)
}
pub fn bp_pie_menu(child: Blueprint) -> Blueprint {
    child.map_shape(vm_pie_menu)
}
pub fn bp_view_mode_switcher(child: Blueprint) -> Blueprint {
    // If the child is a mode-switchable `LiveBlock` (exactly one handle),
    // build the VMS with the same entity_uri and mode list as the handle,
    // so clicking a VMS mode button routes the click handler back
    // to the registry entry the scenario's `SwitchViewMode` action
    // targets. Otherwise fall back to a cosmetic VMS with a single
    // hardcoded mode (no action will ever target it).
    if child.handles.len() == 1 {
        let handle = child.handles[0].clone();
        let entity_uri = handle.block_id.clone();
        let mode_names = handle.mode_names.clone();
        let shape = child.shape.clone();
        let new_shape = vm_view_mode_switcher_for(shape, entity_uri, mode_names);
        Blueprint {
            shape: new_shape,
            handles: child.handles,
            drawers: child.drawers,
        }
    } else {
        child.map_shape(vm_view_mode_switcher)
    }
}
/// Wrap `child` in a Shrink drawer with an auto-minted unique block_id,
/// and record that drawer in `Blueprint.drawers` so proptest actions can
/// target it with `ToggleDrawer`.
pub fn bp_drawer(child: Blueprint) -> Blueprint {
    bp_drawer_with_id(mint_drawer_id(), child)
}

/// Wrap `child` in an Overlay drawer with an auto-minted unique block_id,
/// tracking it for toggle actions.
pub fn bp_drawer_overlay(child: Blueprint) -> Blueprint {
    bp_drawer_overlay_with_id(mint_drawer_id(), child)
}

/// Shrink drawer with a caller-supplied id. Useful for targeted tests
/// that need stable ids across runs. Still records the drawer handle.
pub fn bp_drawer_with_id(block_id: impl Into<String>, child: Blueprint) -> Blueprint {
    let id = block_id.into();
    let id_for_shape = id.clone();
    let mut bp = child.map_shape(move |s| vm_drawer(id_for_shape, s));
    for h in &mut bp.handles {
        h.in_drawer = true;
    }
    bp.drawers.push(DrawerHandle { block_id: id });
    bp
}

/// Overlay drawer with a caller-supplied id.
pub fn bp_drawer_overlay_with_id(block_id: impl Into<String>, child: Blueprint) -> Blueprint {
    let id = block_id.into();
    let id_for_shape = id.clone();
    let mut bp = child.map_shape(move |s| vm_drawer_overlay(id_for_shape, s));
    for h in &mut bp.handles {
        h.in_drawer = true;
    }
    bp.drawers.push(DrawerHandle { block_id: id });
    bp
}

pub fn bp_columns(children: Vec<Blueprint>) -> Blueprint {
    Blueprint::with_children(children, vm_columns)
}

/// Mint a unique synthetic block id per call.
pub fn mint_block_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!("pbt-live-block-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

/// Mint a unique synthetic drawer block_id per call. Separate counter
/// from `mint_block_id` so drawer ids are easy to spot in failure dumps.
pub fn mint_drawer_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!("pbt-drawer-{}", SEQ.fetch_add(1, Ordering::Relaxed))
}

pub fn bp_live_block_with_modes(modes: Vec<(String, Blueprint)>) -> Blueprint {
    assert!(
        !modes.is_empty(),
        "bp_live_block_with_modes requires at least one mode"
    );

    let raw = mint_block_id();
    let uri = EntityUri::from_raw(&raw);
    let block_id = uri.to_string();

    let nested_handles: Vec<BlockHandle> = modes
        .iter()
        .flat_map(|(_, bp)| bp.handles.iter().cloned())
        .collect();

    let mode_names: Vec<String> = modes.iter().map(|(n, _)| n.clone()).collect();
    let mode_thunks: Vec<BlockTreeThunk> = modes
        .into_iter()
        .map(|(_, bp)| {
            let shape = bp.shape;
            Arc::new(move || shape.materialize()) as BlockTreeThunk
        })
        .collect();

    let mut handles = nested_handles;
    handles.push(BlockHandle {
        block_id: block_id.clone(),
        mode_names,
        mode_thunks,
        in_drawer: false,
        initial_mode: 0,
    });

    let shape_block_id = block_id.clone();
    let shape = Shape(Arc::new(move || {
        ReactiveViewModel::live_block(EntityUri::from_raw(&shape_block_id))
    }));

    Blueprint {
        shape,
        handles,
        drawers: vec![],
    }
}

// ── Proptest strategies ───────────────────────────────────────────────────

pub fn arb_static_leaf() -> BoxedStrategy<Blueprint> {
    prop_oneof![
        "[a-zA-Z]{1,15}".prop_map(|s| Blueprint::leaf(vm_text(s))),
        "[A-Z]{1,6}".prop_map(|s| Blueprint::leaf(vm_badge(s))),
        Just(()).prop_map(|_| Blueprint::leaf(vm_icon())),
        any::<bool>().prop_map(|b| Blueprint::leaf(vm_checkbox(b))),
        (1.0f32..40.0, 1.0f32..40.0).prop_map(|(w, h)| Blueprint::leaf(vm_spacer(w, h))),
    ]
    .boxed()
}

pub fn arb_row_template_shape() -> BoxedStrategy<Shape> {
    let leaf = prop_oneof![
        "[a-zA-Z]{1,15}".prop_map(vm_text),
        "[A-Z]{1,6}".prop_map(vm_badge),
        Just(()).prop_map(|_| vm_icon()),
        any::<bool>().prop_map(vm_checkbox),
        (1.0f32..40.0, 1.0f32..40.0).prop_map(|(w, h)| vm_spacer(w, h)),
    ]
    .boxed();
    leaf.prop_recursive(2, 8, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 1..=3).prop_map(vm_row),
            prop::collection::vec(inner.clone(), 1..=3).prop_map(vm_col),
            prop::collection::vec(inner.clone(), 1..=2).prop_map(vm_card),
            inner.clone().prop_map(vm_focusable),
            inner.clone().prop_map(vm_draggable),
            inner.clone().prop_map(vm_selectable),
            inner.prop_map(vm_pie_menu),
        ]
    })
    .boxed()
}

pub fn arb_reactive_collection() -> BoxedStrategy<Blueprint> {
    prop_oneof![
        Just(Blueprint::leaf(vm_reactive_text_items(
            0,
            CollectionVariant::list(0.0)
        ))),
        Just(Blueprint::leaf(vm_reactive_text_items(
            0,
            CollectionVariant::tree()
        ))),
        Just(Blueprint::leaf(vm_reactive_text_items(
            0,
            CollectionVariant::outline()
        ))),
        Just(Blueprint::leaf(vm_reactive_text_items(
            0,
            CollectionVariant::table()
        ))),
        (1usize..=5)
            .prop_map(|n| Blueprint::leaf(vm_reactive_text_items(n, CollectionVariant::list(0.0)))),
        (1usize..=5)
            .prop_map(|n| Blueprint::leaf(vm_reactive_text_items(n, CollectionVariant::tree()))),
        (1usize..=5)
            .prop_map(|n| Blueprint::leaf(vm_reactive_text_items(n, CollectionVariant::outline()))),
        (1usize..=5)
            .prop_map(|n| Blueprint::leaf(vm_reactive_text_items(n, CollectionVariant::table()))),
        (20usize..=60)
            .prop_map(|n| Blueprint::leaf(vm_reactive_text_items(n, CollectionVariant::list(0.0)))),
        (20usize..=60)
            .prop_map(|n| Blueprint::leaf(vm_reactive_text_items(n, CollectionVariant::tree()))),
        (1usize..=8, arb_row_template_shape())
            .prop_map(|(n, tmpl)| Blueprint::leaf(vm_reactive_list_of(n, tmpl))),
    ]
    .boxed()
}

pub fn arb_wrapped_collection() -> BoxedStrategy<Blueprint> {
    let inner = arb_reactive_collection()
        .prop_recursive(2, 6, 1, |c| {
            prop_oneof![
                c.clone().prop_map(bp_focusable),
                c.clone().prop_map(bp_draggable),
                c.clone().prop_map(bp_selectable),
                c.prop_map(bp_pie_menu),
            ]
        })
        .boxed();

    prop_oneof![
        inner.clone(),
        inner.clone().prop_map(bp_drawer),
        inner.clone().prop_map(bp_drawer_overlay),
        inner.clone().prop_map(bp_view_mode_switcher),
        inner.prop_map(|c| { bp_live_block_with_modes(vec![("only".to_string(), c)]) }),
        (1usize..=4, 6usize..=12).prop_map(|(n_a, n_b)| {
            let mode_a = Blueprint::leaf(vm_reactive_text_items(n_a, CollectionVariant::list(0.0)));
            let mode_b = Blueprint::leaf(vm_reactive_text_items(n_b, CollectionVariant::list(0.0)));
            bp_view_mode_switcher(bp_live_block_with_modes(vec![
                ("mode-a".to_string(), mode_a),
                ("mode-b".to_string(), mode_b),
            ]))
        }),
        (3usize..=8,).prop_map(|(n_rows,)| {
            let data_source = Arc::new(ReactiveQueryResults::new());
            populate_data_source(&data_source, n_rows);
            let services: Arc<StubBuilderServices> = Arc::new(StubBuilderServices::new());
            let mode_a = Blueprint::leaf(vm_shared_collection(
                services.clone(),
                data_source.clone(),
                collection_expr("list"),
            ));
            let mode_b = Blueprint::leaf(vm_shared_collection(
                services,
                data_source,
                collection_expr("tree"),
            ));
            bp_view_mode_switcher(bp_live_block_with_modes(vec![
                ("mode-a".to_string(), mode_a),
                ("mode-b".to_string(), mode_b),
            ]))
        }),
    ]
    .boxed()
}

pub fn arb_static_tree() -> BoxedStrategy<Blueprint> {
    arb_static_leaf()
        .prop_recursive(3, 32, 4, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 1..=4).prop_map(bp_col),
                prop::collection::vec(inner.clone(), 1..=4).prop_map(bp_row),
                prop::collection::vec(inner.clone(), 1..=3).prop_map(bp_card),
                (
                    prop::sample::select(vec![
                        "user".to_string(),
                        "assistant".to_string(),
                        "system".to_string(),
                    ]),
                    prop::collection::vec(inner.clone(), 1..=3),
                )
                    .prop_map(|(sender, children)| {
                        bp_chat_bubble(sender, "12:34".to_string(), children)
                    }),
                inner.clone().prop_map(bp_focusable),
                inner.clone().prop_map(bp_draggable),
                inner.clone().prop_map(bp_selectable),
                inner.prop_map(bp_pie_menu),
            ]
        })
        .boxed()
}

fn make_chat_bubble_content(block_id: &str, n_items: usize) -> Shape {
    let bid = block_id.to_string();
    let n = n_items;
    Shape(Arc::new(move || {
        let bubbles: Vec<ReactiveViewModel> = (0..n)
            .map(|j| make_chat_bubble_vm("user", &format!("{j}:00"), &format!("msg-{bid}-{j}")))
            .collect();
        ReactiveViewModel::layout("column", bubbles)
    }))
}

fn make_chat_bubble_vm(sender: &str, time: &str, text: &str) -> ReactiveViewModel {
    let mut props = std::collections::HashMap::new();
    props.insert("sender".to_string(), Value::String(sender.to_string()));
    props.insert("time".to_string(), Value::String(time.to_string()));
    ReactiveViewModel {
        children: vec![Arc::new(ReactiveViewModel::text(text))],
        ..ReactiveViewModel::from_widget("chat_bubble", props)
    }
}

/// Generate a tree collection where some items are live_blocks resolving to
/// lists of chat_bubbles. Reproduces the production shape:
///   tree → tree_item → live_block → list[chat_bubble, chat_bubble, ...]
///
/// Live_blocks are registered with two modes: "empty" (spacer placeholder)
/// and "loaded" (real chat_bubble content). When `deferred=false`, blocks
/// start in "loaded" mode (synchronous). When `deferred=true`, blocks start
/// in "empty" mode and `DeliverBlockContent` actions switch to "loaded" —
/// reproducing async data arrival.
pub fn arb_tree_with_live_block_items() -> BoxedStrategy<Blueprint> {
    arb_tree_with_live_block_items_inner(false)
}

/// Same as `arb_tree_with_live_block_items` but blocks start empty and
/// require `DeliverBlockContent` actions to populate.
pub fn arb_tree_with_deferred_live_block_items() -> BoxedStrategy<Blueprint> {
    arb_tree_with_live_block_items_inner(true)
}

/// Handle for pushing deferred data into a streaming collection after mount.
/// Each entry: (block_id, MutableVec to push into, items to push).
pub type DeferredDataHandles = Vec<(
    String,
    MutableVec<Arc<ReactiveViewModel>>,
    Vec<Arc<ReactiveViewModel>>,
)>;

/// Build a deterministic tree with `n_refs` live_blocks whose "loaded" mode
/// uses **streaming** collections (empty MutableVec) instead of static
/// pre-populated ones. Returns both the blueprint and handles for pushing
/// data after mount.
///
/// This reproduces the production path where:
/// 1. Structural change arrives → live_block gets a Reactive collection tree
/// 2. Collection starts empty (tokio driver hasn't pushed data yet)
/// 3. GPUI renders → live_block at zero height
/// 4. Data arrives (push into MutableVec) → subscribe_inner_collections fires
/// 5. Re-render → live_block should expand to content height
pub fn make_streaming_live_block_fixture(
    n_refs: usize,
    items_per_ref: usize,
) -> (Blueprint, DeferredDataHandles) {
    let mut all_items: Vec<ReactiveViewModel> = Vec::new();
    let mut handles: Vec<BlockHandle> = Vec::new();
    let mut deferred_data: DeferredDataHandles = Vec::new();

    for i in 0..3 {
        let text = ReactiveViewModel::text(format!("heading {i}"));
        all_items.push(ReactiveViewModel::tree_item(text, 0, false));
    }

    for _ in 0..n_refs {
        let raw_id = mint_block_id();
        let uri = EntityUri::from_raw(&raw_id);
        let block_id = uri.to_string();

        // "empty" mode: empty collection (same as production loading state)
        let empty_shape: Shape = Shape(Arc::new(|| {
            let view = Arc::new(ReactiveView::new_static_with_layout(
                vec![],
                CollectionVariant::list(0.0),
            ));
            ReactiveViewModel {
                collection: Some(view),
                ..ReactiveViewModel::from_widget("list", std::collections::HashMap::new())
            }
        }));

        // "loaded" mode: streaming collection (empty MutableVec + deferred data)
        // Create the ReactiveView ONCE so the MutableVec is shared across calls.
        let streaming_view = Arc::new(ReactiveView::new_static_with_layout(
            vec![],
            CollectionVariant::list(4.0),
        ));
        let items_handle = streaming_view.items.clone();
        let streaming_view_for_thunk = streaming_view.clone();

        let bid = block_id.clone();
        let items_to_push: Vec<Arc<ReactiveViewModel>> = (0..items_per_ref)
            .map(|j| {
                Arc::new(make_chat_bubble_vm(
                    "user",
                    &format!("{j}:00"),
                    &format!("msg-{bid}-{j}"),
                ))
            })
            .collect();

        deferred_data.push((block_id.clone(), items_handle, items_to_push));

        handles.push(BlockHandle {
            block_id: block_id.clone(),
            mode_names: vec!["empty".to_string(), "loaded".to_string()],
            mode_thunks: vec![
                Arc::new(move || empty_shape.materialize()) as BlockTreeThunk,
                Arc::new(move || ReactiveViewModel {
                    collection: Some(streaming_view_for_thunk.clone()),
                    ..ReactiveViewModel::from_widget("list", std::collections::HashMap::new())
                }) as BlockTreeThunk,
            ],
            in_drawer: false,
            initial_mode: 0,
        });

        let block_ref = ReactiveViewModel::live_block(uri);
        all_items.push(ReactiveViewModel::tree_item(block_ref, 0, true));
    }

    let view = Arc::new(ReactiveView::new_static_with_layout(
        all_items,
        CollectionVariant::tree(),
    ));
    let shape = Shape(Arc::new(move || ReactiveViewModel {
        collection: Some(view.clone()),
        ..ReactiveViewModel::from_widget("tree", std::collections::HashMap::new())
    }));

    let bp = Blueprint {
        shape,
        handles,
        drawers: vec![],
    };
    (bp, deferred_data)
}

fn arb_tree_with_live_block_items_inner(deferred: bool) -> BoxedStrategy<Blueprint> {
    (1usize..=3, 1usize..=5)
        .prop_map(move |(n_refs, items_per_ref)| {
            let mut all_items: Vec<ReactiveViewModel> = Vec::new();
            let mut handles: Vec<BlockHandle> = Vec::new();

            for i in 0..3 {
                let text = ReactiveViewModel::text(format!("heading {i}"));
                all_items.push(ReactiveViewModel::tree_item(text, 0, false));
            }

            for _ in 0..n_refs {
                let raw_id = mint_block_id();
                let uri = EntityUri::from_raw(&raw_id);
                let block_id = uri.to_string();

                let content_shape = make_chat_bubble_content(&block_id, items_per_ref);
                // Production's "loading" state: watcher delivered Structure
                // (render_expr = list) but no Data events yet → empty collection
                // with zero items → zero intrinsic height.
                let empty_shape: Shape = Shape(Arc::new(|| {
                    let view = Arc::new(ReactiveView::new_static_with_layout(
                        vec![],
                        CollectionVariant::list(0.0),
                    ));
                    ReactiveViewModel {
                        collection: Some(view),
                        ..ReactiveViewModel::from_widget("list", std::collections::HashMap::new())
                    }
                }));

                let initial_mode: usize = if deferred { 0 } else { 1 };
                handles.push(BlockHandle {
                    block_id: block_id.clone(),
                    mode_names: vec!["empty".to_string(), "loaded".to_string()],
                    mode_thunks: vec![
                        Arc::new(move || empty_shape.materialize()) as BlockTreeThunk,
                        Arc::new(move || content_shape.materialize()) as BlockTreeThunk,
                    ],
                    in_drawer: false,
                    initial_mode,
                });

                let block_ref = ReactiveViewModel::live_block(uri);
                all_items.push(ReactiveViewModel::tree_item(block_ref, 0, true));
            }

            let view = Arc::new(ReactiveView::new_static_with_layout(
                all_items,
                CollectionVariant::tree(),
            ));
            let shape = Shape(Arc::new(move || ReactiveViewModel {
                collection: Some(view.clone()),
                ..ReactiveViewModel::from_widget("tree", std::collections::HashMap::new())
            }));

            Blueprint {
                shape,
                handles,
                drawers: vec![],
            }
        })
        .boxed()
}

/// Like `arb_tree_with_live_block_items_inner(true)` but places enough plain
/// tree_items BEFORE the live_blocks to push them below the 600px viewport.
/// Each plain heading row is ~32px. 20 headings ≈ 640px, so the live_block
/// items start off-screen.
///
/// This tests whether the GPUI list re-measures off-screen rows when their
/// content height changes — the production bug scenario where `list()`
/// caches the initial zero/small height and never re-measures.
pub fn arb_tree_with_offscreen_live_blocks() -> BoxedStrategy<Blueprint> {
    (1usize..=3, 1usize..=5)
        .prop_map(|(n_refs, items_per_ref)| {
            let mut all_items: Vec<ReactiveViewModel> = Vec::new();
            let mut handles: Vec<BlockHandle> = Vec::new();

            // Push 25 plain headings to fill more than the viewport
            for i in 0..25 {
                let text = ReactiveViewModel::text(format!("heading {i}"));
                all_items.push(ReactiveViewModel::tree_item(text, 0, false));
            }

            // Add live_block items (will be off-screen initially)
            for _ in 0..n_refs {
                let raw_id = mint_block_id();
                let uri = EntityUri::from_raw(&raw_id);
                let block_id = uri.to_string();

                let content_shape = make_chat_bubble_content(&block_id, items_per_ref);
                // Production's "loading" state: watcher delivered Structure
                // (render_expr = list) but no Data events yet → empty collection
                // with zero items → zero intrinsic height.
                let empty_shape: Shape = Shape(Arc::new(|| {
                    let view = Arc::new(ReactiveView::new_static_with_layout(
                        vec![],
                        CollectionVariant::list(0.0),
                    ));
                    ReactiveViewModel {
                        collection: Some(view),
                        ..ReactiveViewModel::from_widget("list", std::collections::HashMap::new())
                    }
                }));

                handles.push(BlockHandle {
                    block_id: block_id.clone(),
                    mode_names: vec!["empty".to_string(), "loaded".to_string()],
                    mode_thunks: vec![
                        Arc::new(move || empty_shape.materialize()) as BlockTreeThunk,
                        Arc::new(move || content_shape.materialize()) as BlockTreeThunk,
                    ],
                    in_drawer: false,
                    initial_mode: 0, // always start empty (deferred)
                });

                let block_ref = ReactiveViewModel::live_block(uri);
                all_items.push(ReactiveViewModel::tree_item(block_ref, 0, true));
            }

            let view = Arc::new(ReactiveView::new_static_with_layout(
                all_items,
                CollectionVariant::tree(),
            ));
            let shape = Shape(Arc::new(move || ReactiveViewModel {
                collection: Some(view.clone()),
                ..ReactiveViewModel::from_widget("tree", std::collections::HashMap::new())
            }));

            Blueprint {
                shape,
                handles,
                drawers: vec![],
            }
        })
        .boxed()
}

pub fn arb_columns_plain() -> BoxedStrategy<Blueprint> {
    prop::collection::vec(arb_wrapped_collection(), 2..=2)
        .prop_map(bp_columns)
        .boxed()
}

pub fn arb_columns_with_sidebars() -> BoxedStrategy<Blueprint> {
    let wrapped = arb_wrapped_collection();
    (wrapped.clone(), wrapped.clone(), wrapped)
        .prop_map(|(l, m, r)| bp_columns(vec![bp_drawer(l), m, bp_drawer(r)]))
        .boxed()
}

pub fn arb_columns_with_overlay_sidebars() -> BoxedStrategy<Blueprint> {
    let wrapped = arb_wrapped_collection();
    (wrapped.clone(), wrapped.clone(), wrapped)
        .prop_map(|(l, m, r)| bp_columns(vec![bp_drawer_overlay(l), m, bp_drawer_overlay(r)]))
        .boxed()
}

pub fn arb_blueprint() -> BoxedStrategy<Blueprint> {
    prop_oneof![
        arb_static_tree(),
        arb_reactive_collection(),
        arb_wrapped_collection(),
        arb_columns_plain(),
        arb_columns_with_sidebars(),
        arb_columns_with_overlay_sidebars(),
        arb_tree_with_live_block_items(),
    ]
    .boxed()
}

/// Generate a random `UiInteraction` — either a mode switch against a
/// switchable handle, or a drawer toggle against a known drawer. The
/// caller pre-filters `switchable` to handles with ≥2 modes; `drawers`
/// may be empty. Panics if both are empty — callers must check first.
pub fn arb_action(
    switchable: Arc<Vec<BlockHandle>>,
    drawers: Arc<Vec<DrawerHandle>>,
) -> BoxedStrategy<UiInteraction> {
    let has_switchable = !switchable.is_empty();
    let has_drawers = !drawers.is_empty();
    assert!(
        has_switchable || has_drawers,
        "arb_action requires at least one switchable handle or drawer"
    );

    let switch_strat: Option<BoxedStrategy<UiInteraction>> = if has_switchable {
        let len = switchable.len();
        let sw = switchable.clone();
        Some(
            (0..len)
                .prop_flat_map(move |i| {
                    let sw = sw.clone();
                    let num_modes = sw[i].mode_names.len();
                    (Just(i), 0..num_modes).prop_map(move |(i, m)| UiInteraction::SwitchViewMode {
                        block_id: sw[i].block_id.clone(),
                        target_mode: sw[i].mode_names[m].clone(),
                    })
                })
                .boxed(),
        )
    } else {
        None
    };

    let toggle_strat: Option<BoxedStrategy<UiInteraction>> = if has_drawers {
        let dr = drawers.clone();
        let len = dr.len();
        Some(
            (0..len)
                .prop_map(move |i| UiInteraction::ToggleDrawer {
                    block_id: dr[i].block_id.clone(),
                })
                .boxed(),
        )
    } else {
        None
    };

    match (switch_strat, toggle_strat) {
        (Some(s), Some(t)) => prop_oneof![s, t].boxed(),
        (Some(s), None) => s,
        (None, Some(t)) => t,
        (None, None) => unreachable!("checked above"),
    }
}

pub fn arb_scenario() -> BoxedStrategy<Scenario> {
    prop_oneof![
        arb_scenario_from_blueprint(arb_blueprint()),
        arb_deferred_live_block_scenario(),
    ]
    .boxed()
}

fn arb_scenario_from_blueprint(bp_strat: BoxedStrategy<Blueprint>) -> BoxedStrategy<Scenario> {
    bp_strat
        .prop_flat_map(|bp| {
            let switchable: Vec<BlockHandle> = bp
                .handles
                .iter()
                .filter(|h| {
                    h.mode_names.len() >= 2
                        && !h.in_drawer
                        && !h.mode_names.contains(&"empty".to_string())
                })
                .cloned()
                .collect();
            let drawers: Vec<DrawerHandle> = bp.drawers.clone();

            let actions_strat: BoxedStrategy<Vec<UiInteraction>> =
                if switchable.is_empty() && drawers.is_empty() {
                    Just(Vec::new()).boxed()
                } else {
                    let switchable = Arc::new(switchable);
                    let drawers = Arc::new(drawers);
                    prop::collection::vec(arb_action(switchable.clone(), drawers.clone()), 0..=5)
                        .boxed()
                };

            (Just(bp), actions_strat)
                .prop_map(|(blueprint, actions)| Scenario { blueprint, actions })
        })
        .boxed()
}

/// Scenario that mounts a tree with empty live_blocks, then delivers content
/// via `DeliverBlockContent` actions. Reproduces async data arrival timing.
fn arb_deferred_live_block_scenario() -> BoxedStrategy<Scenario> {
    arb_tree_with_deferred_live_block_items()
        .prop_map(|bp| {
            let actions: Vec<UiInteraction> = bp
                .handles
                .iter()
                .map(|h| UiInteraction::DeliverBlockContent {
                    block_id: h.block_id.clone(),
                })
                .collect();
            Scenario {
                blueprint: bp,
                actions,
            }
        })
        .boxed()
}
