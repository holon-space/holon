//! The pending-question view must turn what a live agent session is blocked on
//! into something clickable — one button per offered answer, each wired to the
//! `answer_question` tool with THAT option's label.
//!
//! The profile is read out of `docs/integrations/claude-history.yaml` rather
//! than restated, so a drifting sidecar cannot pass a stale hand-copy.
//!
//! The second property is the safety one: an answer whose label the question
//! does not offer must be refused HERE, before any operation wiring exists to
//! dispatch — the provider refusing it too is a second line, not the first.

use std::sync::Arc;

use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::Value;
use holon_api::render_types::OperationWiring;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::ReactiveViewModel;
use holon_frontend::RenderContext;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::render_interpreter::RenderInterpreter;

struct QuestionViewServices {
    interpreter: Arc<RenderInterpreter<ReactiveViewModel>>,
    link_classifier: holon_api::link_parser::LinkTargetClassifier,
    rt_handle: tokio::runtime::Handle,
}

impl QuestionViewServices {
    fn new(rt_handle: tokio::runtime::Handle) -> Self {
        Self {
            interpreter: Arc::new(holon_frontend::shadow_builders::build_shadow_interpreter()),
            link_classifier: holon_api::link_parser::LinkTargetClassifier::default(),
            rt_handle,
        }
    }
}

impl BuilderServices for QuestionViewServices {
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

    /// Nothing in this view may dispatch at BUILD time — a question is answered
    /// by a click, never by rendering.
    fn dispatch_intent(&self, intent: holon_frontend::operations::OperationIntent) {
        panic!(
            "rendering must dispatch nothing, got {}.{}",
            intent.entity_name, intent.op_name
        )
    }

    fn present_op(
        &self,
        op: holon_api::render_types::OperationDescriptor,
        _: std::collections::HashMap<String, Value>,
    ) {
        panic!("present_op({}.{}) must not fire", op.entity_name, op.name)
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

/// The mirror stores the primary key scheme-qualified, exactly as the UI reads
/// it.
const ROW_ID: &str = "cc-pending-question:job-77:0:1a2b3c4d";
/// What the provider will accept as `answer_question.question_id`.
const QUESTION_ID: &str = "job-77:0:1a2b3c4d";
const OPTIONS_JSON: &str = r#"[{"label":"Turso","description":"embedded"},{"label":"Plain SQLite","description":"no IVM"}]"#;

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

fn question_row(options: &str) -> Arc<DataRow> {
    let mut row = DataRow::new();
    row.insert("id".to_string(), Value::String(ROW_ID.to_string()));
    row.insert(
        "session_id".to_string(),
        Value::String("s-bg-1".to_string()),
    );
    row.insert(
        "question".to_string(),
        Value::String("Which storage backend should the lane use?".to_string()),
    );
    row.insert("options".to_string(), Value::String(options.to_string()));
    row.insert("answerable".to_string(), Value::Integer(1));
    Arc::new(row)
}

fn render(services: &QuestionViewServices, dsl: &str, options: &str) -> ReactiveViewModel {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let parsed: RenderExpr = holon_api::render_dsl::parse_render_dsl(dsl)
        .unwrap_or_else(|e| panic!("pending-question profile must parse: {e}\n{dsl}"));
    let ctx = RenderContext::default().with_row(question_row(options));
    services.interpret(&parsed, &ctx)
}

/// Every wiring in the tree, paired with the widget node carrying it.
fn wirings(vm: &ReactiveViewModel, out: &mut Vec<OperationWiring>) {
    out.extend(vm.operations.iter().cloned());
    for child in &vm.children {
        wirings(child, out);
    }
    if let Some(collection) = vm.collection.as_ref() {
        for item in collection.children_snapshot() {
            wirings(&item, out);
        }
    }
}

/// Every `text` content in the tree, including collection items (which the
/// `snapshot()` pretty-printer does not descend into).
fn texts(vm: &ReactiveViewModel, out: &mut Vec<String>) {
    if let Some(content) = vm.prop_str("content") {
        out.push(content);
    }
    for child in &vm.children {
        texts(child, out);
    }
    if let Some(collection) = vm.collection.as_ref() {
        for item in collection.children_snapshot() {
            texts(&item, out);
        }
    }
}

fn bound(wiring: &OperationWiring, param: &str) -> Value {
    wiring
        .descriptor
        .bound_params
        .get(param)
        .unwrap_or_else(|| {
            panic!(
                "wiring must bind `{param}`; bound: {:?}",
                wiring.descriptor.bound_params
            )
        })
        .clone()
}

/// The labels a wiring would answer with. The provider takes an ARRAY, so a
/// scalar here is the shipped bug, not a shorthand.
fn answers(wiring: &OperationWiring) -> Vec<String> {
    match bound(wiring, "answers") {
        Value::Array(items) => items.iter().map(Value::to_display_string).collect(),
        other => panic!("`answers` must be an array of labels, got {other:?}"),
    }
}

/// (a) A pending question renders one button per offered option, each carrying
/// the `answer_question` wiring for ITS label.
#[tokio::test]
async fn pending_question_renders_one_button_per_offered_option() {
    let services = QuestionViewServices::new(tokio::runtime::Handle::current());
    let vm = render(
        &services,
        &shipped_profile("pending_question"),
        OPTIONS_JSON,
    );

    let mut found = Vec::new();
    wirings(&vm, &mut found);

    let chosen: Vec<Vec<String>> = found.iter().map(answers).collect();
    assert_eq!(
        chosen,
        vec![vec!["Turso".to_string()], vec!["Plain SQLite".to_string()]],
        "one answer button per offered option, in offer order, each dispatching its \
         own label as a ONE-ELEMENT array — the shape the provider declares; tree:\n{}",
        vm.snapshot().pretty_print(0)
    );

    for wiring in &found {
        assert_eq!(
            wiring.descriptor.entity_name,
            holon_api::EntityName::from("pending_question"),
        );
        assert_eq!(wiring.descriptor.name, "answer_question");
        assert_eq!(
            bound(wiring, "question_id").to_display_string(),
            QUESTION_ID,
            "the scheme-qualified row id must be unwrapped to the provider's opaque \
             question_id, read off the live row at build time"
        );
        assert!(
            wiring.descriptor.click_modifiers().is_some(),
            "an answer button must be click-triggered, got {:?}",
            wiring.descriptor.trigger
        );
    }

    let mut on_screen = Vec::new();
    texts(&vm, &mut on_screen);
    assert!(
        on_screen.iter().any(|t| t == "Turso") && on_screen.iter().any(|t| t == "Plain SQLite"),
        "both offered answers must be on screen; got {on_screen:?}"
    );
}

/// (b) A label the question does not offer is refused BEFORE dispatch: the
/// widget emits a visible refusal and NO wiring at all, so there is nothing a
/// click could fire.
#[tokio::test]
async fn a_label_the_question_does_not_offer_is_refused_before_dispatch() {
    let services = QuestionViewServices::new(tokio::runtime::Handle::current());
    let dsl = r#"question_options(#{
        options: col("options"),
        action: answer_question(#{question_id: col("id"), answers: "Maybe"})
    })"#;
    let vm = render(&services, dsl, OPTIONS_JSON);

    let mut found = Vec::new();
    wirings(&vm, &mut found);
    assert!(
        found.is_empty(),
        "a non-offered label must produce NO dispatchable wiring; got {:?}",
        found
            .iter()
            .map(|w| w.descriptor.bound_params.clone())
            .collect::<Vec<_>>()
    );

    let rendered = vm.snapshot().pretty_print(0);
    assert!(
        rendered.contains("Maybe") && rendered.contains("not"),
        "the refusal must be VISIBLE and name the rejected label; got:\n{rendered}"
    );
}

/// Malformed `options` is a loud, visible failure — never a silently
/// button-less question that looks answered.
#[tokio::test]
async fn malformed_options_fail_visibly_with_no_buttons() {
    let services = QuestionViewServices::new(tokio::runtime::Handle::current());
    let vm = render(
        &services,
        &shipped_profile("pending_question"),
        "not json at all",
    );

    let mut found = Vec::new();
    wirings(&vm, &mut found);
    assert!(found.is_empty(), "malformed options must wire nothing");

    let rendered = vm.snapshot().pretty_print(0);
    assert!(
        rendered.contains("not json at all"),
        "the failure must quote what could not be parsed; got:\n{rendered}"
    );
}
