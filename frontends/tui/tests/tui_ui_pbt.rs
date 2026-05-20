//! TUI UI PBT (Full wiring — Loro enabled) — geometry-based PBT against the
//! same shared state machine `frontends/gpui/tests/gpui_ui_pbt.rs` exercises
//! against GPUI.
//!
//! Mirrors `gpui_ui_pbt` topologically: a PBT thread runs the property
//! state machine on a background thread; the main thread owns the
//! renderer; a shared `Arc<DebugServices>` plumbs `interaction_tx` and
//! `user_driver` between them; the readiness gate fires when the
//! frontend's `GeometryProvider` reports an element with
//! `has_content && entity_id.is_some()`.
//!
//! The shared body lives in `common::pbt_main`; the no-Loro / SqlOnly
//! variant is its own test target (`tui_ui_pbt_no_loro`) so it runs
//! automatically.
//!
//! TUI deviations from GPUI (intentional, see plan §Architecture decisions):
//!
//! - Renderer drives `app_render` directly instead of going through
//!   `r3bl_tui::main_event_loop_impl`. Reason: r3bl's input device is a
//!   closed stream (`MockInputDevice` exhausts → loop breaks), and we
//!   need the renderer to keep producing frames as the engine fires
//!   CDC throughout the PBT. The watch task that the first
//!   `app_render` spawns sends
//!   `TerminalWindowMainThreadSignal::Render` through our channel; we
//!   loop on that signal to drive subsequent frames.
//! - Screenshots come from `OffscreenBufferBackend` painting the
//!   `OffscreenBuffer` we compose in `CapturingApp::app_render`, not
//!   from xcap. Same RGBA8 contract — `analyze_screenshot_emptiness`
//!   sees content when any cell has a non-blank glyph with a bright
//!   foreground color.
//!
//! `harness = false`. Run with: `cargo test -p holon-tui --test tui_ui_pbt`.
//!
//! ## Environment variables
//!
//! - `PROPTEST_SEED=<u64>` — pin the random seed (proptest standard).
//! - `PBT_ATOMIC_EDITOR=1` — set automatically here. Routes editing
//!   through the keyboard-driven primitives (FocusEditableText /
//!   TypeChars / ...) instead of the bypass `EditViaViewModel`/
//!   `EditViaDisplayTree` transitions.
//! - `HOLON_PBT_WEIGHTS=Indent:200,Outdent:200,Move*:50` — bias the
//!   strategy aggregator toward (or away from) named transition
//!   variants. Comma-separated `pattern:multiplier` pairs; pattern
//!   may include a single `*` glob (prefix / suffix / contains).
//!   Multiplier `0` removes the variant entirely. Defaults to `1`
//!   for unmatched variants. See
//!   `crates/holon-integration-tests/src/pbt/transition_dispatch.rs`.
//! - `HOLON_LORO_PEER_ID=1` and `PBT_MEMORY_MULTIPLIER=15` — set
//!   automatically here.

mod common;

fn main() {
    common::pbt_main::run(holon_pbt_core::Wiring::full(), "tui_ui_pbt");
}
