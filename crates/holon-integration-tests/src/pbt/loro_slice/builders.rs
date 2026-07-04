//! Compose the Loro slice's SUT `CapMap` from its component — "a slice is a
//! component list" (§1). A second storage backing the *same* shared catalog.

use holon_loro::LoroBackend;
use holon_loro_testing::LoroBackendComponent;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::Config;

/// Build the Loro-storage composed SUT — a real `LoroBackend` CRDT exposed as
/// `SutBackend` + `SutLoroLog`. The same `composed_invariant_catalog()` the
/// memory slice runs selects against these caps; every block-tree invariant now
/// validates the Loro realization too.
pub fn loro_wide(backend: LoroBackend) -> CapMap {
    Config::new()
        .with(LoroBackendComponent::new(backend))
        .build()
}
