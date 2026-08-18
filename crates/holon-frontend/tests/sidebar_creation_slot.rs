//! The trailing "type here to create" slot is a MAIN-PANEL affordance that a
//! collection OPTS INTO with `creation_slot: true`
//! (assets/default/types/collection_profile.yaml). A read-only navigation tree
//! — the left sidebar's page list, the right sidebar's outline mirror — omits
//! the flag and must therefore render NO creation-placeholder row.
//!
//! Martin's live vault (2026-07-30, task #33) showed the opposite: 27 sidebar
//! rows for 26 pages, the extra one an empty `block:__virtual:<page>` bullet
//! nested under its parent, which then desynced the tree provider from its
//! `row_map` on every disclosure toggle and produced a breadcrumb red banner.
//!
//! Run: `cargo nextest run -p holon-frontend --test sidebar_creation_slot`

use std::collections::HashMap;
use std::sync::Arc;

use holon_api::EntityUri;
use holon_api::Value;
use holon_api::entity_profile::VirtualChildConfig;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::ReactiveViewModel;
use holon_frontend::RenderContext;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::render_interpreter::RenderInterpreter;

/// The left sidebar's tree, verbatim in shape from `assets/default/index.org`
/// (`left_sidebar::render::0`) with the item template reduced to its text leaf:
/// no `creation_slot`, no `virtual_parent`.
const SIDEBAR_TREE: &str = r#"tree(#{parent_id: col("parent_id"), sortkey: col("content"), item_template: text(col("content"))})"#;

/// The main panel's tree, verbatim in shape from the `tree_view` variant of
/// `assets/default/types/collection_profile.yaml`: it OPTS IN.
const MAIN_PANEL_TREE: &str = r#"tree(#{parent_id: col("parent_id"), sortkey: col("sort_key"), item_template: text(col("content")), creation_slot: true, virtual_parent: true})"#;

/// Services that publish a `block` virtual-child config — the profile fact that
/// makes a creation slot constructible at all. `StubBuilderServices` reports
/// `None`, under which no slot can appear and the test could never go red.
struct SlotCapableServices {
    interpreter: Arc<RenderInterpreter<ReactiveViewModel>>,
    rt_handle: tokio::runtime::Handle,
    link_classifier: holon_api::link_parser::LinkTargetClassifier,
}

impl SlotCapableServices {
    fn new() -> Self {
        Self {
            interpreter: Arc::new(holon_frontend::shadow_builders::build_shadow_interpreter()),
            rt_handle: tokio::runtime::Handle::try_current()
                .unwrap_or_else(|_| RUNTIME.handle().clone()),
            link_classifier: holon_api::link_parser::LinkTargetClassifier::default(),
        }
    }
}

static RUNTIME: std::sync::LazyLock<tokio::runtime::Runtime> = std::sync::LazyLock::new(|| {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
});

impl BuilderServices for SlotCapableServices {
    fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext) -> ReactiveViewModel {
        self.interpreter.interpret(expr, ctx, self)
    }

    /// All state here is the shared interpreter and a default classifier, so a
    /// handle is a second instance over the same interpreter.
    fn clone_arc(&self) -> Arc<dyn BuilderServices> {
        Arc::new(Self {
            interpreter: self.interpreter.clone(),
            rt_handle: self.rt_handle.clone(),
            link_classifier: holon_api::link_parser::LinkTargetClassifier::default(),
        })
    }

    fn link_classifier(&self) -> &holon_api::link_parser::LinkTargetClassifier {
        &self.link_classifier
    }

    fn virtual_child_config(&self, entity_name: &str) -> Option<VirtualChildConfig> {
        (entity_name == "block").then(|| VirtualChildConfig {
            defaults: HashMap::from([("content".to_string(), Value::String(String::new()))]),
        })
    }

    fn get_block_data(&self, _: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        (
            RenderExpr::FunctionCall {
                name: "table".into(),
                args: vec![],
            },
            vec![],
        )
    }

    fn resolve_profile(&self, _: &DataRow) -> Option<holon_api::RenderProfile> {
        None
    }

    fn watch_query(
        &self,
        _: &str,
        _: holon_api::QueryLanguage,
        _: Option<holon_frontend::QueryContext>,
    ) -> anyhow::Result<holon_api::EnrichedChangeStream> {
        anyhow::bail!("SlotCapableServices renders the snapshot path only")
    }

    fn widget_state(&self, _: &str) -> holon_frontend::WidgetState {
        holon_frontend::WidgetState::default()
    }

    fn set_widget_open(&self, _: &str, _: bool) {}

    fn set_widget_width(&self, _: &str, _: f32, _: bool) {}

    fn dispatch_intent(&self, _: holon_frontend::operations::OperationIntent) {}

    fn present_op(
        &self,
        _: holon_api::render_types::OperationDescriptor,
        _: HashMap<String, Value>,
    ) {
    }

    fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.rt_handle.clone()
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
        Box::pin(async { anyhow::bail!("not supported") })
    }
}

fn page(id: &str, parent: &str, content: &str) -> Arc<DataRow> {
    Arc::new(HashMap::from([
        ("id".to_string(), Value::String(id.to_string())),
        ("parent_id".to_string(), Value::String(parent.to_string())),
        ("content".to_string(), Value::String(content.to_string())),
        ("sort_key".to_string(), Value::String(content.to_string())),
    ]))
}

/// Martin's vault shape: top-level pages at the `no_parent` sentinel plus one
/// NESTED page (a subdirectory page-file), which is what makes the tree's
/// `virtual_parent` fallback resolve a page as the slot's parent.
fn sidebar_forest() -> Vec<Arc<DataRow>> {
    let sentinel = EntityUri::no_parent();
    vec![
        page("block:pageA", sentinel.as_str(), "Alpha"),
        page("block:pageB", sentinel.as_str(), "Beta"),
        page("block:pageC", "block:pageA", "Gamma"),
    ]
}

fn render(dsl: &str, rows: Vec<Arc<DataRow>>) -> Arc<ReactiveViewModel> {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr = holon_api::render_dsl::parse_render_dsl(dsl).expect("dsl should parse");
    let services = SlotCapableServices::new();
    let ctx = RenderContext::default().with_data_rows(rows);
    Arc::new(services.interpret(&expr, &ctx))
}

/// Every DISTINCT entity id reachable in the rendered view-model — through
/// `children`, through a collection's items, and through a slot — so the count
/// is "rendered rows", not "nodes carrying a row".
fn entity_ids(vm: &Arc<ReactiveViewModel>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut stack: Vec<Arc<ReactiveViewModel>> = vec![vm.clone()];
    while let Some(node) = stack.pop() {
        if let Some(id) = node.row_id() {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        for child in node.children.iter() {
            stack.push(child.clone());
        }
        if let Some(view) = node.collection.as_ref() {
            for item in view.children_snapshot() {
                stack.push(item);
            }
        }
        if let Some(slot) = node.slot.as_ref() {
            stack.push(slot.content.get_cloned());
        }
    }
    out.sort();
    out
}

fn creation_slot_ids(vm: &Arc<ReactiveViewModel>) -> Vec<String> {
    entity_ids(vm)
        .into_iter()
        .filter(|id| holon_frontend::RowOrigin::from_id(id).is_creation_placeholder())
        .collect()
}

#[test]
fn read_only_sidebar_tree_renders_no_creation_slot() {
    let vm = render(SIDEBAR_TREE, sidebar_forest());
    let slots = creation_slot_ids(&vm);
    assert!(
        slots.is_empty(),
        "the sidebar tree does not opt into `creation_slot`, so it must render no \
         creation-placeholder row; got {slots:?}"
    );
}

#[test]
fn read_only_sidebar_tree_row_count_equals_page_count() {
    let forest = sidebar_forest();
    let expected = forest.len();
    let vm = render(SIDEBAR_TREE, forest);
    let ids = entity_ids(&vm);
    assert_eq!(
        ids.len(),
        expected,
        "one rendered row per page, no phantom trailing row; got {ids:?}"
    );
}

/// The opt-in path must keep working — the main panel's trailing slot is how a
/// user creates the first child of a page.
#[test]
fn main_panel_tree_still_renders_its_creation_slot() {
    let vm = render(
        MAIN_PANEL_TREE,
        vec![
            page("block:page", "block:root-layout", "Page"),
            page("block:child", "block:page", "Child"),
        ],
    );
    assert_eq!(
        creation_slot_ids(&vm),
        vec!["block:__virtual:page".to_string()],
        "the opted-in main-panel tree keeps its trailing creation slot"
    );
}

/// The main panel's REAL item template: rows are editable.
const EDITABLE_MAIN_PANEL_TREE: &str = r#"tree(#{parent_id: col("parent_id"), sortkey: col("sort_key"), item_template: editable_text(col("content")), creation_slot: true, virtual_parent: true})"#;

/// Every widget name reachable from `vm`, paired with the row id in force at
/// that node (inherited from the nearest ancestor that carries one).
fn widgets_by_row(vm: &Arc<ReactiveViewModel>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut stack: Vec<(Arc<ReactiveViewModel>, String)> = vec![(vm.clone(), String::new())];
    while let Some((node, inherited)) = stack.pop() {
        let row = node.row_id().unwrap_or(inherited);
        if let Some(name) = node.widget_name() {
            out.push((row.clone(), name.to_string()));
        }
        for child in node.children.iter() {
            stack.push((child.clone(), row.clone()));
        }
        if let Some(view) = node.collection.as_ref() {
            for item in view.children_snapshot() {
                stack.push((item, row.clone()));
            }
        }
        if let Some(slot) = node.slot.as_ref() {
            stack.push((slot.content.get_cloned(), row.clone()));
        }
    }
    out
}

/// Ruling (C) + sub-ruling (B): the creation affordance is NOT a block, so it
/// mounts no editor. This is the structural guarantee behind every symptom the
/// ruling kills — with no `editable_text` there is no caret to sit in a
/// non-block, hence no breadcrumb lookup of an id with no path, no silently
/// swallowed indent, and no Enter that has to mean something special.
///
/// Red before the fix: the affordance rendered through the collection's own
/// `item_template`, so it carried an `editable_text` exactly like a real row.
#[test]
fn the_creation_affordance_mounts_no_editor() {
    let vm = render(
        EDITABLE_MAIN_PANEL_TREE,
        vec![
            page("block:page", "block:root-layout", "Page"),
            page("block:child", "block:page", "Child"),
        ],
    );
    let editable_on_the_affordance: Vec<(String, String)> = widgets_by_row(&vm)
        .into_iter()
        .filter(|(row, widget)| {
            widget == "editable_text"
                && holon_frontend::RowOrigin::from_id(row).is_creation_placeholder()
        })
        .collect();
    assert!(
        editable_on_the_affordance.is_empty(),
        "the creation affordance must mount NO editor — a caret must never be able to sit in a \
         row that is not a block; got {editable_on_the_affordance:?}"
    );

    // ... and the real row beside it still does, so the assertion above is not
    // vacuously true of a tree that renders no editors at all.
    let editable_rows: Vec<String> = widgets_by_row(&vm)
        .into_iter()
        .filter(|(_, widget)| widget == "editable_text")
        .map(|(row, _)| row)
        .collect();
    assert!(
        !editable_rows.is_empty(),
        "the real rows must still be editable; the affordance assertion would otherwise be vacuous"
    );
}

/// The affordance's one gesture: `navigation.focus` on its own id, which the
/// engine intercepts and turns into a birth. Without an action it would be
/// inert and there would be no way to create the first block on an empty page.
#[test]
fn the_creation_affordance_is_selectable_so_focus_can_reach_it() {
    let vm = render(
        EDITABLE_MAIN_PANEL_TREE,
        vec![page("block:page", "block:root-layout", "Page")],
    );
    let affordance_widgets: Vec<String> = widgets_by_row(&vm)
        .into_iter()
        .filter(|(row, _)| holon_frontend::RowOrigin::from_id(row).is_creation_placeholder())
        .map(|(_, widget)| widget)
        .collect();
    assert!(
        affordance_widgets.iter().any(|w| w == "selectable"),
        "the affordance must be selectable so a click can reach it; got {affordance_widgets:?}"
    );
}

/// Every text a row paints, paired with the row id in force at that node.
fn texts_by_row(vm: &Arc<ReactiveViewModel>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut stack: Vec<(Arc<ReactiveViewModel>, String)> = vec![(vm.clone(), String::new())];
    while let Some((node, inherited)) = stack.pop() {
        let row = node.row_id().unwrap_or(inherited);
        if let Some(text) = node.prop_str("content") {
            out.push((row.clone(), text));
        }
        for child in node.children.iter() {
            stack.push((child.clone(), row.clone()));
        }
        if let Some(view) = node.collection.as_ref() {
            for item in view.children_snapshot() {
                stack.push((item, row.clone()));
            }
        }
        if let Some(slot) = node.slot.as_ref() {
            stack.push((slot.content.get_cloned(), row.clone()));
        }
    }
    out
}

/// Ruling D5B-8.a (Martin, 2026-08-18): the creation affordance paints NO text.
/// The row itself — a faint trailing bullet the caret can reach — is the whole
/// affordance; the sentence it used to carry only narrated the gesture the row
/// already invites.
///
/// The affordance must still EXIST and stay selectable (the two tests above),
/// so this judges the painted string alone, not birth-on-focus.
#[test]
fn the_creation_affordance_paints_no_text() {
    let vm = render(
        EDITABLE_MAIN_PANEL_TREE,
        vec![
            page("block:page", "block:root-layout", "Page"),
            page("block:child", "block:page", "Child"),
        ],
    );
    let painted_on_the_affordance: Vec<(String, String)> = texts_by_row(&vm)
        .into_iter()
        .filter(|(row, text)| {
            !text.is_empty() && holon_frontend::RowOrigin::from_id(row).is_creation_placeholder()
        })
        .collect();
    assert!(
        painted_on_the_affordance.is_empty(),
        "the creation affordance must paint no text — the row is the affordance; got \
         {painted_on_the_affordance:?}"
    );

    // ... while the real rows still paint their content, so the assertion above
    // is not vacuously true of a tree that renders no text at all.
    let painted_on_real_rows: Vec<String> = texts_by_row(&vm)
        .into_iter()
        .filter(|(row, _)| !holon_frontend::RowOrigin::from_id(row).is_creation_placeholder())
        .map(|(_, text)| text)
        .collect();
    assert!(
        painted_on_real_rows.iter().any(|t| t == "Child"),
        "the real rows must still paint their content; got {painted_on_real_rows:?}"
    );
}
