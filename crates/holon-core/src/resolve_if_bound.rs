//! The one rule for resolving a service whose ABSENCE changes behaviour.
//!
//! `Injector::optional_resolve_async` is `try_resolve_async().ok()`, and
//! `try_resolve().ok()` is its sync twin: both report "nothing bound it" and
//! "it is bound but the resolve failed" as the same `None`. Where the `None`
//! branch installs a permissive default — an inert guard, an unset authority,
//! a read that races the write, a reduced capability set — that collapse turns
//! a wiring fault into a silently disabled guard.
//!
//! These helpers keep the two facts apart: only `ServiceNotProvided` means the
//! container deliberately binds nothing; every other error is a wiring fault
//! and stops the boot, naming the type, the composition site and the cause.
//!
//! They live in `holon-core`, next to the guard traits they protect, so the
//! org-mode and Loro composition roots reach the same rule as the engine's.

use fluxdi::Error;
use fluxdi::ErrorKind;
use fluxdi::Injector;
use fluxdi::Shared;

/// `None` only when nothing is bound. Panics on any other resolve error.
///
/// `site` names the composition site in the panic, because the type alone does
/// not say which permissive default would have been installed instead.
pub async fn resolve_if_bound_async<T>(injector: &Injector, site: &str) -> Option<Shared<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    match injector.try_resolve_async::<T>().await {
        Ok(service) => Some(service),
        Err(e) if e.kind == ErrorKind::ServiceNotProvided => None,
        Err(e) => wiring_fault::<T>(site, &e),
    }
}

/// [`resolve_if_bound_async`] for a service registered with a sync factory.
pub fn resolve_if_bound<T>(injector: &Injector, site: &str) -> Option<Shared<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    match injector.try_resolve::<T>() {
        Ok(service) => Some(service),
        Err(e) if e.kind == ErrorKind::ServiceNotProvided => None,
        Err(e) => wiring_fault::<T>(site, &e),
    }
}

/// For a service the container GUARANTEES: panics on every resolve error,
/// "not bound" included.
///
/// Use it where a composition root has committed to a binding — a guard, where
/// a configuration without one binds the explicit no-op (SEC-bound.a). There
/// "not bound" is a wiring fault like any other, and a permissive default
/// installed here would be the fail-open this module exists to prevent.
pub async fn resolve_bound_async<T>(injector: &Injector, site: &str) -> Shared<T>
where
    T: ?Sized + Send + Sync + 'static,
{
    match injector.try_resolve_async::<T>().await {
        Ok(service) => service,
        Err(e) => wiring_fault::<T>(site, &e),
    }
}

/// The set form: empty only when nothing is bound into the set. Panics on any
/// other resolve error, including a failure of ONE member — a partial set is a
/// different set, and silently dropping a member drops whatever it enforced.
pub async fn resolve_all_if_bound_async<T>(injector: &Injector, site: &str) -> Vec<Shared<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    match injector.try_resolve_all_async::<T>().await {
        Ok(services) => services,
        Err(e) if e.kind == ErrorKind::ServiceNotProvided => Vec::new(),
        Err(e) => wiring_fault::<T>(site, &e),
    }
}

fn wiring_fault<T: ?Sized + 'static>(site: &str, e: &Error) -> ! {
    panic!(
        "{site}: resolving {} failed: {e}. Only \"not bound\" is a legal absence here; \
         taking a failed resolve for an absent service disables the guard silently.",
        std::any::type_name::<T>()
    )
}
