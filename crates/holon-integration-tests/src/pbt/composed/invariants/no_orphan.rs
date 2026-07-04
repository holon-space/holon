//! `inv-no-orphan-blocks` wired into the composed catalog — a pure `SutBackend`
//! matview self-check (no ref). Its former CDC-lag staleness gate (which read
//! `RefBackend`) was proven dead under the deterministic settle and removed, so
//! `Needs` no longer requires a ref cap — it selects wherever a `SutBackend` is
//! wired, like the sibling `no_parent_cycles` self-check.

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::composition::{BridgedInvariant, CapId, CapInvariant, Needs};

use crate::pbt::invariants::bodies::no_orphan_blocks::InvNoOrphanBlocks;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvNoOrphanBlocks,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutBackend>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use crate::pbt::composed::fixtures::*;
    use crate::pbt::composed::subsystem_seed::{run_with_seeded_ref, seed_ref};

    /// Catch (doc §6 gate): a block whose parent is absent — the SUT's parent
    /// pointer is dangling, isolating the failure to no-orphan (a pure SUT
    /// matview self-check; the reference is present but unread).
    #[tokio::test]
    async fn memory_slice_catches_orphan_block() {
        let orphan = uri("local://o");
        let missing_parent = uri("local://m");
        let sut = fixture_slice(vec![Block::new_text(
            orphan.clone(),
            missing_parent,
            "orphan",
        )]);
        let ref_state = seed_ref(vec![Block::new_text(
            orphan,
            EntityUri::no_parent(),
            "orphan",
        )]);

        let report = run_with_seeded_ref(
            &composed_invariant_catalog(),
            &sut,
            crate::pbt::reference_state::Resolved::identity(ref_state),
        )
        .await;

        let failures = report.failures();
        assert!(
            failures.iter().any(|(id, _)| *id == "inv-no-orphan-blocks"),
            "the dangling parent must be caught by inv-no-orphan-blocks; failures={failures:?}",
        );
    }
}
