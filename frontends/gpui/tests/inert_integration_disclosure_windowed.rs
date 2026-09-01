//! Contract: the disclosure for a view over a not-connected integration is
//! actually PAINTED in a real window.
//!
//! The headless pin (`holon-frontend/tests/inert_integration_disclosure.rs`)
//! proves the node keeps a kind every frontend dispatches. This proves the
//! consequence that motivated it: a widget kind GPUI does not register lays
//! out as nothing, so the user sees a blank region where the explanation
//! should be. Only a windowed render can see that.
//!
//! This is the PINNED coverage for the disclosure surface. Do not cite
//! `keystone-smoke` for it: `inv-viewmodel-no-error-widgets` engages
//! non-deterministically there (observed across runs: 17/17, 15/15, 36/36,
//! 24/24, and deselected), so its engagement is a coin flip rather than a
//! guarantee.
//!
//! @pbt kind windowed
//! @pbt covers inert-integration-disclosure-paints — a block whose tables
//! belong to an integration that is not connected renders a laid-out banner
//! whose full disclosure sentence occupies the window, rather than an empty
//! region (BugFunnel 2026-08-31 ENVIRONMENT/ORACLE)
//! @pbt overlaps general_e2e_composed_pbt — kept: the keystone is headless and
//! cannot see whether a node occupies pixels

mod support;

use std::sync::Arc;

use gpui::TestAppContext;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use support::render_fixture;

const DISCLOSURE: &str = "Claude History is not connected — status: Unavailable (binary \
                          'claude-history-mcp' not found on PATH)";

fn props(pairs: &[(&str, &str)]) -> std::collections::HashMap<String, holon_api::Value> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), holon_api::Value::String(v.to_string())))
        .collect()
}

fn error_node(message: &str) -> Arc<ReactiveViewModel> {
    Arc::new(ReactiveViewModel::from_widget(
        "error",
        props(&[
            ("message", message),
            ("degraded_disclosure", message),
            ("integration", "claude-history"),
        ]),
    ))
}

/// The node `ui_watcher::disclosed_render_expr` emits.
fn disclosed_node() -> Arc<ReactiveViewModel> {
    error_node(DISCLOSURE)
}

#[gpui::test]
fn the_inert_integration_disclosure_is_laid_out_in_a_real_window(cx: &mut TestAppContext) {
    let snap = render_fixture(cx, disclosed_node());

    let painted: Vec<_> = snap.of_type("error").collect();
    assert_eq!(
        painted.len(),
        1,
        "exactly one banner, not one per missing table; dump:\n{}",
        snap.dump()
    );
    let banner = painted[0];
    assert!(
        banner.width > 0.0 && banner.height > 0.0,
        "the banner must occupy the window; got {}x{}\n{}",
        banner.width,
        banner.height,
        snap.dump()
    );
}

/// The banner must paint the WHOLE sentence, not a truncated or placeholder
/// stub: the same widget carrying a one-word message lays out strictly
/// shorter, so the disclosure's extra height is the sentence itself on screen.
#[gpui::test]
fn the_whole_disclosure_sentence_occupies_the_banner(cx: &mut TestAppContext) {
    let stub = render_fixture(cx, error_node("x"));
    let stub_height = stub
        .of_type("error")
        .next()
        .expect("stub banner renders")
        .height;

    let disclosed = render_fixture(cx, disclosed_node());
    let disclosed_height = disclosed
        .of_type("error")
        .next()
        .expect("disclosure banner renders")
        .height;

    assert!(
        disclosed_height > stub_height,
        "the full disclosure ({DISCLOSURE:?}) must lay out taller than a one-character message — \
         equal heights would mean the sentence was truncated to a single line or never painted; \
         got {disclosed_height} vs {stub_height}"
    );
}

/// The refuted shape, kept as the contrast: a widget kind GPUI does not
/// register produces no banner at all — the blank region a user got instead of
/// an explanation.
#[gpui::test]
fn a_bespoke_widget_kind_lays_out_nothing(cx: &mut TestAppContext) {
    let bespoke = Arc::new(ReactiveViewModel::from_widget(
        "degraded",
        props(&[("message", DISCLOSURE)]),
    ));

    let snap = render_fixture(cx, bespoke);

    assert_eq!(
        snap.of_type("error").count(),
        0,
        "sanity: the bespoke kind reaches no error builder; dump:\n{}",
        snap.dump()
    );
    assert_eq!(
        snap.of_type("degraded").count(),
        0,
        "and nothing registers its own name either; dump:\n{}",
        snap.dump()
    );
}
