//! WaterUI frontend — PARKED / bit-rotted experimental spike.
//!
//! RECOMMENDED FOR DELETION (crate + its root-`Cargo.toml` `exclude` entry).
//! Single commit (2026-01-26); `HANDOFF.md` calls it a "Parked experimental
//! frontend". Because the crate is workspace-excluded (for an unrelated
//! wgpu/naga/codespan build error), CI never compiled it, so its wiring rotted
//! silently against symbols deleted by later refactors:
//!
//!   * `holon_frontend::cdc::spawn_ui_listener` + `AppState`  → deleted in the
//!     ReactiveEngine rewrite. Replacement: `ReactiveEngine::ensure_watching`
//!     (spawns the `watch_ui` CDC pump internally) + `ReactiveRenderedRows`.
//!   * `RenderContext::new(session, rt)`                      → deleted;
//!     `RenderContext` no longer holds a session/runtime. Use
//!     `RenderContext::default().with_data_rows(..)`.
//!   * `FrontendSession::watch_ui(id, None, true)`            → signature is
//!     now `watch_ui(&EntityUri) -> WatchHandle`.
//!   * `holon_frontend::frontend_module::FrontendInjectorExt` / `add_frontend`
//!     → the module was deleted; frontend DI wiring moved to the `holon-app`
//!     crate (`holon_app::FrontendInjectorExt::add_frontend`).
//!   * `RenderInterpreter::interpret(expr, ctx)`              → now takes a
//!     third `services: &dyn BuilderServices` argument.
//!
//! Restoring a live frontend is a real migration, not a symbol swap, and needs
//! deps this excluded crate does not carry (a `holon-app` path-dep for
//! `add_frontend`, and a `futures_signals` reactive bridge to push
//! `ReactiveRenderedRows` snapshots into a waterui `Binding`). The template to
//! copy is `frontends/tui/src/di.rs` (DI module) + `ReactiveEngine::
//! ensure_watching` / `snapshot()` in `crates/holon-frontend/src/reactive.rs`.
//!
//! Until that decision is made, this module is reduced to a compile-clean shell
//! that references ONLY live symbols and renders a static placeholder view. It
//! does NOT boot the holon backend. `mod render` (the builder registry) is kept
//! on disk pending the delete-vs-revive decision but is currently unused.

#![allow(dead_code)]

mod render;

use waterui::app::App;
use waterui::prelude::*;

/// PARKED entry point — see the module docs. Renders a static placeholder
/// instead of booting the (deleted) CDC/ReactiveEngine pipeline, so the crate
/// references only symbols that currently exist.
pub fn app(env: Environment) -> App {
    App::new(
        || {
            AnyView::new(text(
                "WaterUI frontend is parked — see frontends/waterui/HANDOFF.md",
            ))
        },
        env,
    )
}

waterui_ffi::export!();
