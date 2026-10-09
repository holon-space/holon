//! Every shipped render source (see `shipped_sources`) renders through the
//! shadow builders without an `error` node or an unknown widget, and the
//! bundled root layout draws exactly its perspective's panels. The bundled
//! org files are the first value of every layout stream and the recovery
//! screen is made of them, so they must render from the parsed text alone.

use std::path::Path;
use std::sync::Arc;

use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::RenderExpr;
use holon_api::Value;
use holon_api::widget_spec::DataRow;
use holon_frontend::ReactiveViewModel;
use holon_frontend::RenderContext;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::render_interpreter::RenderInterpreter;

mod shipped_sources;

/// Services whose live queries start and stay empty, so a `live_query` in an
/// asset renders as itself and any `error` node comes from the asset.
struct AssetServices {
    interpreter: Arc<RenderInterpreter<ReactiveViewModel>>,
    link_classifier: holon_api::link_parser::LinkTargetClassifier,
    rt_handle: tokio::runtime::Handle,
}

impl BuilderServices for AssetServices {
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
            detail: "AssetServices: test double, no delivery to prove".to_string(),
        })))
    }

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

    fn dispatch_intent(&self, _: holon_frontend::operations::OperationIntent) {}

    fn present_op(
        &self,
        op: holon_api::render_types::OperationDescriptor,
        _: std::collections::HashMap<String, holon_api::Value>,
    ) {
        panic!(
            "present_op({}.{}) is not reached by interpreting an asset",
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

fn render_json(expr: &RenderExpr, row: DataRow) -> serde_json::Value {
    static RT: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    let rt = RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime")
    });
    let services = AssetServices {
        interpreter: Arc::new(holon_frontend::shadow_builders::build_shadow_interpreter()),
        link_classifier: holon_api::link_parser::LinkTargetClassifier::default(),
        rt_handle: rt.handle().clone(),
    };
    let ctx = RenderContext::default().with_row(Arc::new(row));
    let vm = services.interpret(expr, &ctx).snapshot();
    serde_json::to_value(&vm).expect("ViewModel serializes")
}

/// The message of every `error` node in `node`'s tree.
fn error_messages(node: &serde_json::Value) -> Vec<String> {
    match node {
        serde_json::Value::Object(map) => {
            let own = (map.get("widget").and_then(|w| w.as_str()) == Some("error")).then(|| {
                map.get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("<no message>")
                    .to_string()
            });
            own.into_iter()
                .chain(map.values().flat_map(error_messages))
                .collect()
        }
        serde_json::Value::Array(items) => items.iter().flat_map(error_messages).collect(),
        _ => Vec::new(),
    }
}

/// The block ids of every `live_block("…")` in `expr`.
fn live_block_ids(expr: &RenderExpr) -> Vec<String> {
    let own = match expr {
        RenderExpr::FunctionCall { name, args } if name == "live_block" => {
            args.first().and_then(|a| match &a.value {
                RenderExpr::Literal {
                    value: holon_api::Value::String(id),
                } => Some(id.clone()),
                _ => None,
            })
        }
        _ => None,
    };
    own.into_iter()
        .chain(expr.children().into_iter().flat_map(live_block_ids))
        .collect()
}

fn contains_text(node: &serde_json::Value, needle: &str) -> bool {
    match node {
        serde_json::Value::String(s) => s.contains(needle),
        serde_json::Value::Object(map) => map.values().any(|v| contains_text(v, needle)),
        serde_json::Value::Array(items) => items.iter().any(|v| contains_text(v, needle)),
        _ => false,
    }
}

/// Every column `col("…")` reads in `expr`.
fn column_names(expr: &RenderExpr) -> Vec<String> {
    let own = match expr {
        RenderExpr::ColumnRef { name } => Some(name.clone()),
        _ => None,
    };
    own.into_iter()
        .chain(expr.children().into_iter().flat_map(column_names))
        .collect()
}

/// The row a shipped source is rendered over, shaped like the rows prod
/// delivers to it: an entity id, and a text value in every column the source
/// reads, except the columns whose prod type is not text.
fn source_row(label: &str, expr: &RenderExpr) -> DataRow {
    let mut row = DataRow::new();
    for column in column_names(expr) {
        row.insert(column.clone(), Value::String(format!("{column} value")));
    }
    row.insert(
        "id".to_string(),
        Value::String("block:shipped-source-row".into()),
    );
    match label {
        // The block profile computes `todo_states: '()'`.
        l if l.starts_with("default/types/block_profile.yaml#") => {
            row.insert("todo_states".to_string(), Value::Null);
        }
        // The integration mirror's `enabled` is an INTEGER column.
        "default/types/integration_profile.yaml#0" => {
            row.insert("enabled".to_string(), Value::Integer(1));
        }
        // `pending_question.options` holds the offered answers as a JSON array.
        "integrations/claude-history.yaml#2" => {
            row.insert(
                "options".to_string(),
                Value::String(r#"[{"label":"yes"},{"label":"no"}]"#.into()),
            );
        }
        _ => {}
    }
    row
}

#[test]
fn every_shipped_render_source_draws_no_error_node() {
    let shipped = shipped_sources::shipped_render_strings();
    assert!(
        shipped.iter().any(|(l, _)| l == "default/index.org#0")
            && shipped
                .iter()
                .any(|(l, _)| l.starts_with("default/types/person_profile.yaml"))
            && shipped.iter().any(|(l, _)| l.starts_with("integrations/"))
            && shipped.iter().any(|(l, _)| l.starts_with("kitchen/")),
        "the shipped corpus lost a source family: {:?}",
        shipped.iter().map(|(l, _)| l).collect::<Vec<_>>()
    );
    let mut offenders = Vec::new();
    for (label, source) in &shipped {
        let expr = holon_api::render_dsl::parse_render_dsl(source)
            .unwrap_or_else(|e| panic!("{label} does not parse: {e:#}"));
        let json = render_json(&expr, source_row(label, &expr));
        if contains_text(&json, "[unknown: ") {
            offenders.push(format!("{label}: names a widget the interpreter lacks"));
        }
        let errors = error_messages(&json);
        if !errors.is_empty() {
            offenders.push(format!("{label}: {errors:?}\n    SRC: {source}"));
        }
    }
    assert!(
        offenders.is_empty(),
        "shipped render sources drawing error nodes ({}):\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}

#[test]
fn the_root_layout_draws_exactly_its_perspective_panels() {
    let name = "index.org";
    let text = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/default/index.org"),
    )
    .expect("assets/default/index.org readable");
    let parsed = holon_org_format::parse_org_file(
        Path::new(name),
        &text,
        &EntityUri::no_parent(),
        Path::new(""),
    )
    .unwrap_or_else(|e| panic!("{name} does not parse: {e:#}"));
    let root = holon_api::root_layout_block_uri();
    let spec = holon_api::perspective::resolve_active_perspective(&root, &parsed.blocks)
        .unwrap_or_else(|e| panic!("index.org: root layout does not build: {e:#}"));
    let layout = spec
        .layout_expr()
        .unwrap_or_else(|e| panic!("index.org: layout does not synthesize: {e:#}"));
    let mut panels: Vec<String> = spec
        .panels
        .iter()
        .filter(|p| p.is_displayable())
        .map(|p| p.id.to_string())
        .collect();
    panels.sort();
    assert!(
        !panels.is_empty(),
        "index.org: the root layout has no panel"
    );
    let mut drawn = live_block_ids(&layout);
    drawn.sort();
    drawn.dedup();
    assert_eq!(
        drawn, panels,
        "index.org: the layout must draw exactly the perspective's displayable panels"
    );
    let json = render_json(&layout, DataRow::new());
    let errors = error_messages(&json);
    assert!(
        errors.is_empty(),
        "index.org: the root layout draws error nodes: {errors:?}"
    );
}
