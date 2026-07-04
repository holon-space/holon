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
