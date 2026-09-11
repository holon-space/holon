//! The presence axis has an ORDER, and it is the bundle's.
//!
//! The settings list renders providers in the order the store reports them, so
//! that order is user-visible. The bundle declares one deliberately; a store
//! that reported its hash-map keys, sorted or otherwise, would be substituting
//! a different answer that merely happens to look similar today (the bundle is
//! currently declared alphabetically, which is exactly why a sort can pass for
//! the real thing and then diverge the moment a name is added out of order).
//!
//! Keeping the order pinned here is also what makes the rule for introduced
//! connections statable: they APPEND after the bundle, in file-name order, so
//! adding one never reshuffles the rows above it.

use holon_mcp_client::IntegrationConfigStore;

#[test]
fn providers_are_reported_in_bundle_order() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = IntegrationConfigStore::load(dir.path()).expect("store loads over a clean dir");

    let reported: Vec<String> = store
        .providers()
        .iter()
        .map(|p| p.as_str().to_string())
        .collect();
    let bundled: Vec<String> = holon_mcp_client::BUNDLED_SIDECARS
        .iter()
        .map(|s| s.provider.to_string())
        .collect();

    assert_eq!(
        reported, bundled,
        "the store must report the presence axis in BUNDLE order, not in any order derived from \
         its own map"
    );
}

/// The guard that gives the test above teeth: if the bundle is ever declared
/// out of alphabetical order, a sorted implementation stops passing. While the
/// bundle happens to be alphabetical, this test states that the equality above
/// is not evidence of much on its own.
#[test]
fn the_bundle_order_and_sorted_order_currently_coincide() {
    let bundled: Vec<&str> = holon_mcp_client::BUNDLED_SIDECARS
        .iter()
        .map(|s| s.provider)
        .collect();
    let mut sorted = bundled.clone();
    sorted.sort_unstable();
    assert_eq!(
        bundled, sorted,
        "The bundle is no longer declared alphabetically. That is fine — but it means \
         `providers_are_reported_in_bundle_order` is now the ONLY thing keeping the settings \
         rows in the declared order, so do not weaken it."
    );
}
