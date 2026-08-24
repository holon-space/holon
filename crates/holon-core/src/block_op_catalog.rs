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

/// The "no pages under non-pages" nesting predicate (interim ruling
/// 2026-07-13, Fork B B1): a page block may only nest under another page.
/// Returns `true` when placing a page CHILD under this parent is PROHIBITED.
///
/// `parent_is_page = None` means the child has no parent (a seed/root page at
/// `no_parent`) — always legal, so the seed pages the vault ships stay valid.
///
/// This is the ONE shared predicate for every reparenting/re-tagging
/// chokepoint: `BlockOperations::move_block` (and the
/// indent/outdent/move_up/move_down ops that route through it) plus the
/// `add_tag("Page")` guard in both write authorities. Sharing it keeps the SQL
/// and Loro providers rejecting the prohibited topology identically.
pub fn page_under_non_page_prohibited(child_is_page: bool, parent_is_page: Option<bool>) -> bool {
    child_is_page && parent_is_page == Some(false)
}

/// Shared rejection message for `add_tag("Page")` when the block's immediate
/// parent is a non-page block. Both write authorities emit this exact text so
/// the behaviour is testably identical (increment 4 parity).
pub fn add_page_tag_rejection(block_id: &str, parent_id: &str) -> String {
    format!(
        "add_tag: refusing to mark block '{block_id}' as a Page under non-page parent \
         '{parent_id}' — pages under non-pages are prohibited (interim ruling 2026-07-13); a page \
         may only nest under another page"
    )
}

/// Shared rejection message for `remove_tag("Page")` when the block still has a
/// direct child that is itself a page — unmarking it would leave those children
/// as pages under a non-page block. Both write authorities emit this exact
/// text.
pub fn remove_page_tag_rejection(block_id: &str) -> String {
    format!(
        "remove_tag: refusing to remove the Page tag from block '{block_id}' — it has a direct \
         child page that would become a page under a non-page block (pages under non-pages are \
         prohibited, interim ruling 2026-07-13)"
    )
}

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
        affected_fields: vec![],
        param_mappings: vec![],
        // Dismiss-advice is fired from the advice UI affordance, not the slash
        // command menu.
        target_scope: holon_api::TargetScope::Block,
        menu_exposure: holon_api::MenuExposure::NotListed {
            surface: holon_api::NonMenuSurface::Internal,
        },
        // Mutates the anchor block's `advice_suppressed` set within its
        // container — never crosses an audience boundary.
        boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
        trigger: None,
        bound_params: std::collections::HashMap::new(),
        marking_delta: holon_api::marking::MarkingDelta::Undeclared,
        guard: holon_api::pattern::OpGuard::None,
        arcs: holon_api::arcs::TransitionArcs::Undeclared,
    }
}

/// Canonical descriptor for the bespoke `add_tag` block op: append one `tag`
/// to a block's `tags` set.
///
/// Element-wise, idempotent (re-adding a present tag is an inert no-op), and
/// invertible (its inverse is `remove_tag` of the same `{id, tag}`). This is
/// the element-wise complement to the whole-set `set_field("tags", …)`
/// machinery — the latter stays the vocabulary create/ingest/sync/undo-inverse
/// use to write a WHOLE tag set; element-wise mutation of one tag MUST go
/// through this op, never through a set_field read-modify-write.
///
/// Advertised by the SqlOnly authority only when it owns the `tags` edge field
/// (the `block` entity), and by the Loro authority unconditionally — asserted
/// byte-identical by the `block_op_catalog_parity` test.
pub fn add_tag_descriptor(
    entity_name: &EntityName,
    entity_short_name: &str,
) -> OperationDescriptor {
    tag_descriptor(
        entity_name,
        entity_short_name,
        "add_tag",
        "Add tag",
        "Add a tag to this block (element-wise, idempotent)",
    )
}

/// Canonical descriptor for the bespoke `remove_tag` block op: remove one `tag`
/// from a block's `tags` set. Element-wise, idempotent (removing an absent tag
/// is an inert no-op), and invertible (inverse is `add_tag` of the same
/// `{id, tag}`). Companion of [`add_tag_descriptor`].
pub fn remove_tag_descriptor(
    entity_name: &EntityName,
    entity_short_name: &str,
) -> OperationDescriptor {
    tag_descriptor(
        entity_name,
        entity_short_name,
        "remove_tag",
        "Remove tag",
        "Remove a tag from this block (element-wise, idempotent)",
    )
}

/// Shared shape for the two element-wise tag ops: `{id: EntityId, tag:
/// String}`.
fn tag_descriptor(
    entity_name: &EntityName,
    entity_short_name: &str,
    name: &str,
    display_name: &str,
    description: &str,
) -> OperationDescriptor {
    OperationDescriptor {
        entity_name: entity_name.clone(),
        entity_short_name: entity_short_name.to_string(),
        id_column: "id".to_string(),
        name: name.to_string(),
        display_name: display_name.to_string(),
        description: description.to_string(),
        required_params: vec![
            OperationParam {
                name: "id".to_string(),
                type_hint: TypeHint::EntityId {
                    entity_name: entity_name.clone(),
                },
                description: "The block to tag".to_string(),
            },
            OperationParam {
                name: "tag".to_string(),
                type_hint: TypeHint::String,
                description: "The tag to add or remove".to_string(),
            },
        ],
        affected_fields: vec![],
        param_mappings: vec![],
        // Element-wise tag mutation is a programmatic/structural op, not a bare
        // slash command.
        target_scope: holon_api::TargetScope::Block,
        menu_exposure: holon_api::MenuExposure::NotListed {
            surface: holon_api::NonMenuSurface::Internal,
        },
        // Element-wise tag edit within the block's container — never crosses an
        // audience boundary. (add_tag("Page") topology is guarded separately by
        // `page_under_non_page_prohibited`, not a container crossing.)
        boundary_behavior: holon_api::BoundaryBehavior::PrivateOnly,
        trigger: None,
        bound_params: std::collections::HashMap::new(),
        marking_delta: holon_api::marking::MarkingDelta::Undeclared,
        guard: holon_api::pattern::OpGuard::None,
        arcs: holon_api::arcs::TransitionArcs::Undeclared,
    }
}
