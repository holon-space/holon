//! The enrich→render round trip must not resurrect an UNBOUND computed field
//! as a `unit` binding.
//!
//! Type-aware binding represents "this row is the wrong shape for this computed
//! field" as ABSENCE from the Rhai scope, so a dependent variant condition sees
//! a missing required column and returns a typed non-match without invoking
//! Rhai. The enrichment boundary, however, writes `Value::Null` into the OUTPUT
//! row for those fields (row shape is part of its contract), and that row is
//! what the render seat later resolves. If the render seat pushes that Null
//! into scope as `()`, the field is no longer absent — it is a unit value — and
//! every condition that ANDs it type-errors and degrades.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use holon_api::Value;
use holon_api::entity_profile::EntityProfile;
use rhai::Engine as RhaiEngine;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::Registry;

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<String>>>);

#[derive(Default)]
struct MessageVisitor {
    message: String,
    condition: String,
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "condition" {
            self.condition = value.to_string();
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let mut visitor = MessageVisitor::default();
        event.record(&mut visitor);
        self.0.lock().expect("capture poisoned").push(format!(
            "[{}] condition={:?} {}",
            event.metadata().level(),
            visitor.condition,
            visitor.message
        ));
    }
}

/// The production resolver registers one Rhai lookup fn per live entity
/// (`register_entity_lookups`), returning `()` for a miss. `is_program` and
/// `has_query_source` call `rule_sibling` / `query_source`, so an engine
/// without them is not a faithful stand-in for the render seat — those fields
/// would error and land in scope as `()`, masking the round-trip defect under a
/// wiring artefact.
fn test_engine() -> RhaiEngine {
    let mut engine = RhaiEngine::new();
    engine.register_fn("rule_sibling", |_: String| rhai::Dynamic::UNIT);
    // A HIT for one id, so `has_query_source` can be true and the two
    // `has_query_source && …` conditions are actually reachable. A lookup that
    // always missed would short-circuit them out of the sweep.
    engine.register_fn("query_source", |id: String| {
        if id.contains("query-owner") {
            rhai::Dynamic::from(rhai::Map::new())
        } else {
            rhai::Dynamic::UNIT
        }
    });
    engine
}

fn block_profile() -> EntityProfile {
    let registry = holon_profiles::type_registry::create_default_registry()
        .expect("default registry must build");
    holon_profiles::type_registry::type_profiles_from_registry(&registry)
        .into_iter()
        .find(|p| p.entity_name.as_str() == "block")
        .expect("block profile must exist in the default registry")
}

/// A plain block as the projection carries it: no `tags` column, so
/// `is_page_row` (`tags != () && …`) is structurally unbound.
fn plain_block_row() -> HashMap<String, Value> {
    let mut row: HashMap<String, Value> = HashMap::new();
    row.insert("id".into(), Value::String("block:plain".into()));
    row.insert("parent_id".into(), Value::String("block:root".into()));
    row.insert("content".into(), Value::String("hello".into()));
    row.insert("content_type".into(), Value::String("text".into()));
    row.insert("source_language".into(), Value::Null);
    row
}

fn resolve_capturing(
    profile: &EntityProfile,
    row: &HashMap<String, Value>,
) -> (Option<String>, Vec<String>) {
    let capture = Capture::default();
    let subscriber = Registry::default().with(capture.clone());
    let engine = test_engine();
    let variant = tracing::subscriber::with_default(subscriber, || {
        profile.resolve(row, &engine).map(|p| p.name.clone())
    });
    let events = capture.0.lock().expect("capture poisoned").clone();
    (variant, events)
}

/// The escape: enrich once (as `ui_watcher::enrich_row` does), then render the
/// ENRICHED row. The second pass must be as quiet as the first — an unbound
/// computed field stays unbound, it does not become a `()` that type-errors
/// `is_page_row && …`.
#[test]
fn enriched_null_computed_field_does_not_degrade_the_page_row_condition() {
    let profile = block_profile();
    let raw = plain_block_row();

    // Seat B (enrichment): computed fields only. Unbound fields come back Null.
    let computed = profile.compute_fields_only(&raw, &test_engine());
    assert_eq!(
        computed.get("is_page_row"),
        Some(&Value::Null),
        "precondition: the enrichment boundary reports an unbound field as Null"
    );

    let (_, raw_events) = resolve_capturing(&profile, &raw);
    assert!(
        !raw_events
            .iter()
            .any(|e| e.contains("treated as non-match")),
        "raw row must resolve without a degraded condition, got: {raw_events:#?}"
    );

    // Seat A (render) over the enriched row — exactly what the frontend does.
    let mut enriched = raw;
    for (k, v) in computed {
        enriched.insert(k, v);
    }
    let (variant, events) = resolve_capturing(&profile, &enriched);

    let degraded: Vec<&String> = events
        .iter()
        .filter(|e| e.contains("treated as non-match"))
        .collect();
    assert!(
        degraded.is_empty(),
        "rendering an ENRICHED row degraded {} condition(s) — an unbound computed \
         field was resurrected as a unit binding: {degraded:#?}",
        degraded.len()
    );
    assert!(variant.is_some(), "a variant must still resolve");
}

/// The round trip must also be value-STABLE: enriching an enriched row twice
/// yields the same computed map, and a page row keeps matching `embedded_page`.
#[test]
fn page_row_still_selects_embedded_page_after_a_round_trip() {
    let profile = block_profile();
    let mut row = plain_block_row();
    row.insert("id".into(), Value::String("block:page".into()));
    row.insert("tags".into(), Value::String("[\"Page\"]".into()));

    let computed = profile.compute_fields_only(&row, &test_engine());
    assert_eq!(computed.get("is_page_row"), Some(&Value::Boolean(true)));

    let mut enriched = row;
    for (k, v) in computed.clone() {
        enriched.insert(k, v);
    }
    let (variant, events) = resolve_capturing(&profile, &enriched);
    assert!(
        !events.iter().any(|e| e.contains("treated as non-match")),
        "page row degraded on the second pass: {events:#?}"
    );
    assert_eq!(
        profile.compute_fields_only(&enriched, &test_engine()),
        computed,
        "computed fields must be idempotent across the enrich→render round trip"
    );
    assert_eq!(variant.as_deref(), Some("embedded_page"));
}

/// The class sweep: EVERY shipped block variant condition, over every row shape
/// the projection produces. The realistic shape that produced the reported
/// sighting degraded THREE of them (`is_page_row && …` and both
/// `has_query_source && …`); drop `content_type` as well and FIVE degrade. The
/// defect is the round trip, not any single condition — so the sweep is by row
/// shape, and a newly added condition of the class reds here without an edit.
#[test]
fn no_shipped_block_condition_degrades_on_any_enriched_row_shape() {
    let profile = block_profile();

    // (label, columns to add, columns to remove)
    let shapes: Vec<(&str, Vec<(&str, Value)>, Vec<&str>)> = vec![
        ("plain", vec![], vec![]),
        (
            "page",
            vec![("tags", Value::String("[\"Page\"]".into()))],
            vec![],
        ),
        (
            "source",
            vec![
                ("content_type", Value::String("source".into())),
                ("source_language", Value::String("holon_sql".into())),
            ],
            vec![],
        ),
        (
            "rule-head",
            vec![
                ("content_type", Value::String("source".into())),
                ("source_language", Value::String("holon_rule".into())),
            ],
            vec![],
        ),
        (
            "widget-only-query",
            vec![
                ("content_type", Value::String("source".into())),
                ("source_language", Value::String("prql".into())),
                ("widget_only", Value::Integer(1)),
            ],
            vec![],
        ),
        (
            "task",
            vec![("task_state", Value::String("TODO".into()))],
            vec![],
        ),
        ("collapsed", vec![("collapsed", Value::Integer(1))], vec![]),
        // The reported sighting's shape: the block owns a query source, and
        // `widget_only` is absent so `is_widget_only` is unbound — pre-fix this
        // degrades THREE conditions at once.
        ("query-owner", vec![], vec![]),
        // The whole-class shape: `content_type` genuinely absent unbinds
        // is_source/is_image too, so five conditions degrade pre-fix.
        ("no-content-type", vec![], vec!["content_type"]),
        // Every column present but NULL — the shape a SQL row with no
        // properties actually has. Each computed field resolves to a real bool.
        (
            "all-columns-null",
            vec![
                ("tags", Value::Null),
                ("collapsed", Value::Null),
                ("widget_only", Value::Null),
                ("task_state", Value::Null),
            ],
            vec![],
        ),
        // Adversarial: keys the profile knows nothing about, one of them Null.
        // They must be pushed to scope untouched and disturb nothing — only
        // COMPUTED names are skipped.
        (
            "extra-unknown-keys",
            vec![
                ("tags", Value::String("[\"Page\"]".into())),
                ("some_unknown_column", Value::Null),
                ("another_unknown", Value::String("noise".into())),
                ("third_unknown", Value::Integer(0)),
            ],
            vec![],
        ),
    ];

    // Collected, not asserted per shape: a failure must report the WHOLE class
    // inventory (which shapes, which conditions), not just the first shape.
    let mut failures: Vec<String> = Vec::new();

    for (label, extra, remove) in shapes {
        let mut row = plain_block_row();
        row.insert("id".into(), Value::String(format!("block:{label}")));
        for (k, v) in extra {
            row.insert(k.to_string(), v);
        }
        for k in remove {
            row.remove(k);
        }
        let computed = profile.compute_fields_only(&row, &test_engine());
        let mut enriched = row;
        for (k, v) in computed {
            enriched.insert(k, v);
        }
        let (variant, events) = resolve_capturing(&profile, &enriched);
        let degraded: Vec<&String> = events
            .iter()
            .filter(|e| e.contains("treated as non-match"))
            .collect();
        if !degraded.is_empty() {
            failures.push(format!(
                "shape '{label}': {} condition(s) degraded: {degraded:#?}",
                degraded.len()
            ));
        }
        if variant.is_none() {
            failures.push(format!("shape '{label}': resolved no variant"));
        }
    }

    assert!(
        failures.is_empty(),
        "the enrich→render round trip degraded shipped conditions:\n{}",
        failures.join("\n")
    );
}

/// A row key that SHADOWS a computed field with a wrong, stale value must lose
/// to render-time recomputation. `Change::FieldsChanged` updates a row in place
/// without re-running enrichment, so a stale computed value genuinely reaches
/// the render seat; the seat owning those names is what makes it harmless.
#[test]
fn a_stale_shadowing_computed_value_loses_to_recomputation() {
    let profile = block_profile();
    let mut row = plain_block_row();
    row.insert("id".into(), Value::String("block:stale".into()));
    row.insert("tags".into(), Value::String("[\"Page\"]".into()));

    // The value a pre-tag enrichment would have left behind, now contradicted
    // by the row's own `tags`.
    row.insert("is_page_row".into(), Value::Boolean(false));

    let (variant, events) = resolve_capturing(&profile, &row);
    assert!(
        !events.iter().any(|e| e.contains("treated as non-match")),
        "stale shadowing value degraded a condition: {events:#?}"
    );
    assert_eq!(
        variant.as_deref(),
        Some("embedded_page"),
        "the render seat must recompute `is_page_row` from `tags`, not trust the row"
    );
}

/// Fail-loud is intact: a computed field that errors on columns that ARE
/// present still yields a `()` binding and still degrades LOUDLY. The fix
/// removes the false degrade from the round trip, not the true one.
#[test]
fn a_genuine_type_error_still_degrades_loudly() {
    let profile = block_profile();
    let mut row = plain_block_row();
    // `tags` present but numeric: `tags.contains("\"Page\"")` has no overload,
    // so is_page_row is a real runtime error, not a structural absence.
    row.insert("tags".into(), Value::Integer(7));

    let (_, events) = resolve_capturing(&profile, &row);
    assert!(
        events
            .iter()
            .any(|e| e.contains("computed field eval failed on PRESENT columns")),
        "a genuine computed-field type error must still be disclosed: {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|e| e.contains("treated as non-match") && e.contains("is_page_row &&")),
        "the dependent condition must still report its DEGRADED state: {events:#?}"
    );
}
