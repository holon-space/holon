//! DI registration for the Turso-free ("block-query") frontend (ADR 0004
//! Phase 9; relocated from `holon-frontend::block_query_frontend` in storage
//! de-leak Stage 6).
//!
//! The Turso frontend
//! ([`add_frontend`](crate::wiring::FrontendInjectorExt::add_frontend))
//! builds its [`FrontendSession`] from a `BackendEngine`. A no-engine session
//! instead renders structural blocks from an `Arc<dyn BlockQuerySource>`
//! already registered in the container (e.g. by
//! `register_loro_block_query_source`).
//!
//! This registers the matching `FrontendSession` + `ReactiveEngine` providers
//! so the storage backend is a **DI choice**, not a branch in the caller: both
//! the Turso and the block-query containers expose `FrontendSession` and
//! `ReactiveEngine`, and callers resolve them the same way. After resolving the
//! engine, the caller populates the `BuilderServicesSlot` exactly as the Turso
//! path does (the slot breaks the engine↔interpreter cycle).

use std::sync::Arc;

use fluxdi::Injector;
use fluxdi::Provider;
use fluxdi::Shared;
use holon_core::storage::BlockQuerySource;
use holon_frontend::reactive::make_interpret_fn;
use holon_frontend::reactive::BuilderServicesSlot;
use holon_frontend::reactive::RenderInterpreterInjectorExt;
use holon_frontend::render_services::register_render_services;
use holon_frontend::FrontendSession;
use holon_frontend::SessionParts;

/// Register the block-query (Turso-free) `FrontendSession` + `ReactiveEngine`
/// stack into `injector`.
///
/// Requires an `Arc<dyn BlockQuerySource>` to already be registered in the
/// container (the storage adapter, e.g. the Loro read stack). The registered
/// providers mirror the backend-agnostic half of `add_frontend` — interpreter,
/// `BuilderServicesSlot`, and the `ReactiveEngine` factory — plus a
/// `FrontendSession` factory that builds
/// [`FrontendSession::from_block_query_source`] instead of resolving a
/// `BackendEngine`.
pub fn register_block_query_frontend(injector: &Injector) {
    injector.provide::<FrontendSession>(Provider::root(|resolver| {
        let block_query = resolver.resolve::<dyn BlockQuerySource>();
        // The operation capability is optional: present when a storage adapter
        // registered one (e.g. `register_loro_operation_engine`), absent for a
        // read-only no-Turso session. The session degrades structurally either way.
        let operation_engine = resolver
            .try_resolve::<dyn holon::api::OperationEngine>()
            .ok();
        Shared::new(from_block_query_source(block_query, operation_engine))
    }));

    // Render stack (slot + shared shadow interpreter + ReactiveEngine
    // factory) — backend-agnostic, lives in holon-frontend.
    register_render_services(injector);

    let slot = injector.resolve::<BuilderServicesSlot>();
    injector.set_render_interpreter(make_interpret_fn(slot.0.clone()));
}

/// Wrap a Turso-free [`BlockQuerySource`] into a no-Turso `FrontendSession`
/// (ADR 0004 Phase 9). The session has **no** `BackendEngine`; the render
/// path reads structural blocks from `block_query` (via a `LoroUiWatcher`)
/// instead. Turso-only methods (`watch_query`, …) fail loud if called. The
/// operation path (dispatch, undo/redo) routes through `operation_engine`:
/// pass `Some` for Loro-native mutations (Stage 4), `None` to leave it
/// read-only.
pub fn from_block_query_source(
    block_query: Arc<dyn BlockQuerySource>,
    operation_engine: Option<Arc<dyn holon::api::OperationEngine>>,
) -> FrontendSession {
    let ui_watcher = Arc::new(holon::api::loro_ui_watcher::LoroUiWatcher::new(
        block_query.clone(),
    )) as Arc<dyn holon::api::UiWatcher>;
    let profiles = holon::api::loro_ui_watcher::build_turso_free_profile_resolver();
    FrontendSession::from_parts(SessionParts::with_capabilities(
        None,
        block_query,
        operation_engine,
        ui_watcher,
        profiles,
    ))
}
