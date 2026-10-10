//! Guard composition (SEC-bound.a): every container binds every guard.
//!
//! A guard whose binding is merely OPTIONAL has two failure modes that look
//! alike from the consumer: nothing bound it, and something bound it where the
//! consumer does not look (a child scope, a name). Both read as "not bound",
//! and a consumer that answers "not bound" with a permissive default runs open.
//!
//! So the binding stops being optional. Every container commits to one binding
//! per guard — the real guard, or an EXPLICIT no-op, the way
//! [`holon_core::NoReadOnlyDocuments`] is the explicit no-op write-tier
//! authority. The dispatcher then resolves each guard as REQUIRED
//! ([`holon_core::resolve_bound_async`]), and a missing binding stops the boot
//! instead of disabling the guard.
//!
//! Two kinds of container reach this module:
//!
//! - A composition root that owns its guard policy says so with
//!   [`RootOwnsGuardComposition`]. Nothing is filled in for it; a guard it
//!   forgot is a wiring fault and the boot stops naming the type. The app is
//!   this kind.
//! - Every other container — tests, SqlOnly, headless harnesses — gets the
//!   explicit no-op guards bound here, which is the "no-op test double" half of
//!   the ruling. They are bound as bindings, not as a `None` branch inside the
//!   consumer, so what a container runs with is readable from its wiring.

use std::sync::Arc;

use fluxdi::ErrorKind;
use fluxdi::Injector;
use fluxdi::Provider;

/// A composition root's declaration that it binds every guard itself.
///
/// Its presence turns [`complete_guard_composition`] from "bind the no-ops"
/// into "assert, and stop the boot if one is missing".
pub struct RootOwnsGuardComposition;

/// Settle the guard bindings of `injector`, after every module and the
/// composition root have run.
///
/// Panics when a root that owns its guard composition left one unbound.
pub fn complete_guard_composition(injector: &Injector) {
    let root_owns_them = already_bound::<RootOwnsGuardComposition>(injector);

    claim::<dyn holon_core::BoundaryEnforcer>(
        injector,
        root_owns_them,
        "the boundary/authz seam",
        || {
            Provider::root(|_| {
                Arc::new(holon_core::InertBoundaryEnforcer) as Arc<dyn holon_core::BoundaryEnforcer>
            })
        },
    );
    claim::<dyn crate::api::net_guard::NetGuard>(injector, root_owns_them, "the net gate", || {
        Provider::root(|_| {
            Arc::new(crate::api::net_guard::InertNetGuard)
                as Arc<dyn crate::api::net_guard::NetGuard>
        })
    });
    claim::<dyn holon_core::WriteTierAuthority>(
        injector,
        root_owns_them,
        "the write-tier gate",
        || {
            Provider::root(|_| {
                Arc::new(holon_core::NoReadOnlyDocuments) as Arc<dyn holon_core::WriteTierAuthority>
            })
        },
    );
    // Not a no-op: the shape gate reads the pre-write state from SOME
    // authority, and with no CRDT store the SQL tables are it. Binding it here
    // makes that choice visible in the container instead of hiding it in the
    // consumer's `None` branch.
    claim::<dyn holon_core::WriteAuthorityReads>(
        injector,
        root_owns_them,
        "the block write authority the shape gate reads",
        || {
            Provider::root(|resolver| {
                let db = resolver.resolve::<dyn super::DbHandleProvider>();
                Arc::new(crate::core::sql_write_authority::SqlWriteAuthority::new(
                    db.handle(),
                )) as Arc<dyn holon_core::WriteAuthorityReads>
            })
        },
    );
}

/// Bind `default` for `T` unless the container already did.
///
/// With `assert_only`, a container that did not bind `T` is a wiring fault: the
/// root said it owns guard composition and then left this guard out.
fn claim<T>(
    injector: &Injector,
    assert_only: bool,
    role: &str,
    default: impl FnOnce() -> Provider<T>,
) where
    T: ?Sized + Send + Sync + 'static,
{
    if already_bound::<T>(injector) {
        return;
    }
    assert!(
        !assert_only,
        "guard composition: nothing binds {role} ({}) in this container. A composition root \
         that owns its guard composition binds every guard — the real one, or the explicit \
         no-op. Leaving it out would run the gate open.",
        std::any::type_name::<T>()
    );
    // `already_bound` left its probe registered, so this replaces the probe.
    injector.override_provider::<T>(default());
}

/// Whether `T` has a binding in the scope a consumer resolves from.
///
/// Asked by ATTEMPTING the registration, not by resolving: a resolve would run
/// the factory, and ordering a guard's factory before the store is ready is a
/// different failure. `try_provide` answers from the same root-scope table the
/// consumer's resolve reads, so a guard bound in a child scope or under a name
/// reads as absent here exactly as it does there.
fn already_bound<T>(injector: &Injector) -> bool
where
    T: ?Sized + Send + Sync + 'static,
{
    // Never resolved: the probe either loses the race to the existing binding,
    // or its `Ok` is immediately replaced by the caller's real default (or
    // ends the boot).
    match injector.try_provide::<T>(Provider::root_try_async(|_| async {
        Err::<fluxdi::Shared<T>, String>(
            "guard-composition probe: this provider is never resolved".to_string(),
        )
    })) {
        Err(e) if e.kind == ErrorKind::ProviderAlreadyRegistered => true,
        Ok(()) => false,
        Err(e) => panic!(
            "guard composition: probing whether {} is bound failed: {e}",
            std::any::type_name::<T>()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A root that declares it owns guard composition and forgets one guard
    /// stops the boot, naming the guard — it does NOT get a silent no-op.
    #[test]
    #[should_panic(expected = "nothing binds the net gate")]
    fn a_root_that_owns_guard_composition_must_bind_every_guard() {
        let injector = Injector::root();
        injector.provide::<RootOwnsGuardComposition>(Provider::root(|_| {
            fluxdi::Shared::new(RootOwnsGuardComposition)
        }));
        injector.provide::<dyn holon_core::BoundaryEnforcer>(Provider::root(|_| {
            Arc::new(holon_core::InertBoundaryEnforcer) as Arc<dyn holon_core::BoundaryEnforcer>
        }));
        // `dyn NetGuard` is deliberately left out.
        complete_guard_composition(&injector);
    }

    /// A container that does NOT own its guard composition gets the explicit
    /// no-op guards, so the consumer can require all of them.
    #[test]
    fn a_plain_container_gets_the_explicit_no_op_guards() {
        let injector = Injector::root();
        complete_guard_composition(&injector);
        assert!(already_bound::<dyn holon_core::BoundaryEnforcer>(&injector));
        assert!(already_bound::<dyn crate::api::net_guard::NetGuard>(
            &injector
        ));
        assert!(already_bound::<dyn holon_core::WriteTierAuthority>(
            &injector
        ));
        assert!(already_bound::<dyn holon_core::WriteAuthorityReads>(
            &injector
        ));
    }

    /// A guard bound where the consumer does not look is MISSING, not bound.
    ///
    /// A module registered through `imports()` gets its own child injector, and
    /// a `Provider::singleton` there lands in that child's table — invisible to
    /// the dispatcher, which resolves from the root scope. The claim therefore
    /// has to fail. This pins `try_provide`: were it to answer from the local
    /// scope (or walk scopes), this guard would read as bound and the gate
    /// would run open with nothing in the root scope to enforce it.
    #[test]
    #[should_panic(expected = "nothing binds the net gate")]
    fn a_guard_bound_only_in_a_child_scope_is_still_missing() {
        let root = Injector::root();
        root.provide::<RootOwnsGuardComposition>(Provider::root(|_| {
            fluxdi::Shared::new(RootOwnsGuardComposition)
        }));
        root.provide::<dyn holon_core::BoundaryEnforcer>(Provider::root(|_| {
            Arc::new(holon_core::InertBoundaryEnforcer) as Arc<dyn holon_core::BoundaryEnforcer>
        }));
        let child = Injector::child(fluxdi::Shared::new(root));
        child.provide::<dyn crate::api::net_guard::NetGuard>(Provider::singleton(|_| {
            Arc::new(crate::api::net_guard::InertNetGuard)
                as Arc<dyn crate::api::net_guard::NetGuard>
        }));
        complete_guard_composition(&child);
    }

    /// The same for a NAME: a named binding answers `resolve_named`, never the
    /// unnamed resolve the dispatcher does, so it does not satisfy the claim.
    /// This pins `try_provide` against treating a named binding as the unnamed
    /// one.
    #[test]
    #[should_panic(expected = "nothing binds the net gate")]
    fn a_guard_bound_only_under_a_name_is_still_missing() {
        let injector = Injector::root();
        injector.provide::<RootOwnsGuardComposition>(Provider::root(|_| {
            fluxdi::Shared::new(RootOwnsGuardComposition)
        }));
        injector.provide::<dyn holon_core::BoundaryEnforcer>(Provider::root(|_| {
            Arc::new(holon_core::InertBoundaryEnforcer) as Arc<dyn holon_core::BoundaryEnforcer>
        }));
        injector.provide_named::<dyn crate::api::net_guard::NetGuard>(
            "the net gate under a name",
            Provider::root(|_| {
                Arc::new(crate::api::net_guard::InertNetGuard)
                    as Arc<dyn crate::api::net_guard::NetGuard>
            }),
        );
        complete_guard_composition(&injector);
    }

    /// The converse, so the two tests above cannot pass by calling EVERY guard
    /// missing: the one shape a consumer resolves — unnamed, root scope — reads
    /// as present.
    #[test]
    fn a_root_scope_unnamed_binding_reads_as_present() {
        let injector = Injector::root();
        injector.provide::<dyn crate::api::net_guard::NetGuard>(Provider::root(|_| {
            Arc::new(crate::api::net_guard::InertNetGuard)
                as Arc<dyn crate::api::net_guard::NetGuard>
        }));
        assert!(already_bound::<dyn crate::api::net_guard::NetGuard>(
            &injector
        ));
    }

    /// A real binding is never replaced by the no-op.
    #[tokio::test]
    async fn an_existing_binding_wins_over_the_no_op() {
        let injector = Injector::root();
        injector.provide::<dyn holon_core::WriteTierAuthority>(Provider::root(|_| {
            Arc::new(holon_core::NoReadOnlyDocuments) as Arc<dyn holon_core::WriteTierAuthority>
        }));
        complete_guard_composition(&injector);
        // The probe must not have left a failing provider behind.
        let _ = injector
            .try_resolve_async::<dyn holon_core::WriteTierAuthority>()
            .await
            .expect("the binding the container made still resolves");
    }
}
