//! The SUT cap the ADR 0032 net-totality invariant reads: the derived net and
//! the operations that actually fired.
//!
//! Both halves sit on ONE cap on purpose. The invariant's question — "does the
//! net describe everything the system fired?" — needs the two answers to come
//! from the same SUT, and a single cap makes the body drivable by a hand-built
//! fake, so the ablation gate (drop a descriptor → red) is a unit test rather
//! than a whole-slice run.

use std::collections::BTreeSet;

use holon::api::BackendEngine;
use holon_api::EntityName;
use holon_net::CompiledNet;

/// Read cap: the derived net plus the operations dispatched since the last
/// per-transition span reset.
#[allow(async_fn_in_trait)]
#[holon_macros::capmap_adapter]
pub trait SutDerivedNet {
    /// The net derived from THIS SUT's production sources, recomputed now
    /// (D31.a: nothing is cached, so a post-boot provider registration is
    /// visible to the very next call).
    async fn derived_net(&self) -> CompiledNet;

    /// Every `(entity, op)` the production dispatcher was asked to execute
    /// since the last per-transition reset. The entity is canonicalized
    /// through [`EntityName`] so a raw `gen_1` compares equal to the `gen-1` a
    /// descriptor advertises.
    async fn fired_operations(&self) -> BTreeSet<(String, String)>;
}

/// Read the fired set off the shared span collector. Every dispatch path opens
/// the `dispatcher.execute_operation` span, so this sees ops a component's own
/// write caps never routed — rule firings and follow-up dispatches included.
pub fn fired_operations_from_spans() -> BTreeSet<(String, String)> {
    crate::test_tracing::SpanCollector::global()
        .dispatched_operations()
        .into_iter()
        .map(|(entity, op)| (EntityName::new(&entity).as_str().to_string(), op))
        .collect()
}

/// The net the engine derives from its TOTAL production sources (Increment 2):
/// the dispatcher's registered providers, the engine-synthetic `block`
/// compounds, and the rule watcher's published verdicts.
///
/// Recomputed per call and held nowhere — the cap must observe the same net a
/// production consumer would, including providers registered after boot.
pub fn derived_net_of(engine: &BackendEngine) -> CompiledNet {
    engine
        .derived_net()
        .expect("the production catalog must compile to a net")
}
