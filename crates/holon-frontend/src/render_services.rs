//! DI registrations for the render stack — the backend-agnostic half of the
//! session wiring, shared by the Turso and no-Turso assemblies in `holon-app`.
//!
//! Lives here (not in the wiring crate) because the shadow interpreter is
//! deliberately crate-private: `build_shadow_interpreter` must stay the single
//! construction site (see `shadow_builders` docs). The wiring crate calls
//! [`register_render_services`] and never names `RenderInterpreter` directly.

use std::sync::Arc;

use fluxdi::{Injector, Provider, Shared};

use crate::reactive::{BuilderServicesSlot, ReactiveEngine, RenderInterpreterFn};
use crate::render_interpreter::RenderInterpreter;
use crate::{FrontendSession, ReactiveViewModel};

/// Register the render stack: `BuilderServicesSlot` (OnceLock that breaks the
/// engine↔interpreter cycle, populated by the platform shell after resolving
/// the engine), the shared shadow interpreter (built ONCE here), and the
/// `ReactiveEngine` factory.
///
/// The `ReactiveEngine` provider only resolves if `set_render_interpreter`
/// was called by the frontend (Flutter doesn't use `ReactiveEngine`).
pub fn register_render_services(injector: &Injector) {
    // BuilderServicesSlot — OnceLock for circular dep breaking (GPUI)
    injector.provide::<BuilderServicesSlot>(Provider::root(|_| {
        Shared::new(BuilderServicesSlot(Arc::new(std::sync::OnceLock::new())))
    }));

    // Shared shadow interpreter — built ONCE here and handed to every
    // consumer via DI. This is the only place in the entire frontend
    // where the interpreter is constructed; see `build_shadow_interpreter`
    // for the rationale behind crate-private visibility.
    //
    // Everything downstream reaches interpretation via
    // `BuilderServices::interpret` / `interpret_with_source` — no call
    // site ever names `RenderInterpreter` directly. Registered as
    // `RenderInterpreter<ReactiveViewModel>` so fluxdi resolves it as
    // `Shared<_>` (i.e. `Arc<_>`).
    let shadow_interpreter_shared = Shared::new(crate::shadow_builders::build_shadow_interpreter());
    injector.provide::<RenderInterpreter<ReactiveViewModel>>(Provider::root({
        let s = shadow_interpreter_shared;
        move |_| s.clone()
    }));

    // ReactiveEngine factory — resolves FrontendSession + RenderInterpreterFn
    // + the shared shadow interpreter from DI.
    injector.provide::<ReactiveEngine>(Provider::root(|resolver| {
        let session = resolver.resolve::<FrontendSession>();
        let interpret = resolver.resolve::<RenderInterpreterFn>();
        let interpreter = resolver.resolve::<RenderInterpreter<ReactiveViewModel>>();
        let services_slot = resolver.resolve::<BuilderServicesSlot>();
        let f = interpret.0.clone();
        Shared::new(ReactiveEngine::new(
            session,
            tokio::runtime::Handle::current(),
            interpreter,
            move |expr, rows| f(expr, rows),
            services_slot.0.clone(),
        ))
    }));
}
