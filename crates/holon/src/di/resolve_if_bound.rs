//! The one rule for resolving a service whose ABSENCE changes behaviour.
//!
//! `Injector::optional_resolve_async` is `try_resolve_async().ok()`: it reports
//! "nothing bound it" and "it is bound but the resolve failed" as the same
//! `None`. Where the `None` branch installs a permissive default — an inert
//! guard, an unset authority, a read that races the write — that collapse turns
//! a wiring fault into a silently disabled guard.
//!
//! [`resolve_if_bound`] keeps the two facts apart: only `ServiceNotProvided`
//! means the container deliberately binds nothing; every other error is a
//! wiring fault and stops the boot, naming the type, the composition site and
//! the cause.

use fluxdi::ErrorKind;
use fluxdi::Injector;
use fluxdi::Shared;

/// `None` only when nothing is bound. Panics on any other resolve error.
///
/// `site` names the composition site in the panic, because the type alone does
/// not say which permissive default would have been installed instead.
pub async fn resolve_if_bound<T>(injector: &Injector, site: &str) -> Option<Shared<T>>
where
    T: ?Sized + Send + Sync + 'static,
{
    match injector.try_resolve_async::<T>().await {
        Ok(service) => Some(service),
        Err(e) if e.kind == ErrorKind::ServiceNotProvided => None,
        Err(e) => panic!(
            "{site}: resolving {} failed: {e}. Only \"not bound\" is a legal absence here; \
             taking a failed resolve for an absent service disables the guard silently.",
            std::any::type_name::<T>()
        ),
    }
}
