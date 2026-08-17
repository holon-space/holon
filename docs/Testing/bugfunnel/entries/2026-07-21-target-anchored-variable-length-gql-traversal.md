---
id: 2026-07-21-target-anchored-variable-length-gql-traversal
date: 2026-07-21
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  Target-anchored variable-length GQL traversal compiled to an UNanchored
  O(N²) recursive CTE (gql-to-sql): a bounded var-length edge traversal from a
  fixed start node lowered to a CTE that seeds FROM every row (filtering to
  the anchor only at the end) instead of anchoring the recursive seed at the
  target — at ~vault scale it pegged holon-gpui at 115% CPU, blocked the
  single DatabaseActor, and forced a restart (live SqlOnly desktop dogfood).
  Same O(N×subtree) shape as the 2026-07-18 focus-descendant matview row,
  different producer (query compilation).
source_line: 1073
---

## Bug

Target-anchored variable-length GQL traversal compiled to an UNanchored
O(N²) recursive CTE (gql-to-sql): a bounded var-length edge traversal from a
fixed start node lowered to a CTE that seeds FROM every row (filtering to
the anchor only at the end) instead of anchoring the recursive seed at the
target — at ~vault scale it pegged holon-gpui at 115% CPU, blocked the
single DatabaseActor, and forced a restart (live SqlOnly desktop dogfood).
Same O(N×subtree) shape as the 2026-07-18 focus-descendant matview row,
different producer (query compilation).

## Missing piece

The keystone never generates GQL variable-length traversals at all (no
varlen edge query in the composed alphabet), and gql-to-sql itself had NO
target-anchored var-length compile test, so the unanchored lowering shipped
uncaught. Secondary ENVIRONMENT: the O(N²) is sub-millisecond at the
keystone's 3-block focus doc and only wedges at vault scale. Remedy: a
gql-to-sql compile assertion that a target-anchored varlen traversal anchors
its recursive seed at the start node, plus a vault-scale GQL-varlen
navigation rung.

## Remedy

FIXED same day (merged, live-verified 66.75ms): gql-to-sql PR #1 anchors the
recursive seed at the traversal target + holon PR #55 bump. The gap-closing
tests (anchored-seed compile assertion / scale rung) are NOT yet added — the
prod fix landed ahead of the coverage.
