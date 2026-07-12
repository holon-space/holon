//! Parsed origin of a rendered row (parse-don't-validate).
//!
//! A `DataRow` flowing through the render pipeline is one of three origins.
//! Historically the "is this a creation slot" question was answered by
//! string-sniffing the `:__virtual:` infix, scattered across the frontend
//! (`view_event_handler`, `reactive_view`, `shadow_builders`, plus dioxus-web
//! and the PBT bodies). This module parses that shape ONCE into a type so every
//! call site branches on a variant instead of a substring, and the wire
//! encoding lives in exactly one place
//! ([`RowOrigin::creation_placeholder_id`]).
//!
//! Increment A (ADR 0015 / ADR 0016) only distinguishes `Canonical` from
//! `CreationPlaceholder`. The `DisplayPlaced` variant and the [`Occurrence`] /
//! [`OccurrenceId`] types are introduced here but left opaque: Increment B
//! gives `OccurrenceId` its meaning (which display placement produced the row)
//! and the wire carriage. Every construction in this increment defaults to
//! `Canonical`, so the refactor is behavior-preserving.

use holon_api::EntityUri;
// `Occurrence` / `OccurrenceId` live in `holon-api` because the
// `ReactiveRowProvider` trait (there) keys its `keyed_rows_signal_vec` item by
// them; a frontend-side definition would be a circular dependency. Re-exported
// here so `holon_frontend::row_origin::Occurrence` (and the `RowOrigin`
// variant below) keep their historical path.
pub use holon_api::{Occurrence, OccurrenceId};

/// The infix that marks a creation-placeholder row's synthetic id. It lives in
/// the **local** part of the URI (not the scheme) so `EntityUri::scheme()`
/// still returns the real entity type and the profile resolver finds the right
/// profile.
const VIRTUAL_MARKER: &str = ":__virtual:";

/// Where a rendered row came from.
///
/// - `Canonical` — a real block; its id is a genuine `EntityUri`.
/// - `CreationPlaceholder` — the empty-collection "type here to create" slot;
///   its id is synthetic and its first edit materializes a real entity under
///   `parent` (`view_event_handler::handle_text_sync`).
/// - `DisplayPlaced` — the SAME real block rendered at a display-only position
///   (ADR 0015 P2 transclusion). Introduced here; **constructed by Increment
///   B**, which gives `OccurrenceId` its meaning and wire encoding. No wire
///   encoding exists yet, so [`RowOrigin::from_id`] never yields this variant
///   in Increment A.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowOrigin {
    Canonical,
    CreationPlaceholder {
        entity_type: String,
        parent: EntityUri,
    },
    DisplayPlaced {
        canonical_id: EntityUri,
        occurrence: OccurrenceId,
    },
}

impl RowOrigin {
    /// Parse the origin of a row from its `id` field (the render-pipeline wire
    /// format). Increment A distinguishes only `Canonical` vs
    /// `CreationPlaceholder`; `DisplayPlaced` has no wire encoding yet
    /// (Increment B), so it is never produced here.
    ///
    /// The `Canonical` path is a single `split_once` with no allocation — the
    /// same cost as the `.contains(":__virtual:")` sniff it replaces, so
    /// the render hot path is unaffected.
    pub fn from_id(id: &str) -> Self {
        match id.split_once(VIRTUAL_MARKER) {
            Some((scheme, parent_local)) if !scheme.is_empty() && !parent_local.is_empty() => {
                // ALLOW(entity_uri_from_raw): synthetic creation-slot id (render-spec/row
                // boundary)
                let parent = EntityUri::from_raw(&format!("{scheme}:{parent_local}"));
                RowOrigin::CreationPlaceholder {
                    entity_type: scheme.to_string(),
                    parent,
                }
            }
            _ => RowOrigin::Canonical,
        }
    }

    /// Parse the origin from a `DataRow`'s `id` field. A row with no `id` is
    /// `Canonical`.
    pub fn from_row(row: &holon_api::widget_spec::DataRow) -> Self {
        row.get("id")
            .and_then(|v| v.as_string())
            .map(Self::from_id)
            .unwrap_or(RowOrigin::Canonical)
    }

    /// The synthetic id a `CreationPlaceholder` renders under. This is the
    /// single source of the `:__virtual:` wire encoding — previously
    /// duplicated in `reactive_view` and `shadow_builders::prelude`.
    pub fn creation_placeholder_id(parent: &EntityUri) -> String {
        format!("{}{VIRTUAL_MARKER}{}", parent.scheme(), parent.id())
    }

    /// Whether this row is the empty-collection creation slot.
    pub fn is_creation_placeholder(&self) -> bool {
        matches!(self, RowOrigin::CreationPlaceholder { .. })
    }
}

/// Resolve the parent a creation slot must dispatch `block.create` under, from
/// the collection's **rendered rowset** — NOT from a static container id.
///
/// Bug 2A: the panel's `live_query` context id (`block:default-main-panel`) is
/// the render container, not the query's focus root. Parenting new blocks to it
/// silently mis-parents every top-level create under the panel container. The
/// focus root only materialises at render time in the query result, so it must
/// be read back out of the rows.
///
/// Two rowset shapes carry a creation slot:
/// - **flat** (`from children` of a container): every displayed row is a direct
///   child of the same `container` → new items parent to that `container`.
/// - **rooted tree** (focus root + descendants, e.g. the main panel's `MATCH
///   (fr:focus_root) … RETURN d`): exactly one row is the forest root (its
///   stated parent is not itself displayed) → new items parent to that root,
///   i.e. `fr.root_id`, the focused block.
///
/// `container` is used ONLY to recognise the flat shape; it is NEVER returned
/// blindly. Per the fail-loud contract:
/// - empty / not-yet-resolvable rowset → `None` (the streaming path shows no
///   slot rather than silently mis-parenting; the panel's `0..N` query always
///   returns its root, so a focused page is never truly empty here);
/// - a single forest root → parent to that root;
/// - a multi-root forest → gated by `allow_root_creation` (below).
///
/// `allow_root_creation` gates the multi-root forest — a list of top-level
/// entities. Two legitimate multi-root shapes exist: a flat top-level-pages
/// list (every root at the `no_parent` sentinel) and a NESTED-PAGE forest where
/// a page is parented to another page filtered out of this rowset (BugFunnel
/// #67: a subdir page-file roots the date page under a non-`Page` folder-page).
/// Per the links-ruling nested pages are a REPRESENTABLE state, so a mixed
/// forest is legal data, not corruption.
/// - `allow_root_creation = false` (read-only navigation list, e.g. the Pages
///   sidebar): ANY multi-root forest → `None`. No creation slot, so no parent
///   needs resolving; the list no longer renders a phantom
///   `sentinel:__virtual:no_parent` row and no longer PANICS on a nested-page
///   forest at boot (BugFunnel #61 + #67).
/// - `allow_root_creation = true` (editable list opting in via `creation_slot:
///   true`): a new item is a NEW TOP-LEVEL PAGE parented to the `no_parent`
///   sentinel, resolved whenever the forest carries at least one
///   sentinel-rooted anchor (mixed nested-page forests included). A
///   creation-enabled forest whose EVERY root dangles to a real-but-absent
///   parent has no top-level anchor → `panic!` (genuinely malformed
///   query/render spec; fail-loud remains right for truly impossible states).
///
/// The single-root (main panel focus root) and flat-container shapes are
/// unaffected — they never needed the opt-in.
pub fn resolve_creation_parent(
    rows: &[std::sync::Arc<holon_api::widget_spec::DataRow>],
    container: &EntityUri,
    allow_root_creation: bool,
) -> Option<EntityUri> {
    use holon_api::widget_spec::data_row_parent_id;

    // Ignore any already-appended creation-slot row (idempotent re-resolution).
    let real: Vec<&std::sync::Arc<holon_api::widget_spec::DataRow>> = rows
        .iter()
        .filter(|r| !RowOrigin::from_row(r).is_creation_placeholder())
        .collect();
    if real.is_empty() {
        return None;
    }

    // Flat shape: a displayed row is a direct child of the container.
    let container_str = container.as_str();
    if real.iter().any(|r| {
        data_row_parent_id(r)
            .map(|p| p.as_str() == container_str)
            .unwrap_or(false)
    }) {
        return Some(container.clone());
    }

    // Rooted-tree shape: the forest root is the row whose stated parent is not
    // itself displayed.
    let ids: std::collections::HashSet<String> = real
        .iter()
        .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(String::from))
        .collect();
    let roots: Vec<&std::sync::Arc<holon_api::widget_spec::DataRow>> = real
        .iter()
        .filter(|r| match data_row_parent_id(r) {
            None => true,
            Some(p) => !ids.contains(p.as_str()),
        })
        .copied()
        .collect();
    match roots.as_slice() {
        [single] => {
            let id = single
                .get("id")
                .and_then(|v| v.as_string())
                .expect("rendered row carries an 'id' column");
            // ALLOW(entity_uri_from_raw): forest-root id read back from a rendered row
            // (render-pipeline/row boundary)
            Some(EntityUri::from_raw(id))
        }
        // Every real row's parent is present → a cycle; not resolvable yet.
        [] => None,
        many => {
            // A multi-root forest. Two legitimate shapes reach here:
            //  - a top-level-pages list (e.g. the Pages sidebar `WHERE tag='Page'`), whose
            //    roots are parented to the `no_parent` sentinel; and
            //  - a NESTED-PAGE forest, where a page is parented to another page that this
            //    rowset filters out (BugFunnel #67: a subdir page-file
            //    `Journals/2026-07-10.org` roots the date page under the `journals`
            //    folder-page, which is not itself a `Page` and so is absent from `WHERE
            //    tag='Page'`). Per the links-ruling nested pages are a REPRESENTABLE state
            //    (anchored by id), so a mixed forest — some roots at the sentinel, some
            //    dangling to an absent-but-real parent — is legal data, NOT a corrupt
            //    rowset.
            //
            // A read-only navigation list (`allow_root_creation = false`, e.g.
            // the sidebar) never offers a creation slot, so it does not need to
            // resolve a parent at all: ANY multi-root forest → `None` (no slot,
            // no panic). This is the fix for the boot panic.
            if !allow_root_creation {
                return None;
            }
            // Creation-enabled list (`creation_slot: true`): a new item is a NEW
            // TOP-LEVEL PAGE, parented to the `no_parent` sentinel. This is only
            // meaningful when the forest actually carries a top-level anchor (at
            // least one sentinel-rooted row); mixed forests with nested pages
            // still resolve here. A creation-enabled forest whose EVERY root
            // dangles to a real-but-absent parent has no top-level to create
            // under → genuinely malformed query/render spec → loud panic.
            let no_parent = EntityUri::no_parent();
            if many.iter().any(|r| {
                data_row_parent_id(r)
                    .map(|p| p == no_parent)
                    .unwrap_or(false)
            }) {
                return Some(no_parent);
            }
            panic!(
                "creation slot: rowset has {} disjoint root rows (container={container}); \
                 creation enabled but no top-level (no_parent) anchor — malformed query or render \
                 spec",
                many.len()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_placeholder_id_round_trips_through_from_id() {
        // ALLOW(entity_uri_from_raw): test fixture
        let parent = EntityUri::from_raw("block:default-main-panel");
        let id = RowOrigin::creation_placeholder_id(&parent);
        assert_eq!(id, "block:__virtual:default-main-panel");
        match RowOrigin::from_id(&id) {
            RowOrigin::CreationPlaceholder {
                entity_type,
                parent: p,
            } => {
                assert_eq!(entity_type, "block");
                assert_eq!(p.as_str(), "block:default-main-panel");
            }
            other => panic!("expected CreationPlaceholder, got {other:?}"),
        }
    }

    #[test]
    fn a_real_id_is_canonical() {
        assert_eq!(RowOrigin::from_id("block:abc123"), RowOrigin::Canonical);
        // Empty scheme or empty parent local is not a creation slot.
        assert_eq!(RowOrigin::from_id(":__virtual:x"), RowOrigin::Canonical);
        assert_eq!(RowOrigin::from_id("block:__virtual:"), RowOrigin::Canonical);
    }

    #[test]
    fn occurrence_defaults_to_canonical() {
        assert_eq!(Occurrence::default(), Occurrence::Canonical);
    }

    // ─── resolve_creation_parent (bug 2A) ───────────────────────────────────

    use std::sync::Arc;

    fn row(id: &str, parent: Option<&str>) -> Arc<holon_api::widget_spec::DataRow> {
        let mut r = std::collections::HashMap::new();
        r.insert("id".to_string(), holon_api::Value::String(id.to_string()));
        if let Some(p) = parent {
            r.insert(
                "parent_id".to_string(),
                holon_api::Value::String(p.to_string()),
            );
        }
        Arc::new(r)
    }

    fn uri(s: &str) -> EntityUri {
        // ALLOW(entity_uri_from_raw): test fixture
        EntityUri::from_raw(s)
    }

    /// The panel shape: `MATCH (fr:focus_root),(root)<-[:CHILD_OF*0..N]-(d)`
    /// returns the focus root (level 0) + its descendants. The container is the
    /// panel id, which is NOT a parent of any returned row → the slot must
    /// parent to the focus root, NOT the panel.
    #[test]
    fn panel_tree_resolves_to_focus_root_not_container() {
        let rows = vec![
            row("block:page", Some("block:root-layout")), // focus root (level 0)
            row("block:c1", Some("block:page")),
            row("block:c2", Some("block:page")),
        ];
        let parent = resolve_creation_parent(&rows, &uri("block:default-main-panel"), false)
            .expect("resolvable");
        assert_eq!(parent.as_str(), "block:page");
    }

    /// An empty focused page still returns its own row (0-hop `d = root`), so
    /// the slot parents to the page even with no children.
    #[test]
    fn empty_focused_page_resolves_to_the_page() {
        let rows = vec![row("block:page", Some("block:root-layout"))];
        let parent = resolve_creation_parent(&rows, &uri("block:default-main-panel"), false)
            .expect("resolvable");
        assert_eq!(parent.as_str(), "block:page");
    }

    /// Flat `from children` of a container: every row is a direct child of the
    /// container → new items parent to the container.
    #[test]
    fn flat_children_resolve_to_container() {
        let rows = vec![
            row("block:a", Some("block:container")),
            row("block:b", Some("block:container")),
        ];
        let parent =
            resolve_creation_parent(&rows, &uri("block:container"), false).expect("resolvable");
        assert_eq!(parent.as_str(), "block:container");
    }

    /// Flat with a single child must still parent to the container, NOT the
    /// lone child's own id.
    #[test]
    fn flat_single_child_resolves_to_container() {
        let rows = vec![row("block:only", Some("block:container"))];
        let parent =
            resolve_creation_parent(&rows, &uri("block:container"), false).expect("resolvable");
        assert_eq!(parent.as_str(), "block:container");
    }

    /// An empty rowset is not yet resolvable → `None` (no silent container
    /// mis-parent). On the streaming path this shows no slot until data loads.
    #[test]
    fn empty_rowset_is_unresolvable() {
        assert!(resolve_creation_parent(&[], &uri("block:container"), false).is_none());
    }

    /// An already-appended creation-slot row is ignored (idempotent).
    #[test]
    fn creation_slot_row_is_ignored() {
        let rows = vec![
            row("block:page", Some("block:root-layout")),
            row("block:__virtual:page", Some("block:page")),
        ];
        let parent = resolve_creation_parent(&rows, &uri("block:default-main-panel"), false)
            .expect("resolvable");
        assert_eq!(parent.as_str(), "block:page");
    }

    /// The Pages sidebar (`WHERE tag='Page'`) renders a flat forest of all
    /// top-level pages, each parented to the `no_parent` sentinel. This is a
    /// read-only navigation list that does NOT opt in (`allow_root_creation =
    /// false`), so it gets NO creation slot — no phantom
    /// `sentinel:__virtual:no_parent` row (BugFunnel #61).
    #[test]
    fn top_level_pages_list_without_optin_gets_no_slot() {
        let sentinel = EntityUri::no_parent();
        let rows = vec![
            row("block:pageA", Some(sentinel.as_str())),
            row("block:pageB", Some(sentinel.as_str())),
            row("block:pageC", Some(sentinel.as_str())),
        ];
        assert!(resolve_creation_parent(&rows, &uri("block:journals"), false).is_none());
    }

    /// An editable top-level-pages list that DOES opt in
    /// (`creation_slot: true`) resolves the forest-root slot to the `no_parent`
    /// sentinel — a "create a new top-level page" slot.
    #[test]
    fn top_level_pages_list_with_optin_resolves_to_no_parent() {
        let sentinel = EntityUri::no_parent();
        let rows = vec![
            row("block:pageA", Some(sentinel.as_str())),
            row("block:pageB", Some(sentinel.as_str())),
            row("block:pageC", Some(sentinel.as_str())),
        ];
        let parent =
            resolve_creation_parent(&rows, &uri("block:journals"), true).expect("resolvable");
        assert_eq!(parent, sentinel);
    }

    /// A read-only list (`allow_root_creation = false`) that renders a
    /// multi-root forest with dangling (real-but-absent) parents — the
    /// shape that used to PANIC — now gets NO slot (`None`). A navigation
    /// list never creates, so a multi-root forest is expected, never
    /// corrupt.
    #[test]
    fn read_only_disjoint_roots_get_no_slot() {
        let rows = vec![
            row("block:p1", Some("block:outsideA")),
            row("block:p2", Some("block:outsideB")),
        ];
        assert!(resolve_creation_parent(&rows, &uri("block:default-main-panel"), false).is_none());
    }

    /// BugFunnel #67 reproduction at the unit level: a NESTED-PAGE forest. The
    /// Pages sidebar (`WHERE tag='Page'`) renders top-level pages at the
    /// `no_parent` sentinel PLUS a date page parented to the `journals`
    /// folder-page, which is not itself a `Page` and so is absent from the
    /// rowset. That mixed forest (some sentinel roots, one dangling root) used
    /// to PANIC at boot. Read-only (`allow_root_creation = false`) →
    /// `None`, no panic.
    #[test]
    fn nested_page_forest_read_only_gets_no_slot() {
        let sentinel = EntityUri::no_parent();
        let rows = vec![
            row("block:pageA", Some(sentinel.as_str())),
            row("block:journal-2026-07-10", Some("block:journals")), // parent filtered out
        ];
        assert!(resolve_creation_parent(&rows, &uri("block:journals-sidebar"), false).is_none());
    }

    /// The SAME mixed nested-page forest on an editable list that opts in
    /// (`allow_root_creation = true`) resolves the new-item slot to the
    /// `no_parent` sentinel (a "create a new top-level page" slot), because the
    /// forest carries at least one sentinel-rooted anchor.
    #[test]
    fn nested_page_forest_with_optin_resolves_to_no_parent() {
        let sentinel = EntityUri::no_parent();
        let rows = vec![
            row("block:pageA", Some(sentinel.as_str())),
            row("block:journal-2026-07-10", Some("block:journals")),
        ];
        let parent = resolve_creation_parent(&rows, &uri("block:journals-sidebar"), true)
            .expect("resolvable");
        assert_eq!(parent, sentinel);
    }

    /// A creation-enabled forest whose EVERY root dangles to a real-but-absent
    /// parent (NO sentinel anchor at all) has nothing top-level to create under
    /// — a genuinely malformed query/render spec → loud panic (fail-loud
    /// for truly impossible states).
    #[test]
    #[should_panic(expected = "no top-level")]
    fn creation_enabled_all_dangling_roots_panic() {
        let rows = vec![
            row("block:p1", Some("block:outsideA")),
            row("block:p2", Some("block:outsideB")),
        ];
        let _ = resolve_creation_parent(&rows, &uri("block:default-main-panel"), true);
    }
}
