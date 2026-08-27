//! `inv-net-totality` — every operation the run actually fired has a
//! transition in the derived net (ADR 0032 §2, ruling D32.a's C1 half).
//!
//! @pbt oracle internal-consistency — the set of `(entity, op)` pairs the
//!   production dispatcher was asked to execute is a SUBSET of the derived
//!   net's transition keys
//! @pbt covers net-totality — an operation the system can fire that the
//!   derived net does not describe, so every net-based analysis (conflicts,
//!   cycles, the marking oracle) silently reports on a partial world
//! @pbt slips-if-removed a provider registered outside the net's source union
//!   fires unmodelled; `conflicts()` reports no contention for the places it
//!   writes, and the fail-closed `Unanalyzable` posture becomes decorative
//!
//! ABSENCE is the failure. A transition that says `Unanalyzable` passes: the
//! net has declared "cannot say", which every analysis must surface. A missing
//! transition says nothing at all, which no analysis can surface.

use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

use crate::pbt::net_cap::SutDerivedNet;

/// The wildcard fan-out entity. `*::sync` / `*::full_sync` re-dispatch to every
/// syncable provider rather than writing anything themselves, and `*` is not a
/// relation, so they lower to no places and are excluded from the net by
/// construction (`BackendEngine::derived_net`). Excluded here for the same
/// reason, not as a tolerance: the fan-out's per-provider dispatches open their
/// own spans and ARE checked.
const WILDCARD_ENTITY: &str = "*";

/// A dispatched `(entity, op)` the net excludes by construction rather than by
/// omission: the two sync fan-out layers, `*` and the `<provider>.sync` marker
/// the wildcard re-dispatches onto. Both name a provider set, not a relation.
/// The entity ops a sync ultimately writes through open their own spans and are
/// checked like any other.
///
/// A span carries a name, not a descriptor, so this cannot re-run the
/// STRUCTURAL test that earns the exclusion (zero places). It does not have to:
/// `holon_core::classify_for_net` refuses at the catalog boundary any
/// descriptor wearing this name shape while writing places, so by the time an
/// op with this name can fire, the boundary has already proven it writes
/// nothing.
fn is_fan_out(entity: &str) -> bool {
    entity == WILDCARD_ENTITY || holon_core::has_sync_fan_out_name(entity)
}

pub struct InvNetTotality;

impl InvNetTotality {
    pub const ID: InvariantId = InvariantId("inv-net-totality");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvNetTotality
where
    S: SutDerivedNet,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let net = sut.derived_net().await;
        let mut missing = Vec::new();
        for (entity, op) in sut.fired_operations().await {
            if is_fan_out(&entity) {
                continue;
            }
            let key = match holon_net::TransitionKey::operation(&entity, &op) {
                Ok(key) => key,
                // A fired non-fan-out pair whose entity the key grammar cannot
                // encode is a hole of its own kind: `classify_for_net` should
                // have refused the descriptor at the catalog, so the net cannot
                // be describing it either.
                Err(err) => {
                    return InvariantResult::Fail(format!(
                        "the run fired `{entity}`.`{op}`, whose entity no transition key can \
                         encode: {err}. `holon_core::classify_for_net` refuses this shape at the \
                         catalog boundary, so a pair that both fires and cannot be keyed is an \
                         operation outside every net analysis",
                    ));
                }
            };
            if net.transition(&key).is_none() {
                missing.push(key);
            }
        }
        if missing.is_empty() {
            return InvariantResult::Ok;
        }
        let named: Vec<&str> = missing.iter().map(|k| k.as_str()).collect();
        let described: std::collections::BTreeSet<String> = net
            .transitions
            .iter()
            .map(|t| t.key().as_str().to_string())
            .collect();
        InvariantResult::Fail(format!(
            "the derived net does not describe {} operation(s) this run fired: {named:?} — the \
             net has {} transition(s), so every analysis over it reports on a partial world. An \
             operation the system can fire must appear as a transition, `Unanalyzable` if its \
             declarations are incomplete, never absent. The net describes: {described:?}",
            missing.len(),
            net.transitions.len(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use holon_api::EntityName;
    use holon_net::Analyzability;
    use holon_net::CompiledNet;
    use holon_net::NetTransition;
    use holon_net::TransitionSource;
    use holon_net::UndeclaredHalf;

    use super::*;

    /// A SUT that answers both halves from hand-built data, so the body can be
    /// exercised without a slice.
    struct FakeNetSut {
        net: CompiledNet,
        fired: BTreeSet<(String, String)>,
    }

    #[async_trait::async_trait(?Send)]
    impl SutDerivedNet for FakeNetSut {
        async fn derived_net(&self) -> CompiledNet {
            self.net.clone()
        }

        async fn fired_operations(&self) -> BTreeSet<(String, String)> {
            self.fired.clone()
        }
    }

    fn transition(entity: &str, op: &str, analyzability: Analyzability) -> NetTransition {
        NetTransition {
            source: TransitionSource::Operation {
                entity: holon_net::NetEntity::parse(entity).expect("dotless entity"),
                op: op.to_string(),
            },
            analyzability,
            arcs: Vec::new(),
            residue: Vec::new(),
        }
    }

    fn fired(pairs: &[(&str, &str)]) -> BTreeSet<(String, String)> {
        pairs
            .iter()
            .map(|(e, o)| (EntityName::new(*e).as_str().to_string(), o.to_string()))
            .collect()
    }

    async fn check(sut: &FakeNetSut) -> InvariantResult {
        Invariant::<(), FakeNetSut>::check(&InvNetTotality, &(), sut).await
    }

    /// A described operation passes, and `Unanalyzable` is described — the
    /// fail-closed slot is a declaration, not an absence.
    #[tokio::test]
    async fn a_described_operation_passes_even_when_unanalyzable() {
        let sut = FakeNetSut {
            net: CompiledNet {
                transitions: vec![
                    transition("block", "set_field", Analyzability::Analyzable),
                    transition(
                        "block",
                        "split_block",
                        Analyzability::Unanalyzable {
                            undeclared: vec![UndeclaredHalf::Arcs],
                        },
                    ),
                ],
            },
            fired: fired(&[("block", "set_field"), ("block", "split_block")]),
        };
        assert!(
            matches!(check(&sut).await, InvariantResult::Ok),
            "an operation the net describes must pass, Unanalyzable included",
        );
    }

    /// Non-vacuity ablation: drop ONE descriptor's registration from the net
    /// and the same fired set must go red, naming it.
    #[tokio::test]
    async fn dropping_one_descriptor_registration_reds() {
        let sut = FakeNetSut {
            net: CompiledNet {
                transitions: vec![transition("block", "set_field", Analyzability::Analyzable)],
            },
            fired: fired(&[("block", "set_field"), ("navigation", "focus")]),
        };
        let InvariantResult::Fail(msg) = check(&sut).await else {
            panic!("a fired operation absent from the net must fail the invariant");
        };
        assert!(
            msg.contains("op:navigation.focus"),
            "the failure must name the missing transition; got {msg}"
        );
    }

    /// The wildcard fan-out names no relation, so it is out of the net's
    /// domain rather than a hole in it.
    #[tokio::test]
    async fn the_wildcard_fan_out_is_not_a_hole() {
        let sut = FakeNetSut {
            net: CompiledNet {
                transitions: Vec::new(),
            },
            fired: fired(&[("*", "sync"), ("*", "full_sync")]),
        };
        assert!(
            matches!(check(&sut).await, InvariantResult::Ok),
            "the wildcard fan-out names no relation, so it is out of the net's domain",
        );
    }

    /// The wildcard's second layer: it re-dispatches onto one
    /// `<provider>.sync` marker per syncable provider
    /// (`holon_core::generate_sync_operation`). Those name a provider too, so
    /// they are out of the domain for the same reason — and their name carries
    /// the `.` a transition key cannot encode, so letting one through would red
    /// on the key grammar rather than report a hole.
    #[tokio::test]
    async fn the_per_provider_sync_fan_out_is_not_a_hole() {
        let sut = FakeNetSut {
            net: CompiledNet {
                transitions: Vec::new(),
            },
            fired: fired(&[("orgmode.sync", "sync"), ("todoist.sync", "sync")]),
        };
        assert!(
            matches!(check(&sut).await, InvariantResult::Ok),
            "a `<provider>.sync` marker names a provider, not a relation",
        );
    }

    /// A dotted entity that is NOT a fan-out marker has no transition key, so
    /// the check reds naming it — it does not panic out of the invariant, and
    /// it does not pass by being unkeyable.
    #[tokio::test]
    async fn a_dotted_non_fan_out_entity_reds_instead_of_panicking() {
        let sut = FakeNetSut {
            net: CompiledNet {
                transitions: Vec::new(),
            },
            fired: fired(&[("orgmode.import", "run")]),
        };
        let InvariantResult::Fail(message) = check(&sut).await else {
            panic!("a fired operation with no encodable key is not a passing world");
        };
        assert!(
            message.contains("orgmode.import") && message.contains("run"),
            "the failure must name the unkeyable pair: {message}"
        );
    }
}
