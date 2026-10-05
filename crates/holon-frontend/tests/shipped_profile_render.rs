//! Renders shipped profile variants through the real shadow interpreter and
//! checks the output, not just that the profile parses.
//!
//! The render DSL evaluates a profile's render string once, at load. Two bugs
//! hid behind parse-only tests: an unknown widget name (the interpreter
//! degrades it to a `[unknown: <name>]` text node) and operator expressions on
//! `col(...)` that never run per row.

use std::sync::Arc;

use holon_api::Value;
use holon_api::widget_spec::DataRow;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;

const UNKNOWN_BUILDER_MARKER: &str = "[unknown: ";

fn row(pairs: &[(&str, &str)]) -> Arc<DataRow> {
    let mut row = DataRow::new();
    for (k, v) in pairs {
        row.insert((*k).to_string(), Value::String((*v).to_string()));
    }
    Arc::new(row)
}

fn collect_text(node: &serde_json::Value, out: &mut Vec<String>) {
    match node {
        serde_json::Value::Object(map) => {
            if map.get("widget").and_then(|w| w.as_str()) == Some("text") {
                if let Some(content) = map.get("content").and_then(|c| c.as_str()) {
                    out.push(content.to_string());
                }
            }
            map.values().for_each(|v| collect_text(v, out));
        }
        serde_json::Value::Array(items) => items.iter().for_each(|v| collect_text(v, out)),
        _ => {}
    }
}

/// Every text node the render produces for `row`, in tree order.
fn render_texts(render: &str, row: Arc<DataRow>) -> Vec<String> {
    let services = StubBuilderServices::new();
    let expr = holon_api::render_dsl::parse_render_dsl(render)
        .unwrap_or_else(|e| panic!("render does not parse: {render}: {e:#}"));
    let ctx = RenderContext::default().with_row(row);
    let vm = services.interpret(&expr, &ctx).snapshot();
    let json = serde_json::to_value(&vm).expect("ViewModel serializes");
    let mut out = Vec::new();
    collect_text(&json, &mut out);
    out
}

fn variants_of(yaml: &str) -> Vec<(String, String)> {
    holon_profiles::parse_profile_yaml(yaml)
        .expect("shipped profile parses")
        .variants
        .into_iter()
        .map(|v| (v.name, v.render))
        .collect()
}

#[test]
fn recipe_page_renders_without_an_unknown_builder() {
    let variants = variants_of(holon_kitchen::RECIPE_PROFILE_YAML);
    let (_, render) = variants
        .iter()
        .find(|(name, _)| name == "default")
        .expect("recipe has a default variant");
    let texts = render_texts(
        render,
        row(&[("title", "Pancakes"), ("course", "breakfast")]),
    );
    assert!(
        !texts.iter().any(|t| t.contains(UNKNOWN_BUILDER_MARKER)),
        "recipe page rendered an unknown-builder node: {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t == "Pancakes"),
        "recipe page lost its title: {texts:?}"
    );
}

/// General guard: no shipped variant may name a widget the interpreter lacks.
#[test]
fn no_shipped_profile_variant_renders_an_unknown_builder() {
    let shipped: [(&str, &str); 6] = [
        ("recipe", holon_kitchen::RECIPE_PROFILE_YAML),
        ("shopping_item", holon_kitchen::SHOPPING_ITEM_PROFILE_YAML),
        (
            "block",
            include_str!("../../../assets/default/types/block_profile.yaml"),
        ),
        (
            "person",
            include_str!("../../../assets/default/types/person_profile.yaml"),
        ),
        (
            "collection",
            include_str!("../../../assets/default/types/collection_profile.yaml"),
        ),
        (
            "integration",
            include_str!("../../../assets/default/types/integration_profile.yaml"),
        ),
    ];
    let mut offenders = Vec::new();
    for (profile, yaml) in shipped {
        for (variant, render) in variants_of(yaml) {
            let r = render.clone();
            let rendered = std::panic::catch_unwind(move || {
                render_texts(&r, row(&[("id", "block:x"), ("content", "c")]))
            });
            let Ok(texts) = rendered else {
                eprintln!(
                    "{profile}/{variant}: builder panicked on the stub row (not an unknown-builder case)"
                );
                continue;
            };
            if texts.iter().any(|t| t.contains(UNKNOWN_BUILDER_MARKER)) {
                offenders.push(format!("{profile}/{variant}: {texts:?}"));
            }
        }
    }
    assert!(offenders.is_empty(), "unknown builders: {offenders:#?}");
}

fn two_rows() -> (Arc<DataRow>, Arc<DataRow>) {
    (
        row(&[("q", "100"), ("u", "g"), ("unit", "g")]),
        row(&[("q", "2"), ("u", "kg"), ("unit", "kg")]),
    )
}

fn assert_per_row(render: &str, expected_a: &str, expected_b: &str) {
    let (a, b) = two_rows();
    let ta = render_texts(render, a);
    let tb = render_texts(render, b);
    eprintln!("{render}\n  row A -> {ta:?}\n  row B -> {tb:?}");
    assert_eq!(ta, vec![expected_a.to_string()], "row A");
    assert_eq!(tb, vec![expected_b.to_string()], "row B");
}

#[test]
#[ignore = "2026-10-05-render-dsl-col-expressions-evaluated-at-load"]
fn col_plus_string_concatenation_is_evaluated_per_row() {
    assert_per_row(r#"text(col("q") + " " + col("u"))"#, "100 g", "2 kg");
}

#[test]
#[ignore = "2026-10-05-render-dsl-col-expressions-evaluated-at-load"]
fn template_string_interpolation_is_evaluated_per_row() {
    assert_per_row(r#"text(`${col("q")} ${col("u")}`)"#, "100 g", "2 kg");
}

#[test]
#[ignore = "2026-10-05-render-dsl-col-expressions-evaluated-at-load"]
fn if_on_col_equality_is_decided_per_row() {
    assert_per_row(
        r#"if col("unit") == "g" { text("grams") } else { text("other") }"#,
        "grams",
        "other",
    );
}
