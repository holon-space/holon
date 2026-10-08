//! The bundled `assets/default` org files are the first value of every layout
//! stream, and the recovery screen is made of them. They must render from the
//! parsed text alone, so this test parses each one, builds the root-layout
//! perspective, parses every render source, and interprets each through the
//! shadow builders. Authored input that a builder rejects must draw an error
//! node: a panic there aborts a release build before any pixel.

use std::path::Path;
use std::sync::Arc;

use holon_api::EntityUri;
use holon_api::SourceLanguage;
use holon_api::widget_spec::DataRow;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;

fn render_json(expr: &holon_api::RenderExpr) -> serde_json::Value {
    let services = StubBuilderServices::new();
    let ctx = RenderContext::default().with_row(Arc::new(DataRow::new()));
    let vm = services.interpret(expr, &ctx).snapshot();
    serde_json::to_value(&vm).expect("ViewModel serializes")
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
fn op_button_without_a_name_or_target_draws_an_error() {
    let expr = holon_api::render_dsl::parse_render_dsl("op_button()").expect("parses");
    let json = render_json(&expr);
    assert_eq!(
        json.get("widget").and_then(|w| w.as_str()),
        Some("error"),
        "op_button over a row with no `name`/`target_id` must draw an error node: {json}"
    );
    assert!(
        contains_text(&json, "op_button"),
        "the error must name the builder: {json}"
    );
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
            render_sources += 1;
        }

        if name == "index.org" {
            let root = holon_api::root_layout_block_uri();
            let spec = holon_api::perspective::resolve_active_perspective(&root, &parsed.blocks)
                .unwrap_or_else(|e| panic!("index.org: root layout does not build: {e:#}"));
            let layout = spec
                .layout_expr()
                .unwrap_or_else(|e| panic!("index.org: layout does not synthesize: {e:#}"));
            render_json(&layout);
        }
    }
    assert!(
        render_sources > 0,
        "no render source found in assets/default — the check would be vacuous"
    );
}
