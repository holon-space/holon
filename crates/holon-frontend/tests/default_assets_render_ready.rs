//! The bundled `assets/default` org files are the first value of every layout
//! stream, and the recovery screen is made of them. They must render from the
//! parsed text alone, so this test parses each one, builds the root-layout
//! perspective, parses every render source, and interprets each through the
//! shadow builders. No asset may draw an `error` node or an unknown widget,
//! and the root layout draws exactly its perspective's panels.

use std::path::Path;
use std::sync::Arc;

use holon_api::EntityUri;
use holon_api::QueryLanguage;
use holon_api::RenderExpr;
use holon_api::SourceLanguage;
use holon_api::widget_spec::DataRow;
use holon_frontend::ReactiveViewModel;
use holon_frontend::RenderContext;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::render_interpreter::RenderInterpreter;

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

fn render_json(expr: &RenderExpr) -> serde_json::Value {
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
    let ctx = RenderContext::default().with_row(Arc::new(DataRow::new()));
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

#[test]
fn every_default_asset_org_file_parses_and_renders() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/default");
    let mut orgs: Vec<_> = std::fs::read_dir(&dir)
        .expect("assets/default exists")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "org"))
        .collect();
    orgs.sort();
    assert!(
        orgs.iter().any(|p| p.ends_with("index.org")),
        "assets/default lost index.org: {orgs:?}"
    );

    let mut render_sources = 0;
    for path in &orgs {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(path).expect("asset readable");
        let parsed = holon_org_format::parse_org_file(
            Path::new(&name),
            &text,
            &EntityUri::no_parent(),
            Path::new(""),
        )
        .unwrap_or_else(|e| panic!("{name} does not parse: {e:#}"));

        for block in &parsed.blocks {
            if !matches!(block.source_language, Some(SourceLanguage::Render)) {
                continue;
            }
            let expr =
                holon_api::render_dsl::parse_render_dsl(&block.content).unwrap_or_else(|e| {
                    panic!("{name}: render source {} does not parse: {e:#}", block.id)
                });
            let json = render_json(&expr);
            assert!(
                !contains_text(&json, "[unknown: "),
                "{name}: render source {} names a widget the interpreter lacks: {json}",
                block.id
            );
            let errors = error_messages(&json);
            assert!(
                errors.is_empty(),
                "{name}: render source {} draws error nodes: {errors:?}",
                block.id
            );
            render_sources += 1;
        }

        if name == "index.org" {
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
            let json = render_json(&layout);
            let errors = error_messages(&json);
            assert!(
                errors.is_empty(),
                "index.org: the root layout draws error nodes: {errors:?}"
            );
        }
    }
    assert!(
        render_sources > 0,
        "no render source found in assets/default — the check would be vacuous"
    );
}
