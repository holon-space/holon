//! Integration-test-local PBT SUT capability traits (the cap home-rule, third
//! category: *test-only-typed*).
//!
//! A cap lives in the crate that owns the types it names. `CycleTarget` and
//! `MutationEvent` are integration-test-only types that `holon-pbt-core` cannot
//! name, so the caps that mention them live here rather than being forced down
//! into `holon-pbt-core`.
//!
//! `#[holon_macros::capmap_adapter]` references `::holon_pbt_core::composition::*`
//! by absolute path and the trait by bare name, so it expands correctly in this
//! crate too: `impl SutMutate for CapMap` (local trait for foreign type — allowed)
//! and `impl CapName for dyn SutMutate` (foreign trait for local type — allowed).

use crate::pbt::transitions::toggle_state::CycleTarget;
use crate::pbt::types::MutationEvent;

/// SUT capability: task-state cycling (`ToggleState`). A genuinely composable
/// `&self` mutation — `HeadlessFrontendComponent` realizes it HEADLESSLY via the
/// production `set_field task_state` op, so any composed config that has it can drive
/// `ToggleState` faithfully.
#[holon_macros::capmap_adapter]
pub trait SutMutate {
    async fn toggle_state(&self, block_id: &holon_api::EntityUri, new_state: CycleTarget);
}

/// SUT capability: the SEAM-relocated mutations — generic UI/external mutations
/// (`ApplyMutation`) and bulk external block adds (`BulkExternalAdd`). Split off
/// `SutMutate` (2026-06-21, swap track) because their real, `ref_state`-dependent
/// dispatch lives ENTIRELY in the `E2ESut` harness seam (`block_tree_post_action`):
/// the cap action itself is a pure `&self` no-op, and a no-op cap is a FAKE on any
/// SUT that lacks the seam (the composed `HeadlessFrontendComponent` would silently
/// pass `ApplyMutation`/`BulkExternalAdd` while doing nothing). So `E2ESut` provides
/// this cap (its seam does the work) but the composed frontend does NOT — keeping
/// cap-presence honest, so these transitions AUTO-NARROW out of the composed
/// alphabet (`required_caps` ∌ a provided cap) instead of generating + diverging. The
/// seam rebuild (a real composed dispatch) is what will let the composed path provide
/// it; until then its absence is the truthful signal.
#[holon_macros::capmap_adapter]
pub trait SutSeamMutate {
    async fn apply_mutation(&self, event: MutationEvent);
    async fn bulk_external_add(
        &self,
        doc_uri: &holon_api::EntityUri,
        blocks: &[holon_api::block::Block],
    );
}

/// SUT capability: pre-startup org-filesystem fixture setup — writing org files,
/// creating directories, `git`/`jj` init, and planting a stale/corrupt Loro
/// snapshot. Driven by the `WriteOrgFile`, `CreateDirectory`, `GitInit`,
/// `JjGitInit`, `CreateStaleLoro` transitions. Lives here (cap home-rule, local
/// category) because `create_stale_loro` names the test-only
/// [`crate::LoroCorruptionType`]; the others are grouped with it as cohesive
/// fixture ops. `E2ESut`-only — no headless `frontend_slice` filesystem.
/// SUT capability: app lifecycle for the wide PBT — boot, restart, document
/// creation, and the concurrent-schema-init regression probe. Distinct from the
/// coarser `holon_pbt_core::capabilities::SutLifecycle` (a `&mut self` cap used
/// only by the decomposition spike): this is the `&self`, `ref_state`-free
/// surface the wide-PBT `StartApp`/`SimulateRestart`/`CreateDocument`/
/// `ConcurrentSchemaInit` transitions bind. `ref_state`-derived values are
/// either precomputed at the transition boundary and passed as typed args
/// (`start_app`'s `root_id`/`expects_valid_index`) or relocated to the
/// `block_tree_post_action` seam (settles + doc-uri reconciliation).
#[holon_macros::capmap_adapter]
pub trait SutAppLifecycle {
    /// Boot the app. `root_id` (the layout render root) and `expects_valid_index`
    /// are `ref_state`-derived but consumed DURING the action (render warmup /
    /// `ensure_reactive_engine`), so the `StartApp` transition precomputes them at
    /// the boundary and passes them as typed args — the HARD RULE forbids
    /// `ref_state` in the signature, not derived values. The doc-uri reconcile and
    /// the seed-count settle relocate to the `block_tree_post_action` `StartApp`
    /// arm.
    #[allow(clippy::too_many_arguments)]
    async fn start_app(
        &self,
        root_id: holon_api::EntityUri,
        expects_valid_index: bool,
        wait_for_ready: bool,
        enable_fake_mcp: bool,
        enable_loro: bool,
    );
    async fn simulate_restart(&self);
    async fn create_document(&self, file_name: &str);
    async fn concurrent_schema_init(&self);
}

#[holon_macros::capmap_adapter]
pub trait SutFixtureFs {
    async fn write_org_file(&self, filename: &str, content: &str);
    async fn create_directory(&self, path: &str);
    async fn git_init(&self);
    async fn jj_git_init(&self);
    async fn create_stale_loro(
        &self,
        org_filename: &str,
        corruption_type: crate::LoroCorruptionType,
    );
}
