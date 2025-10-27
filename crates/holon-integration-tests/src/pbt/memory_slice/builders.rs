//! Compose the per-slice SUT `CapMap` from components — the whole "a slice is a
//! component list" surface (§1).

use std::sync::Arc;

use holon::api::MemoryBackend;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapProvider;
use holon_pbt_core::composition::Config;

use super::components::InMemEditorComponent;
use super::components::MemoryBackendComponent;

/// Build the pure-memory composed SUT — block store only.
pub fn memory_wide(backend: MemoryBackend) -> CapMap {
    Config::new()
        .with(MemoryBackendComponent::new(backend))
        .build()
}

/// The structural-write SUT: block store hosting **both** the `SutBackend` read
/// cap and the `SutBlockTreeWrite` write cap, over one shared
/// `Arc<MemoryBackend>`. Returns the component `Arc` so the driver can keep its
/// synthetic-id counter in lockstep with the oracle (`set_next_split_id`) and
/// seed the shared store. The composed `CapMap` *is* the structural
/// `SutTransitionTarget` — Stage 1a needs only `SutBlockTreeWrite`, so no
/// editor/focus/quiesce component is wired.
pub fn memory_wide_structural(
    backend: Arc<MemoryBackend>,
) -> (CapMap, Arc<MemoryBackendComponent>) {
    let component = Arc::new(MemoryBackendComponent::new_shared(backend));
    let mut caps = CapMap::new();
    component.clone().register(&mut caps);
    (caps, component)
}

/// The memory-*wide* SUT (doc §6): block store **plus** in-memory editor. Two
/// components, one `.register` each — the payoff. Returns the editor `Arc` too,
/// since the apply phase drives it through `&self` writes (§4.4) while it is
/// also hosted as the read-only `SutEditorMirrorRead` cap.
pub fn memory_wide_with_editor(backend: MemoryBackend) -> (CapMap, Arc<InMemEditorComponent>) {
    // Share one backend `Arc` between the read/structural cap
    // (`MemoryBackendComponent`) and the editor's commit store, so an editor
    // write is observed by the block-content invariants reading the same store.
    let backend = Arc::new(backend);
    let editor = Arc::new(InMemEditorComponent::new(
        backend.clone() as Arc<dyn holon_api::repository::CoreOperations>
    ));
    let mut caps = CapMap::new();
    Arc::new(MemoryBackendComponent::new_shared(backend)).register(&mut caps);
    editor.clone().register(&mut caps);
    (caps, editor)
}
