#![cfg(feature = "pbt")]
//! **The cap-set table is the real `compose_sut`'s cap set — exactly.**
//!
//! The keystone reference reads
//! [`cap_ids_for_wiring`](holon_integration_tests::pbt::composed::wide_e2e::cap_ids_for_wiring)
//! — a declarative wiring → cap-set table — to narrow the generation alphabet,
//! instead of booting a throwaway SUT per drawn wiring. That table is only as
//! honest as this test: it boots the PRODUCTION `compose_sut` for every wiring
//! class the keystone can draw and asserts set EQUALITY with the table.
//!
//! Why equality and not containment: a cap the table claims but `compose_sut`
//! never registers makes the generator draw a transition the SUT cannot drive
//! (a "selected but absent from the CapMap" panic), and a cap `compose_sut`
//! registers but the table omits silently narrows the alphabet — the vacuity
//! the keystone has no other detector for. Dropping ONE cap from one
//! provider's registration must fail here.
//!
//! @pbt kind harness
//! @pbt covers composed-cap-set-table — the declarative cap-set table vs the
//! real `compose_sut`

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::Mutex;

use holon_integration_tests::pbt::composed::builder::compose_sut;
use holon_integration_tests::pbt::composed::wide_e2e::cap_ids_for_wiring;
use holon_integration_tests::pbt::composed::wide_e2e::set_for_wiring;
use holon_pbt_core::Wiring;

/// Boot the real `compose_sut` for `wiring` and assert the table declares
/// exactly the caps it registered.
async fn assert_table_is_exact(wiring: Wiring) {
    holon_integration_tests::pbt::set_loro_peer_id_if_unset("1");
    let set = set_for_wiring(&wiring);
    let declared = cap_ids_for_wiring(&wiring);
    let names: BTreeSet<&'static str> = declared.iter().map(|c| c.name()).collect();
    assert_eq!(
        names.len(),
        declared.len(),
        "the cap-set table lists a cap twice for storage={:?} projections={:?}",
        set.wiring.storage_adapters,
        set.projections,
    );

    let resolver = Arc::new(Mutex::new(BTreeMap::new()));
    let registered = compose_sut(&set, &resolver).await.caps.cap_names();

    let missing: Vec<_> = registered.difference(&names).collect();
    let extra: Vec<_> = names.difference(&registered).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "cap-set table diverges from compose_sut for storage={:?} projections={:?}\n  \
         registered but NOT in the table: {missing:?}\n  in the table but NOT registered: \
         {extra:?}",
        set.wiring.storage_adapters,
        set.projections,
    );
}

/// Turso frontend, no Loro (`sql_only`): the frontend arm alone.
#[tokio::test(flavor = "multi_thread")]
async fn table_is_exact_for_sql_only() {
    assert_table_is_exact(Wiring::sql_only()).await;
}

/// The widest wiring: frontend arm + Loro arm + the editor legs.
#[tokio::test(flavor = "multi_thread")]
async fn table_is_exact_for_full() {
    assert_table_is_exact(Wiring::full()).await;
}

/// Loro-only storage: the Loro arm + the in-memory editor.
#[tokio::test(flavor = "multi_thread")]
async fn table_is_exact_for_loro_backend() {
    assert_table_is_exact(Wiring::loro_backend()).await;
}

/// Org-only storage normalizes to the Loro class (`set_for_wiring` inserts Loro
/// whenever Turso is absent) — the table must be keyed on the NORMALIZED set.
#[tokio::test(flavor = "multi_thread")]
async fn table_is_exact_for_org_create_ordering() {
    assert_table_is_exact(Wiring::org_create_ordering()).await;
}

/// The degenerate draw (no storage at all) normalizes to the same Loro class.
#[tokio::test(flavor = "multi_thread")]
async fn table_is_exact_for_empty_storage() {
    assert_table_is_exact(Wiring::custom([], [], [])).await;
}
