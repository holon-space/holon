//! Canonical block op-catalog — the single home for block-operation metadata
//! that both write authorities must agree on.
//!
//! Motivation (Write-Path Unification,
//! `docs/Plans/WritePathUnification-Options-2026-07-17.md`, Option A increment
//! 0 / Option D). Every block op is served by two independent
//! `OperationProvider`s — `SqlOperationProvider` (SqlOnly authority) and
//! `LoroBlockOperations` (Loro authority) — and each hand-built its own bespoke
//! descriptors. Drift between those hand-built descriptors is bug class **I1**;
//! its first casualty was BugFunnel row 26 (`dismiss_advice` undispatchable in
//! the SqlOnly prod session because only the Loro provider advertised it).
//!
//! The macro-generated CRUD/task/block/mark/text descriptors are already
//! single-sourced from the `#[operations_trait]` macro, so they are not the
//! drift surface. The drift surface is the *bespoke* descriptors that are
//! hand-written in both providers. Today that is exactly `dismiss_advice`.
//! Both providers now source it from here; the parity test
//! (`crates/holon/tests/block_op_catalog_parity.rs`) is the certificate that
//! no future edit re-forks it.

use holon_api::EntityName;
use holon_api::OperationDescriptor;
use holon_api::OperationParam;
use holon_api::TypeHint;

/// Canonical descriptor for the bespoke `dismiss_advice` block op
/// (ADR 0021/0022): append one `lesson_id` to an anchor block's
/// `advice_suppressed` set.
///
/// This is the ONE home for the descriptor. Both the SqlOnly authority
/// (`SqlOperationProvider`, only when it owns the `advice_suppressed` edge
/// field) and the Loro authority (`LoroBlockOperations`) advertise the value
/// this function returns — see the `block_op_catalog_parity` test.
pub fn dismiss_advice_descriptor(
    entity_name: &EntityName,
    entity_short_name: &str,
) -> OperationDescriptor {
    OperationDescriptor {
        entity_name: entity_name.clone(),
        entity_short_name: entity_short_name.to_string(),
        id_column: "id".to_string(),
        name: "dismiss_advice".to_string(),
        display_name: "Dismiss advice".to_string(),
        description: "Suppress this advice lesson under its anchor block".to_string(),
        required_params: vec![
            OperationParam {
                name: "anchor_id".to_string(),
                type_hint: TypeHint::EntityId {
                    entity_name: entity_name.clone(),
                },
                description: "The anchor block the advice is woven under".to_string(),
            },
            OperationParam {
                name: "lesson_id".to_string(),
                type_hint: TypeHint::EntityId {
                    entity_name: entity_name.clone(),
                },
                description: "The advice lesson block to dismiss".to_string(),
            },
        ],
        ..Default::default()
    }
}
