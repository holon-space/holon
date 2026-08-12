//! `inv-viewmodel-tree-virtual-slots`.
//!
//! @pbt oracle internal-consistency — R:RefBlockTree only LOCATES the Main
//!   focus roots; nothing from ref is compared as an expected value, the
//!   assertions are structural (slot order, no state_toggle in title subtree)
//! @pbt covers virtual-slot-misorder + page-title-wrong-variant — creation
//!   slot not last child; focused page title on the default (state_toggle)
//!   variant instead of the page_title bare-text variant
//! @pbt slips-if-removed the "add new block" slot renders above the page
//!   title, or the title renders as an editable state_toggle row instead of
//!   an h1; both dogfood regressions ship (devlog 2026-07-05)
//!
//! Locks in two GPUI render regressions the app currently exhibits — both
//! observed during dogfood triage, see
//! `devlog/2026-07-05-011500-gpui-dogfood-triage.md`:
//!
//! 1. **Virtual creation slot misordered.** The synthetic "add new block"
//!    creation slot (entity id containing `:__virtual:`, minted by the tree
//!    collection with `creation_slot: true`) must be the LAST child of its
//!    collection container. A root-sibling + sort-key encoding bug renders it
//!    ABOVE the page-title block instead of at the end. We walk the snapshot
//!    for any container node that has a `:__virtual:` child and assert that
//!    child is last among its siblings.
//!
//! 2. **Focused page title rendered with the wrong block-profile variant.** The
//!    focused page's title block (the `Main`-region focus root, a level-0 block
//!    that the tree overrides to `role == "page_title"`) must render via the
//!    `page_title` variant — a bare `text` h1 node (`text(col("content"),
//!    #{style: "h1"})`). The GPUI app instead renders it with the `default`
//!    variant, whose fingerprint is a `state_toggle` widget inside a row
//!    (`column(row(… state_toggle …), drop_zone())`). The focus-root id occurs
//!    MULTIPLE times in the snapshot (left-sidebar row, main-panel title row,
//!    …), so we check EVERY occurrence and assert none of their subtrees
//!    contains a `state_toggle` — stopping at the pre-order first match only
//!    ever inspected the sidebar row and was vacuously green. Blocks are
//!    flattened siblings in the tree collection (not nested widgets), so a
//!    focus-root node's subtree is that block's own rendering only.
//!
//! A tree collection with NO virtual slot is NOT a failure (the original stub
//! treated an absent creation slot as warn-level); such a container simply has
//! no `:__virtual:` child and the walk skips it, folding into `Ok`.
//!
//! Gate: an empty / unavailable snapshot (no entity ids reachable) → `Ok`.
//!
//! ## Bound
//!
//! `S: SutRenderer` for the widget tree; `R: RefBlockTree` for the
//! `Main`-region focus roots (`focus_root_ids`), resolved into SUT id space by
//! the runner's `with_resolved_doc_uris` view.

use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutRenderer;
use holon_pbt_core::capabilities::WidgetSnapshot;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvViewmodelTreeVirtualSlots;

impl InvViewmodelTreeVirtualSlots {
    pub const ID: InvariantId = InvariantId("inv-viewmodel-tree-virtual-slots");
}

/// Whether an entity id is a creation-placeholder slot. Single source of the
/// `:__virtual:` detection is `RowOrigin` (holon-frontend); the PBT body reads
/// through it rather than re-sniffing the infix.
fn is_creation_slot_id(id: &str) -> bool {
    holon_frontend::row_origin::RowOrigin::from_id(id).is_creation_placeholder()
}

/// First entity id found in the node's subtree (pre-order, self first).
/// Collection children may carry their block id on a wrapper or one level
/// deeper (tree_item → row → …), so position checks must not depend on which
/// level the id happens to sit at.
fn effective_entity_id(node: &WidgetSnapshot) -> Option<&str> {
    node.walk().find_map(|n| n.entity_id.as_deref())
}

fn is_virtual(node: &WidgetSnapshot) -> bool {
    effective_entity_id(node).is_some_and(is_creation_slot_id)
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvViewmodelTreeVirtualSlots
where
    R: RefBlockTree,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let root = sut.widget_tree_snapshot().await;

        // Gate: nothing rendered yet / renderer unavailable — no structure to
        // assert on. An empty entity-id set is the same "not ready" signal the
        // sibling `inv-viewmodel-entity-ids-subset-of-data` early-returns on.
        if root.collect_entity_ids().is_empty() {
            return InvariantResult::Ok;
        }

        // ── Bug 1: virtual creation slot must be the LAST child ─────────────
        // Every container that owns a `:__virtual:` child must place it at the
        // end of its sibling list. A container with no virtual child is not a
        // violation (absent creation slot = warn-level, folded into Ok).
        for node in root.walk() {
            // Only genuine collections count: the container must also hold at
            // least one NON-virtual entity child (a real row). Inside the
            // creation slot's own widget template every id-bearing node carries
            // the same `:__virtual:` id, and "position" among those is
            // meaningless.
            let has_real_row = node
                .children
                .iter()
                .any(|c| effective_entity_id(c).is_some_and(|id| !is_creation_slot_id(id)));
            if !has_real_row {
                continue;
            }
            // "Last" means: no REAL row may follow the virtual slot. Trailing
            // id-less chrome (spacer, drop_zone) after the slot is fine.
            for (pos, child) in node.children.iter().enumerate() {
                if !is_virtual(child) {
                    continue;
                }
                let real_after = node.children[pos + 1..]
                    .iter()
                    .any(|c| effective_entity_id(c).is_some_and(|id| !is_creation_slot_id(id)));
                if real_after {
                    let siblings: Vec<String> = node
                        .children
                        .iter()
                        .map(|c| {
                            effective_entity_id(c)
                                .map(str::to_string)
                                .unwrap_or_else(|| format!("<{}>", c.kind))
                        })
                        .collect();
                    let container = node
                        .entity_id
                        .clone()
                        .unwrap_or_else(|| format!("<{}>", node.kind));
                    return InvariantResult::Fail(format!(
                        "[inv-viewmodel-tree-virtual-slots] virtual creation slot '{}' is at \
                         position {pos} of {} in container '{container}' (kind '{}') but real \
                         rows follow it — it must come after every real row. Siblings in order:\n  \
                         {siblings:?}",
                        effective_entity_id(child).unwrap_or("<no-id>"),
                        node.children.len(),
                        node.kind,
                    ));
                }
            }
        }

        // ── Bug 2: focused page title must use the `page_title` variant ─────
        // The page_title variant is a bare `text` h1; the `default` variant's
        // fingerprint is a `state_toggle`. Within one collection blocks are
        // flattened siblings, so a block node's subtree is that block's own
        // rendering — a `state_toggle` in it means the wrong variant fired.
        for focus_root in ref_.focus_root_ids(CapRegion::Main) {
            // The focus-root id occurs MULTIPLE times in the snapshot (left
            // sidebar row, main-panel title row, …). Stopping at the pre-order
            // first match silently checked only the sidebar row (icon+text,
            // never a toggle) and let the main-panel title's `default`-variant
            // rendering through — vacuity found during triage, see
            // devlog/2026-07-05. Check EVERY occurrence: per the `eq("level",0)`
            // page_title rule the focus root is a level-0 row wherever it is a
            // tree root, and no occurrence of it may carry a state_toggle.
            // A snapshot with no occurrence of the id stays a non-failure
            // (same "not rendered here" gate as before).
            // A `live_block` carries the id of the block it DELEGATES to, and
            // its subtree is that block's whole render rather than its own row
            // — so it is a boundary, not an occurrence. The delegated render's
            // own level-0 row still appears below it and is checked.
            for block_node in root
                .walk()
                .filter(|n| n.kind != "live_block")
                .filter(|n| n.entity_id.as_deref() == Some(focus_root.as_str()))
            {
                let toggles = block_node
                    .walk()
                    .filter(|n| n.kind == "state_toggle")
                    .count();
                if toggles > 0 {
                    let kinds: Vec<&str> = block_node.walk().map(|n| n.kind.as_str()).collect();
                    return InvariantResult::Fail(format!(
                        "[inv-viewmodel-tree-virtual-slots] focused page title block '{}' \
                         rendered via the `default` variant ({toggles} state_toggle widget(s) in \
                         occurrence kind '{}', subtree {kinds:?}) instead of the bare-`text` \
                         `page_title` variant",
                        focus_root.as_str(),
                        block_node.kind,
                    ));
                }
            }
        }

        InvariantResult::Ok
    }
}
