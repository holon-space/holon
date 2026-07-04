//! The `claude-history` chat view must actually render.
//!
//! The shipped `cc_session` / `cc_agent` profiles nest a `live_query` inside
//! `expand_toggle`'s `content:`, which only reaches the screen if `content` is
//! deferred as a template and materialised through the lazy slot. This test
//! reads the profile out of `docs/integrations/claude-history.yaml` rather than
//! restating it, so a drifting sidecar cannot pass a stale hand-copy.

use std::sync::Arc;

use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::Value;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::ReactiveViewModel;
use holon_frontend::RenderContext;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::render_interpreter::RenderInterpreter;

/// Services that can start a live query. `StubBuilderServices` deliberately
/// refuses one, which would collapse every `live_query` in the profile into an
/// error node and hide the very nesting under test. The interpreter is owned
/// here (not delegated) so nested interpretation re-enters THESE services.
struct ChatViewServices {
    interpreter: Arc<RenderInterpreter<ReactiveViewModel>>,
    link_classifier: holon_api::link_parser::LinkTargetClassifier,
    rt_handle: tokio::runtime::Handle,
}

impl ChatViewServices {
    fn new(rt_handle: tokio::runtime::Handle) -> Self {
        Self {
            interpreter: Arc::new(holon_frontend::shadow_builders::build_shadow_interpreter()),
            link_classifier: holon_api::link_parser::LinkTargetClassifier::default(),
            rt_handle,
        }
    }
}

impl BuilderServices for ChatViewServices {
    fn interpret(&self, expr: &RenderExpr, ctx: &RenderContext) -> ReactiveViewModel {
        self.interpreter.interpret(expr, ctx, self)
    }

    fn clone_arc(&self) -> Arc<dyn BuilderServices> {
        Arc::new(Self {
            interpreter: self.interpreter.clone(),
            link_classifier: holon_api::link_parser::LinkTargetClassifier::default(),
            rt_handle: self.rt_handle.clone(),
        })
    }

    fn get_block_data(&self, _: &EntityUri) -> (RenderExpr, Vec<Arc<DataRow>>) {
        (
            RenderExpr::FunctionCall {
                name: "table".to_string(),
                args: vec![],
            },
            vec![],
        )
    }

    fn link_classifier(&self) -> &holon_api::link_parser::LinkTargetClassifier {
        &self.link_classifier
    }

    fn resolve_profile(&self, _: &DataRow) -> Option<holon_api::RenderProfile> {
        None
    }

    /// An empty-but-live stream: `live_query` validates by starting a watch and
    /// dropping it, then publishes the query on the node for the platform layer
    /// to subscribe to. Rows never flow through this call.
    fn watch_query(
        &self,
        _: &str,
        _: QueryLanguage,
        _: Option<holon_frontend::QueryContext>,
    ) -> anyhow::Result<holon_api::EnrichedChangeStream> {
        let (_tx, rx) = tokio::sync::mpsc::channel(1);
        Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
    }

    fn widget_state(&self, _: &str) -> holon_frontend::WidgetState {
        holon_frontend::WidgetState::default()
    }

    fn dispatch_intent(&self, _: holon_frontend::operations::OperationIntent) {}

    fn present_op(
        &self,
        op: holon_api::render_types::OperationDescriptor,
        _: std::collections::HashMap<String, Value>,
    ) {
        panic!(
            "present_op({}.{}) is not part of the chat view",
            op.entity_name, op.name
        )
    }

    fn search_link_candidates(
        &self,
        _: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = anyhow::Result<Vec<holon_api::link_candidate::LinkCandidate>>,
                > + Send,
        >,
    > {
        Box::pin(async { Ok(vec![]) })
    }

    fn runtime_handle(&self) -> tokio::runtime::Handle {
        self.rt_handle.clone()
    }
}

/// The `render` string of an entity's first profile variant, straight from the
/// shipped sidecar.
fn shipped_profile(entity: &str) -> String {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/integrations/claude-history.yaml"
    );
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read claude-history.yaml at {path}: {e}"));
    let doc: serde_yaml::Value =
        serde_yaml::from_str(&raw).unwrap_or_else(|e| panic!("parse claude-history.yaml: {e}"));
    doc["entities"][entity]["profile_variants"][0]["render"]
        .as_str()
        .unwrap_or_else(|| panic!("entity `{entity}` has no profile_variants[0].render"))
        .to_string()
}

fn interpret_with(
    services: &ChatViewServices,
    dsl: &str,
    ctx: &RenderContext,
) -> ReactiveViewModel {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let parsed: RenderExpr = holon_api::render_dsl::parse_render_dsl(dsl)
        .unwrap_or_else(|e| panic!("chat-view profile must parse: {e}"));
    services.interpret(&parsed, ctx)
}

/// The mirror stores the primary key scheme-qualified, exactly as the chat
/// view reads it.
const ROW_ID: &str = "cc-session:sess-1";
/// A live row's own key: the provider keys a backgrounded session by its job
/// id.
const LIVE_ROW_ID: &str = "cc-live-session:job-77";
/// The ONLY value the provider accepts as a `send_message` target. It is
/// deliberately unlike the transcript uuid on the same row — an id derived from
/// anything but `job_id` would address nothing.
const JOB_ID: &str = "job-77";

fn session_row() -> Arc<DataRow> {
    let mut row = DataRow::new();
    row.insert("id".to_string(), Value::String(ROW_ID.to_string()));
    row.insert(
        "first_prompt".to_string(),
        Value::String("Fix the parser".to_string()),
    );
    row.insert("message_count".to_string(), Value::Integer(2));
    Arc::new(row)
}

fn live_session_row() -> Arc<DataRow> {
    let mut row = DataRow::new();
    row.insert("id".to_string(), Value::String(LIVE_ROW_ID.to_string()));
    row.insert("job_id".to_string(), Value::String(JOB_ID.to_string()));
    row.insert(
        "session_id".to_string(),
        Value::String("sess-1".to_string()),
    );
    row.insert("name".to_string(), Value::String("lane-1".to_string()));
    row.insert("state".to_string(), Value::String("running".to_string()));
    Arc::new(row)
}

/// The profile is rendered the way the sidebar renders it, with the gate seeded
/// open so the lazy content materialises in-process.
fn expanded_chat_view(services: &ChatViewServices, entity: &str) -> Arc<ReactiveViewModel> {
    let profile = shipped_profile(entity);
    let dsl = profile.replacen(
        "expand_toggle(#{",
        "expand_toggle(#{default_expanded: true, ",
        1,
    );
    assert_ne!(
        dsl, profile,
        "the `{entity}` profile is expected to be an expand_toggle; got:\n{profile}"
    );
    let row = if entity == "live_session" {
        live_session_row()
    } else {
        session_row()
    };
    let ctx = RenderContext::default().with_row(row);
    let vm = interpret_with(services, &dsl, &ctx);
    assert_eq!(vm.widget_name().as_deref(), Some("expand_toggle"));
    vm.lazy_slot
        .as_ref()
        .expect(
            "`content:` must build a lazy slot — without it the chat view is a header with \
             nothing under it",
        )
        .materialize_if_gated()
        .expect("an expanded chat view must materialise its content")
}

/// The first node in the tree rendering `widget`. The chat view nests the
/// message stream and the compose box under one container, so a test about the
/// stream must address it rather than the container.
fn find(vm: &Arc<ReactiveViewModel>, widget: &str) -> Arc<ReactiveViewModel> {
    if vm.widget_name().as_deref() == Some(widget) {
        return vm.clone();
    }
    for child in &vm.children {
        if let Some(hit) = find_opt(child, widget) {
            return hit;
        }
    }
    panic!(
        "no `{widget}` node in the rendered chat view:\n{}",
        vm.snapshot().pretty_print(0)
    )
}

fn find_opt(vm: &Arc<ReactiveViewModel>, widget: &str) -> Option<Arc<ReactiveViewModel>> {
    if vm.widget_name().as_deref() == Some(widget) {
        return Some(vm.clone());
    }
    vm.children.iter().find_map(|c| find_opt(c, widget))
}

fn descendants(vm: &ReactiveViewModel, out: &mut Vec<String>) {
    if let Some(name) = vm.widget_name() {
        out.push(name);
    }
    for child in &vm.children {
        descendants(child, out);
    }
    if let Some(slot) = vm.slot.as_ref() {
        descendants(&slot.content.get_cloned(), out);
    }
}

/// The load-bearing case: the session chat view materialises a `live_query`
/// under the toggle, carrying the message query. A regression in per-widget
/// template classification drops the slot entirely and this fails at
/// `materialize_if_gated`.
#[tokio::test]
async fn session_chat_view_materialises_its_message_query() {
    let services = ChatViewServices::new(tokio::runtime::Handle::current());
    let content = expanded_chat_view(&services, "session");

    let mut names = Vec::new();
    descendants(&content, &mut names);
    assert!(
        names.iter().any(|n| n == "live_query"),
        "the expanded session view must contain a live_query; got {names:?}"
    );

    let query = find(&content, "live_query")
        .prop_str("query")
        .expect("the live_query node must publish its query for the platform layer to subscribe");
    // The 2026-08-04 turso-fdw-ivm-handoff ruling: read the CACHE table
    // (`cc_message`), never the `_fdw` foreign table (snapshot, not stream).
    assert!(
        query.contains("FROM cc_message ") || query.ends_with("FROM cc_message"),
        "the chat view must query the cc_message cache table (never cc_message_fdw); got `{query}`"
    );
    assert!(
        !query.contains("cc_message_fdw"),
        "the chat view must not read the _fdw snapshot table; got `{query}`"
    );
}

/// The agent chat view is the same shape and must not rot independently.
#[tokio::test]
async fn agent_chat_view_materialises_its_message_query() {
    let services = ChatViewServices::new(tokio::runtime::Handle::current());
    let content = expanded_chat_view(&services, "agent");

    let mut names = Vec::new();
    descendants(&content, &mut names);
    assert!(
        names.iter().any(|n| n == "live_query"),
        "the expanded agent view must contain a live_query; got {names:?}"
    );
}

/// A message row must render as a `chat_bubble` carrying the message text.
/// `live_query` interprets its `item_template` against rows supplied by the
/// platform layer, so the template is exercised here against a fixture row.
#[tokio::test]
async fn message_row_renders_as_a_chat_bubble_with_its_text() {
    let services = ChatViewServices::new(tokio::runtime::Handle::current());
    let content = expanded_chat_view(&services, "session");

    let item_template = find(&content, "live_query")
        .prop_str("render_expr")
        .expect("live_query must publish the item_template it renders rows with");
    let template: RenderExpr = serde_json::from_str(&item_template)
        .unwrap_or_else(|e| panic!("item_template must deserialise: {e}\n{item_template}"));

    let mut row = DataRow::new();
    row.insert("uuid".to_string(), Value::String("m-1".to_string()));
    row.insert("role".to_string(), Value::String("user".to_string()));
    row.insert(
        "content".to_string(),
        Value::String("HELLO-FROM-THE-TRANSCRIPT".to_string()),
    );
    row.insert("ts".to_string(), Value::String("2026-08-03".to_string()));
    let ctx = RenderContext::default().with_row(Arc::new(row));

    let bubble = services.interpret(&template, &ctx);
    assert_eq!(
        bubble.widget_name().as_deref(),
        Some("chat_bubble"),
        "a message row must render as a chat_bubble"
    );
    assert_eq!(bubble.prop_str("sender").as_deref(), Some("user"));

    let rendered = bubble.snapshot().pretty_print(0);
    assert!(
        rendered.contains("HELLO-FROM-THE-TRANSCRIPT"),
        "the bubble must show the message text; got:\n{rendered}"
    );
}

/// Every operation wiring in the tree, including collection items.
fn wirings(vm: &ReactiveViewModel, out: &mut Vec<holon_api::render_types::OperationWiring>) {
    out.extend(vm.operations.iter().cloned());
    for child in &vm.children {
        wirings(child, out);
    }
    if let Some(slot) = vm.slot.as_ref() {
        wirings(&slot.content.get_cloned(), out);
    }
    if let Some(collection) = vm.collection.as_ref() {
        for item in collection.children_snapshot() {
            wirings(&item, out);
        }
    }
}

/// The chat view must offer a way to REPLY, and that compose box must dispatch
/// arguments the provider actually declares.
///
/// Both halves were wrong at once and each was fatal on its own: the text rode
/// under `message` where the tool declares `text`, and the target was the
/// transcript uuid where the tool resolves only a live `id` / `job_id`. This
/// asserts the wire names, not a Holon-side convention.
#[tokio::test]
async fn live_session_chat_view_composes_with_the_arguments_the_provider_declares() {
    let services = ChatViewServices::new(tokio::runtime::Handle::current());
    let content = expanded_chat_view(&services, "live_session");

    let mut names = Vec::new();
    descendants(&content, &mut names);
    assert!(
        names.iter().any(|n| n == "input_box"),
        "the expanded live-session view must carry a compose box; got {names:?}"
    );

    let mut found = Vec::new();
    wirings(&content, &mut found);
    let send: Vec<_> = found
        .iter()
        .filter(|w| w.descriptor.name == "send_message")
        .collect();
    assert_eq!(
        send.len(),
        1,
        "exactly one send wiring under the chat view; got {:?}",
        found
            .iter()
            .map(|w| format!("{}.{}", w.descriptor.entity_name, w.descriptor.name))
            .collect::<Vec<_>>()
    );
    let wiring = send[0];
    assert_eq!(
        wiring.descriptor.entity_name,
        holon_api::EntityName::from("live_session"),
    );
    assert_eq!(
        wiring.descriptor.bound_params.get("id"),
        Some(&Value::String(JOB_ID.to_string())),
        "the target must be the background job id, not the row key or the transcript uuid; \
         bound: {:?}",
        wiring.descriptor.bound_params
    );
    assert_eq!(
        wiring.modified_param, "text",
        "the typed message must ride under the argument the tool declares"
    );
}

/// A transcript on disk is READ-ONLY: nothing on a `session` row can address a
/// running process, so the profile must not offer a compose box that could only
/// dispatch a send the provider refuses.
#[tokio::test]
async fn session_chat_view_offers_no_compose_box() {
    let services = ChatViewServices::new(tokio::runtime::Handle::current());
    let content = expanded_chat_view(&services, "session");

    let mut names = Vec::new();
    descendants(&content, &mut names);
    assert!(
        !names.iter().any(|n| n == "input_box"),
        "a transcript view must not carry an unaddressable compose box; got {names:?}"
    );
}
