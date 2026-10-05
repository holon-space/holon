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
            if let Err(e) = holon_api::render_dsl::parse_render_dsl(&render) {
                offenders.push(format!("{profile}/{variant}: refused: {e:#}"));
                continue;
            }
            // The stub row has no `enabled` column, which `state_toggle`'s
            // builder rejects; that variant's builders are checked by
            // `state_toggle_bool_binding`.
            if (profile, variant.as_str()) == ("integration", "default") {
                continue;
            }
            let texts = render_texts(&render, row(&[("id", "block:x"), ("content", "c")]));
            if texts.iter().any(|t| t.contains(UNKNOWN_BUILDER_MARKER)) {
                offenders.push(format!("{profile}/{variant}: {texts:?}"));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "unknown or refused builders: {offenders:#?}"
    );
}

fn collect_yaml_renders(node: &serde_yaml::Value, out: &mut Vec<String>) {
    match node {
        serde_yaml::Value::Mapping(map) => {
            for (k, v) in map {
                match (k.as_str(), v.as_str()) {
                    (Some("render"), Some(render)) => out.push(render.to_string()),
                    _ => collect_yaml_renders(v, out),
                }
            }
        }
        serde_yaml::Value::Sequence(items) => {
            items.iter().for_each(|v| collect_yaml_renders(v, out))
        }
        _ => {}
    }
}

fn yaml_renders(label: &str, yaml: &str) -> Vec<(String, String)> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(yaml).unwrap_or_else(|e| panic!("{label} is not yaml: {e}"));
    let mut renders = Vec::new();
    collect_yaml_renders(&doc, &mut renders);
    renders
        .into_iter()
        .enumerate()
        .map(|(i, r)| (format!("{label}#{i}"), r))
        .collect()
}

/// The bodies of the `#+BEGIN_SRC render` blocks of an org file.
fn org_renders(label: &str, org: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    for line in org.lines() {
        let upper = line.trim().to_uppercase();
        if upper.starts_with("#+BEGIN_SRC RENDER") {
            current = Some(Vec::new());
        } else if upper.starts_with("#+END_SRC") {
            if let Some(body) = current.take() {
                out.push((format!("{label}#{}", out.len()), body.join("\n")));
            }
        } else if let Some(body) = current.as_mut() {
            body.push(line);
        }
    }
    out
}

fn shipped_render_strings() -> Vec<(String, String)> {
    let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
    let mut out = Vec::new();
    for (label, yaml) in [
        ("recipe", holon_kitchen::RECIPE_PROFILE_YAML),
        ("shopping_item", holon_kitchen::SHOPPING_ITEM_PROFILE_YAML),
    ] {
        out.extend(yaml_renders(label, yaml));
    }
    let mut files: Vec<std::path::PathBuf> = ["default/types", "integrations"]
        .iter()
        .flat_map(|dir| std::fs::read_dir(assets.join(dir)).expect("asset dir exists"))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
        .collect();
    files.sort();
    for path in files {
        let text = std::fs::read_to_string(&path).expect("asset readable");
        out.extend(yaml_renders(
            &path.file_name().unwrap().to_string_lossy(),
            &text,
        ));
    }
    let mut orgs: Vec<std::path::PathBuf> = std::fs::read_dir(assets.join("default"))
        .expect("asset dir exists")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "org"))
        .collect();
    orgs.sort();
    for path in orgs {
        let text = std::fs::read_to_string(&path).expect("asset readable");
        out.extend(org_renders(
            &path.file_name().unwrap().to_string_lossy(),
            &text,
        ));
    }
    out
}

/// `to_rhai` prints a form that parses back to the same tree and prints the
/// same string again, for every shipped render string.
#[test]
fn every_shipped_render_string_round_trips_through_to_rhai() {
    let shipped = shipped_render_strings();
    assert!(
        shipped.len() >= 48,
        "the corpus lost render strings: {}",
        shipped.len()
    );
    let mut offenders = Vec::new();
    for (label, render) in &shipped {
        let first = holon_api::render_dsl::parse_render_dsl(render)
            .unwrap_or_else(|e| panic!("{label} is refused: {e:#}"));
        let printed = first.to_rhai();
        match holon_api::render_dsl::parse_render_dsl(&printed) {
            Ok(second) if second == first && second.to_rhai() == printed => {}
            Ok(second) => offenders.push(format!(
                "{label}: not a fixed point\n  printed: {printed}\n  reprinted: {}",
                second.to_rhai()
            )),
            Err(e) => offenders.push(format!(
                "{label}: printed form is refused: {e:#}\n  printed: {printed}"
            )),
        }
    }
    assert!(offenders.is_empty(), "round-trip failures: {offenders:#?}");
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
fn col_plus_string_concatenation_is_evaluated_per_row() {
    assert_per_row(r#"text(col("q") + " " + col("u"))"#, "100 g", "2 kg");
}

#[test]
fn template_string_interpolation_is_evaluated_per_row() {
    assert_per_row(r#"text(`${col("q")} ${col("u")}`)"#, "100 g", "2 kg");
}

#[test]
fn if_on_col_equality_is_decided_per_row() {
    assert_per_row(
        r#"if col("unit") == "g" { text("grams") } else { text("other") }"#,
        "grams",
        "other",
    );
}

fn typed_row(pairs: &[(&str, Value)]) -> Arc<DataRow> {
    let mut row = DataRow::new();
    for (k, v) in pairs {
        row.insert((*k).to_string(), v.clone());
    }
    Arc::new(row)
}

fn texts_for(render: &str, rows: &[Arc<DataRow>]) -> Vec<Vec<String>> {
    rows.iter()
        .map(|r| render_texts(render, r.clone()))
        .collect()
}

#[test]
fn arithmetic_on_number_columns_is_evaluated_per_row() {
    let rows = [
        typed_row(&[("a", Value::Integer(6)), ("b", Value::Integer(3))]),
        typed_row(&[("a", Value::Integer(10)), ("b", Value::Integer(5))]),
        typed_row(&[("a", Value::Float(1.5)), ("b", Value::Integer(2))]),
    ];
    assert_eq!(
        texts_for(
            r#"text((col("a") + col("b")) * 2 - col("a") / col("b"))"#,
            &rows
        ),
        vec![vec!["16"], vec!["28"], vec!["6.25"]],
    );
}

#[test]
fn comparisons_are_decided_per_row() {
    let rows = [
        typed_row(&[("n", Value::Integer(4))]),
        typed_row(&[("n", Value::Integer(5))]),
        typed_row(&[("n", Value::Float(6.5))]),
    ];
    let cases = [
        ("==", ["no", "yes", "no"]),
        ("!=", ["yes", "no", "yes"]),
        ("<", ["yes", "no", "no"]),
        ("<=", ["yes", "yes", "no"]),
        (">", ["no", "no", "yes"]),
        (">=", ["no", "yes", "yes"]),
    ];
    for (op, expected) in cases {
        let render = format!(r#"if col("n") {op} 5 {{ text("yes") }} else {{ text("no") }}"#);
        let expected: Vec<Vec<String>> = expected.iter().map(|e| vec![e.to_string()]).collect();
        assert_eq!(texts_for(&render, &rows), expected, "{render}");
    }
}

#[test]
fn boolean_and_or_not_are_decided_per_row() {
    let rows = [
        typed_row(&[("done", Value::Boolean(true)), ("n", Value::Integer(1))]),
        typed_row(&[("done", Value::Boolean(false)), ("n", Value::Integer(1))]),
        typed_row(&[("done", Value::Boolean(false)), ("n", Value::Integer(9))]),
    ];
    let render = r#"if (col("done") && col("n") < 5) || !(col("done") || col("n") < 5) { text("x") } else { text("-") }"#;
    assert_eq!(
        texts_for(render, &rows),
        vec![vec!["x"], vec!["-"], vec!["x"]],
    );
}

#[test]
fn a_value_level_if_and_a_template_choose_per_row() {
    let rows = [
        typed_row(&[("q", Value::Integer(1)), ("u", Value::String("cup".into()))]),
        typed_row(&[("q", Value::Integer(3)), ("u", Value::String("cup".into()))]),
    ];
    assert_eq!(
        texts_for(
            r#"text(`${col("q")} ${col("u")}${if col("q") == 1 { "" } else { "s" }}`)"#,
            &rows
        ),
        vec![vec!["1 cup"], vec!["3 cups"]],
    );
}

/// A missing column is missing in, missing out for arithmetic, concatenation
/// and templates (D62.a), and a missing `if` condition takes the else branch.
#[test]
fn a_missing_column_yields_a_missing_value_not_a_wrong_one() {
    let missing = typed_row(&[("other", Value::Integer(1))]);
    for render in [
        r#"text(col("n") * 2)"#,
        r#"text(col("q") + " g")"#,
        r#"text(`${col("q")} g`)"#,
    ] {
        assert_eq!(
            render_texts(render, missing.clone()),
            vec![String::new()],
            "{render}"
        );
    }
    assert_eq!(
        render_texts(
            r#"if col("flag") { text("on") } else { text("off") }"#,
            missing
        ),
        vec!["off".to_string()],
    );
}

/// A form that reads a column but is not a per-row node would be evaluated
/// once at load on the column marker; it is refused when the profile loads,
/// naming the profile, the variant and the expression.
#[test]
fn a_col_form_that_cannot_run_per_row_is_refused_at_profile_load() {
    for (render, form) in [
        (r#"text(col("q").len())"#, "method call"),
        (r#"text(col("q")[0])"#, "index"),
        (r#"text(col("q") ?? "x")"#, "??"),
    ] {
        let yaml =
            format!("entity_name: recipe\nvariants:\n  - name: card\n    render: '{render}'\n");
        let err = holon_profiles::parse_entity_profile(&yaml)
            .expect_err(&format!("{render} must be refused at load"));
        let msg = format!("{err:#}");
        for needle in ["recipe", "card", render, form] {
            assert!(
                msg.contains(needle),
                "{render}: error lacks {needle:?}: {msg}"
            );
        }
    }
}
