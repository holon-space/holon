//! Fail-loud contract for the MCP `scroll` tool (dogfood #3).
//!
//! The tool used to dispatch a synthetic off-cursor `ScrollWheel` that gpui's
//! `Hitbox::should_handle_scroll` hover-gate silently dropped, then report
//! success anyway — "reported success while nothing moved." The fix drives the
//! target panel's `ListState::scroll_by` DIRECTLY via `scroll_list_by`, which
//! returns `Ok(false)` when no scrollable list is reachable; the interaction
//! pump turns that into a rich error and the driver bails instead of faking
//! success.
//!
//! This pins the detection half of that contract: an unreachable target →
//! `Ok(false)`, a malformed entity id → `Err`. (The pump/driver wiring that
//! turns those into a surfaced error is exercised live.)
//!
//! Run: `cargo test -p holon-gpui --test mcp_scroll_fail_loud`

use holon_gpui::entity_view_registry::EntityCache;
use holon_gpui::scroll_list_by;

/// With no panel shells cached, every scroll target is unreachable, so
/// `scroll_list_by` reports not-scrolled (`Ok(false)`) — never a fake success —
/// and a malformed entity id fails loud.
#[gpui::test]
fn unreachable_target_reports_not_scrolled_never_fakes_success(cx: &mut gpui::TestAppContext) {
    cx.update(|cx| {
        let empty: EntityCache = Default::default();
        assert_eq!(
            scroll_list_by("block:default-main-panel", 100.0, &empty, cx),
            Ok(false),
            "an unreachable target must report not-scrolled (→ loud error), not fake success"
        );
        assert!(
            scroll_list_by("not a valid uri", 100.0, &empty, cx).is_err(),
            "a malformed entity id must fail loud, not silently no-op"
        );
    });
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;
