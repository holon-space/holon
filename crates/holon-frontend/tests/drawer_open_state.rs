//! The `drawer` builder stamps the view store's open state onto the snapshot.
//!
//! The keystone's `inv-drawer-open-matches-ref` proves the field is populated
//! and agrees with the reference, but no generated sequence closes a drawer, so
//! it only ever observes the OPEN direction. These tests pin the closed one
//! through the same production path (shadow builder → `ReactiveViewModel` →
//! `snapshot()`), which is where D26's bug lived: without a stamped `open` a
//! snapshot-driven frontend can only render every drawer open.

use holon_api::render_types::RenderExpr;
use holon_frontend::DRAWER_TOGGLE_WIDTH;
use holon_frontend::LayoutHint;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::view_model::DrawerMode;
use holon_frontend::view_model::ViewKind;

const SIDEBAR: &str = "block:sidebar";

fn snapshot_drawer(services: &StubBuilderServices, dsl: &str) -> (bool, DrawerMode, LayoutHint) {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr: RenderExpr = holon_api::render_dsl::parse_render_dsl(dsl).expect("drawer dsl parses");
    let vm = services.interpret(&expr, &RenderContext::default());
    let snap = vm.snapshot();
    let ViewKind::Drawer { open, mode, .. } = snap.kind else {
        panic!("expected a Drawer node, got {:?}", snap.kind);
    };
    (open, mode, snap.layout_hint)
}

#[test]
fn a_stored_closed_bit_reaches_the_snapshot() {
    let services = StubBuilderServices::new().with_widget_open(SIDEBAR, false);
    let (open, mode, hint) =
        snapshot_drawer(&services, &format!("drawer(\"{SIDEBAR}\", text(\"x\"))"));

    assert_eq!(mode, DrawerMode::Shrink);
    assert!(!open, "a closed store bit must reach ViewKind::Drawer.open");
    assert_eq!(
        hint,
        LayoutHint::Fixed {
            px: DRAWER_TOGGLE_WIDTH
        },
        "a closed shrink drawer reserves only its toggle strip"
    );
}

#[test]
fn a_stored_open_bit_reserves_the_full_width() {
    let services = StubBuilderServices::new().with_widget_open(SIDEBAR, true);
    let (open, _, hint) = snapshot_drawer(
        &services,
        &format!("drawer(\"{SIDEBAR}\", text(\"x\"), #{{width: 240}})"),
    );

    assert!(open);
    assert_eq!(hint, LayoutHint::Fixed { px: 240.0 });
}

#[test]
fn an_overlay_drawer_claims_no_flow_space_in_either_state() {
    // Overlays float above siblings, so the reserved width is 0 whether they
    // are open or closed — only the panel itself appears or does not.
    for open in [true, false] {
        let services = StubBuilderServices::new().with_widget_open(SIDEBAR, open);
        let (snap_open, mode, hint) = snapshot_drawer(
            &services,
            &format!("drawer(\"{SIDEBAR}\", text(\"x\"), #{{mode: \"overlay\"}})"),
        );
        assert_eq!(mode, DrawerMode::Overlay);
        assert_eq!(snap_open, open);
        assert_eq!(hint, LayoutHint::Fixed { px: 0.0 });
    }
}

/// The mode-aware default for an UNTRACKED drawer (Shrink open, Overlay closed)
/// belongs to the session-backed services, not to the stub:
/// `StubBuilderServices` deliberately reports every widget as explicit,
/// preserving the legacy open-by-default semantics its gallery/layout consumers
/// rely on (`BuilderServices::widget_state_explicit`). The rule itself is
/// pinned by `DrawerMode::default_open`'s own unit test; what these tests own
/// is that whatever the store resolves to reaches the snapshot.
#[test]
fn the_mode_default_rule_is_the_one_the_builder_applies() {
    assert!(DrawerMode::Shrink.default_open());
    assert!(!DrawerMode::Overlay.default_open());
}
