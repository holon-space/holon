//! Parsed origin of a rendered row (parse-don't-validate).
//!
//! A `DataRow` flowing through the render pipeline is one of three origins.
//! Historically the "is this a creation slot" question was answered by
//! string-sniffing the `:__virtual:` infix, scattered across the frontend
//! (`view_event_handler`, `reactive_view`, `shadow_builders`, plus dioxus-web
//! and the PBT bodies). This module parses that shape ONCE into a type so every
//! call site branches on a variant instead of a substring, and the wire
//! encoding lives in exactly one place ([`RowOrigin::creation_placeholder_id`]).
//!
//! Increment A (ADR 0015 / ADR 0016) only distinguishes `Canonical` from
//! `CreationPlaceholder`. The `DisplayPlaced` variant and the [`Occurrence`] /
//! [`OccurrenceId`] types are introduced here but left opaque: Increment B gives
//! `OccurrenceId` its meaning (which display placement produced the row) and the
//! wire carriage. Every construction in this increment defaults to `Canonical`,
//! so the refactor is behavior-preserving.

use holon_api::EntityUri;

/// The infix that marks a creation-placeholder row's synthetic id. It lives in
/// the **local** part of the URI (not the scheme) so `EntityUri::scheme()` still
/// returns the real entity type and the profile resolver finds the right
/// profile.
const VIRTUAL_MARKER: &str = ":__virtual:";

/// Opaque identifier for one display-placement occurrence of a block.
///
/// "Which display placement produced this row." Its inner meaning is set by
/// Increment B (the result-row re-home mechanism); this increment fixes only the
/// type. Deliberately opaque — no arithmetic exposed — so no caller can assume it
/// is a positional/render-path index (ADR 0016 rejects that identity). It is
/// `Ord`/`Hash` only so Increment B can use it in the `(EntityUri, Occurrence)`
/// store keyspace.
///
/// No constructor exists yet: Increment A introduces only the type. Increment B
/// adds the minting API when its result-row re-home mechanism first produces a
/// display-placed occurrence (the throwaway payload is `TBD-by-P2`, ADR 0016 §2).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct OccurrenceId(u32);

/// The occurrence coordinate of a focus/caret/store key (ADR 0016 §1 widened
/// tuple). `Canonical` is the default that makes the Increment A refactor
/// behavior-preserving: every existing key carries `Occurrence::Canonical`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Occurrence {
    #[default]
    Canonical,
    Placed(OccurrenceId),
}

/// Where a rendered row came from.
///
/// - `Canonical` — a real block; its id is a genuine `EntityUri`.
/// - `CreationPlaceholder` — the empty-collection "type here to create" slot; its
///   id is synthetic and its first edit materializes a real entity under
///   `parent` (`view_event_handler::handle_text_sync`).
/// - `DisplayPlaced` — the SAME real block rendered at a display-only position
///   (ADR 0015 P2 transclusion). Introduced here; **constructed by Increment B**,
///   which gives `OccurrenceId` its meaning and wire encoding. No wire encoding
///   exists yet, so [`RowOrigin::from_id`] never yields this variant in Increment
///   A.
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
    /// `CreationPlaceholder`; `DisplayPlaced` has no wire encoding yet (Increment
    /// B), so it is never produced here.
    ///
    /// The `Canonical` path is a single `split_once` with no allocation — the same
    /// cost as the `.contains(":__virtual:")` sniff it replaces, so the render hot
    /// path is unaffected.
    pub fn from_id(id: &str) -> Self {
        match id.split_once(VIRTUAL_MARKER) {
            Some((scheme, parent_local)) if !scheme.is_empty() && !parent_local.is_empty() => {
                // ALLOW(entity_uri_from_raw): synthetic creation-slot id (render-spec/row boundary)
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

    /// The synthetic id a `CreationPlaceholder` renders under. This is the single
    /// source of the `:__virtual:` wire encoding — previously duplicated in
    /// `reactive_view` and `shadow_builders::prelude`.
    pub fn creation_placeholder_id(parent: &EntityUri) -> String {
        format!("{}{VIRTUAL_MARKER}{}", parent.scheme(), parent.id())
    }

    /// Whether this row is the empty-collection creation slot.
    pub fn is_creation_placeholder(&self) -> bool {
        matches!(self, RowOrigin::CreationPlaceholder { .. })
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
}
