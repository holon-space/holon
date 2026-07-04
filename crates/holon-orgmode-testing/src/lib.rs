//! Org-mode test-support helpers, co-located out of `holon-integration-tests`
//! (Phase 1a Step 0) so companion `*-testing` crates (e.g.
//! `holon-loro-testing`) can normalize blocks for cross-store comparison
//! without depending on the integration-tests crate. Phase 3 grows this into
//! the org subsystem's full companion crate.

use holon_api::block::Block;

/// Properties that are internal bookkeeping (never part of an org round-trip)
/// and must be stripped before comparing a reference block against a
/// store-projected one.
pub const INTERNAL_PROPS: &[&str] = &[
    "sequence",
    "level",
    "ID",
    "id",
    "created_at",
    "updated_at",
    "document_id",
    "todo_keywords",
];

pub fn normalize_block(block: &Block) -> Block {
    let mut normalized = block.clone();
    normalized.created_at = 0;
    normalized.updated_at = 0;
    // sort_key is no longer a field of the domain Block (ADR 0005) — ordering is
    // validated separately via `assert_block_order` / `children_of`.
    // Trim overall content and normalize internal trailing whitespace per line
    // (org round-trip strips trailing whitespace from source block lines)
    normalized.content = normalized
        .content
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string();
    // The `__default__` page is prod's layout-owning root container (a real
    // block, not the sentinel — see `default_doc_block_uri`). The reference
    // model represents the document root as `__document_root__` and parents the
    // layout straight to it, so unify the two roots here.
    if normalized.parent_id.is_no_parent()
        || normalized.parent_id.is_sentinel()
        || normalized.parent_id == holon_api::default_doc_block_uri()
    {
        normalized.parent_id = holon_api::EntityUri::block("__document_root__");
    }
    // document_id removed from Block struct; no normalization needed
    for prop in INTERNAL_PROPS {
        normalized.properties.remove(*prop);
    }
    // Strip Null-valued and empty-string properties: the org parser stores
    // task_state=Null explicitly in the DB but the reference model omits absent
    // properties. Empty-string task_state means "no state" and is lost during
    // org round-trip (not written as a keyword, so not parsed back).
    normalized.properties.retain(|_, v| match v {
        holon_api::Value::Null => false,
        holon_api::Value::String(s) if s.is_empty() => false,
        _ => true,
    });
    // `task_state_category` is `task_state`'s sidecar — without a (non-empty)
    // keyword it carries no information, and the org round-trip drops the PAIR
    // (no keyword rendered → neither parsed back). The retain above already
    // dropped an empty/Null keyword; drop its orphaned sidecar with it, or the
    // ref (which stores ""+"active" after cycling to Clear, exactly like
    // block_raw) diverges from the org-parsed side on a phantom property.
    if !normalized.properties.contains_key("task_state") {
        normalized.properties.remove("task_state_category");
    }
    normalized
}
