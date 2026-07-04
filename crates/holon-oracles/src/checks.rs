//! Pure oracle check functions — the single shared implementation.
//!
//! The keystone PBT invariant bodies
//! (`holon-integration-tests/src/pbt/invariants/bodies/`) delegate here, and
//! the live [`crate::runner`] feeds the same functions from SQL snapshots.
//! Inputs are minimal typed rows (parse, don't validate: callers convert
//! their representation — `holon_api::Block` or SQL rows — at the boundary).

use std::collections::HashMap;
use std::collections::HashSet;

use holon_api::EntityUri;

/// Minimal row for parent-structure checks: `(id, parent_id)`.
#[derive(Clone, Debug)]
pub struct ParentRow {
    pub id: EntityUri,
    pub parent_id: EntityUri,
}

/// Minimal row for the source-language domain check.
#[derive(Clone, Debug)]
pub struct SourceLanguageRow {
    pub id: EntityUri,
    pub is_source: bool,
    pub source_language: Option<String>,
}

/// `inv-no-orphan-blocks` — every non-root block must reference a parent
/// that also exists in the snapshot. A dangling parent means the projection
/// lost a node. Expects the `block` matview snapshot.
pub fn find_orphans(rows: &[ParentRow]) -> Vec<String> {
    let all_ids: HashSet<&EntityUri> = rows.iter().map(|r| &r.id).collect();
    rows.iter()
        .filter(|r| !r.parent_id.is_no_parent() && !r.parent_id.is_sentinel())
        .filter(|r| !all_ids.contains(&r.parent_id))
        .map(|r| {
            format!(
                "[inv-no-orphan-blocks] orphan block: {} has invalid parent {} (parent not \
                 present in the matview snapshot)",
                r.id, r.parent_id
            )
        })
        .collect()
}

/// `inv-no-parent-cycles` — the block parent relation is acyclic: following
/// `parent_id` from any block reaches a root (no-parent / sentinel) without
/// revisiting a node. Expects the write-side `block_raw` snapshot.
pub fn find_parent_cycles(rows: &[ParentRow]) -> Vec<String> {
    let parents: HashMap<&EntityUri, &EntityUri> = rows
        .iter()
        .filter(|r| !r.parent_id.is_no_parent() && !r.parent_id.is_sentinel())
        .map(|r| (&r.id, &r.parent_id))
        .collect();

    let mut failures = Vec::new();
    // Nodes proven to reach a root — avoids re-walking shared chains.
    let mut safe: HashSet<&EntityUri> = HashSet::new();
    for start in parents.keys() {
        let mut seen: HashSet<&EntityUri> = HashSet::new();
        let mut current: &EntityUri = start;
        while let Some(parent) = parents.get(current) {
            if safe.contains(current) {
                break;
            }
            if !seen.insert(current) {
                failures.push(format!(
                    "[inv-no-parent-cycles] parent cycle detected walking up from {start}: \
                     revisited {current} (chain re-enters a node instead of terminating at a root)",
                ));
                break;
            }
            current = parent;
        }
        safe.extend(seen);
    }
    failures
}

/// `inv-source-language-iff-source` — ADR-0004 domain rule: a block carries a
/// `source_language` **iff** its content type is `Source`. Expects the
/// write-side `block_raw` snapshot.
pub fn find_source_language_violations(rows: &[SourceLanguageRow]) -> Vec<String> {
    rows.iter()
        .filter(|r| r.is_source != r.source_language.is_some())
        .map(|r| {
            format!(
                "[inv-source-language-iff-source] block {} violates the domain rule: is_source={} \
                 but source_language={:?} (source_language must be Some iff content_type == \
                 Source)",
                r.id, r.is_source, r.source_language,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, parent: &str) -> ParentRow {
        ParentRow {
            id: EntityUri::block(id),
            parent_id: EntityUri::block(parent),
        }
    }

    fn root_row(id: &str) -> ParentRow {
        ParentRow {
            id: EntityUri::block(id),
            parent_id: EntityUri::no_parent(),
        }
    }

    #[test]
    fn orphan_detected() {
        let rows = vec![root_row("r"), row("a", "r"), row("o", "missing")];
        let v = find_orphans(&rows);
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("block:o"), "{v:?}");
    }

    #[test]
    fn clean_forest_has_no_orphans_or_cycles() {
        let rows = vec![root_row("r"), row("a", "r"), row("b", "a")];
        assert!(find_orphans(&rows).is_empty());
        assert!(find_parent_cycles(&rows).is_empty());
    }

    #[test]
    fn cycle_detected() {
        let rows = vec![row("a", "b"), row("b", "a")];
        let v = find_parent_cycles(&rows);
        assert!(!v.is_empty());
        assert!(v[0].contains("parent cycle"), "{v:?}");
    }

    #[test]
    fn source_language_iff_source() {
        let ok = SourceLanguageRow {
            id: EntityUri::block("s"),
            is_source: true,
            source_language: Some("holon_prql".into()),
        };
        let bad = SourceLanguageRow {
            id: EntityUri::block("t"),
            is_source: false,
            source_language: Some("holon_prql".into()),
        };
        assert!(find_source_language_violations(&[ok.clone()]).is_empty());
        assert_eq!(find_source_language_violations(&[ok, bad]).len(), 1);
    }
}
