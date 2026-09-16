//! Contract: a collection over a NAMED row source paints its rows in a real
//! window.
//!
//! The claim needs a window. The named-source seam is typed all the way from
//! the registry to the provider, so a headless rung can prove the spec parses
//! and the provider yields rows — and still miss that the platform layer never
//! reaches `render_named` and the region lays out as nothing. That is the exact
//! shape of the inert-disclosure defect: a node no frontend dispatches occupies
//! no pixels and says nothing.
//!
//! **Scope.** The fixture is quiescent, so it cannot run the collection driver
//! `start_reactive_views` spawns; its `named_source_live` therefore builds the
//! rows statically FROM THE REGISTERED SOURCE'S PROVIDER (see `support::mod`).
//! What this pins is the platform wiring — `source` prop → `render_named` →
//! registry resolution → the holder's rows painted. What it does NOT pin is the
//! template interpretation, which needs the composed windowed harness.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! named_source_rows_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).
//!
//! @pbt kind windowed
//! @pbt covers named-row-source-renders-rows — a `live_query(#{source: ...})`
//! over a registered holder paints one row per item, with the severity colour
//! the condition's profile declares
//! @pbt slips-if-removed the named-source arm parses, resolves and yields rows
//! while the window shows an empty region, with every headless rung green

mod support;

use std::sync::Arc;

use gpui::TestAppContext;
use holon_api::Condition;
use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::condition_source::conditions_source;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_api::row_source::RowSourceRegistry;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use support::TestServices;
use support::render_reactive_fixture_quiescent_sized_with_services;

/// Two integrations, both failing to connect: `Severity::Error`, and a label
/// and detail the profile and the kind own.
fn failing_integrations() -> ConditionBus {
    let bus = ConditionBus::new();
    for name in ["todoist", "claude-history"] {
        bus.emit(Condition {
            subject: name.to_string(),
            reason: ConditionKind::IntegrationConnectFailed {
                integration: name.to_string(),
                error: "binary not found on PATH".to_string(),
            },
        });
    }
    bus
}

fn registry(bus: &ConditionBus) -> Arc<RowSourceRegistry> {
    let mut registry = RowSourceRegistry::new();
    registry
        .register(conditions_source(bus))
        .expect("the first registration on a fresh registry cannot collide");
    Arc::new(registry)
}

fn named(name: &str, value: RenderExpr) -> Arg {
    Arg {
        name: Some(name.to_string()),
        value,
    }
}

fn positional(value: RenderExpr) -> Arg {
    Arg { name: None, value }
}

fn call(name: &str, args: Vec<Arg>) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: name.to_string(),
        args,
    }
}

fn literal(text: &str) -> RenderExpr {
    RenderExpr::Literal {
        value: holon_api::Value::String(text.to_string()),
    }
}

/// `list(#{item_template: text(col("subject"))})` — one painted row per
/// condition, showing a column the source's own row map minted. The shape
/// mirrors the shipped sidebar template in `assets/default/index.org`:
/// `item_template` is named, everything a widget wraps is positional.
fn item_template() -> RenderExpr {
    call(
        "list",
        vec![named(
            "item_template",
            call(
                "text",
                vec![positional(call(
                    "col",
                    vec![positional(literal("subject"))],
                ))],
            ),
        )],
    )
}

/// The node the `live_query` builder emits for a named source: the source name
/// and the serialized template, exactly as `shadow_builders::live_query` writes
/// the props.
fn named_source_node() -> Arc<ReactiveViewModel> {
    let props = [
        ("source", "conditions".to_string()),
        (
            "render_expr",
            serde_json::to_string(&item_template()).expect("the template serializes"),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), holon_api::Value::String(v)))
    .collect();

    let mut node = ReactiveViewModel::from_widget("live_query", props);
    // The builder always leaves a slot; the platform layer falls back to it
    // when the props are incomplete, and asserts on its presence otherwise.
    node.slot = Some(holon_frontend::reactive_view_model::ReactiveSlot::new(
        ReactiveViewModel::from_widget("column", Default::default()),
    ));
    Arc::new(node)
}

fn size() -> gpui::Size<gpui::Pixels> {
    gpui::size(gpui::px(900.0), gpui::px(600.0))
}

#[gpui::test]
fn a_named_source_paints_one_row_per_item(cx: &mut TestAppContext) {
    let bus = failing_integrations();
    let registry = registry(&bus);
    let snap = render_reactive_fixture_quiescent_sized_with_services(
        cx,
        named_source_node(),
        size(),
        TestServices::with_row_sources(registry),
    );

    let elements = &snap.entries;
    eprintln!(
        "DUMP:
{}",
        snap.dump()
    );
    assert!(
        !elements.is_empty(),
        "the window tracked NO elements at all — the fixture painted nothing, so every \
         assertion below would be vacuous"
    );

    for subject in ["todoist", "claude-history"] {
        assert!(
            elements.iter().any(|(_, info)| info
                .displayed_text
                .as_deref()
                .is_some_and(|t| t.contains(subject))),
            "no painted element shows `{subject}`, so the named source's rows did not reach \
             the window. Tracked: {:?}",
            elements
                .iter()
                .filter_map(|(id, info)| info.displayed_text.clone().map(|t| (id.clone(), t)))
                .collect::<Vec<_>>()
        );
    }
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
