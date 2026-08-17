---
id: 2026-08-14-document-membership-walk-boundary-exclusion-could
date: 2026-08-14
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  The document-membership walk's `Page`-boundary exclusion could have been
  swapped for a semantically wrong variant with no test going red.
source_line: 709
---

## Bug

(task-#22 turso-repin lane; found by agent exploration — differential SQL
probing of the production `get_blocks` CTE at our pin and at the real fork
head, no test produced it) **The document-membership walk's `Page`-boundary
exclusion could have been swapped for a semantically wrong variant with no
test going red.** The recursive arm must test the CHILD it is about to admit
(`bt.block_id = b.id`); a proposed rewrite keyed it on the CTE row
(`bt.block_id = d.id`) and was recorded as the prescribed remedy in
`recursive_cte_shape_architecture.rs` after being measured "safe". It is
not: a row already in `descendants` can never carry the tag, so the
sub-document boundary slides down one level and the `Page` itself is
admitted — wrong on the engine we ship today, not only after the re-pin.

## Root cause

task-#22 turso-repin lane, found by agent exploration — differential SQL
probing of the production `get_blocks` CTE against `tursodb` built at our
pin AND at the real fork head, not by any test: **the document-membership
walk's `Page`-boundary exclusion could have been replaced by a semantically
WRONG variant and NO test would have gone red.** The recursive arm excludes
blocks tagged `Page` by testing the CHILD it is about to admit (`bt.block_id
= b.id`). A rewrite keying that on the CTE row instead (`bt.block_id =
d.id`) was proposed — and recorded as the prescribed remedy in
`recursive_cte_shape_architecture.rs`'s doc comment — after being measured
"safe" on probes E/F. It is not safe: a row already in `descendants` can
never carry the tag, so keying on it moves the sub-document boundary DOWN
one level and admits the `Page` itself, together with nothing beneath it.
Measured wrong on the engine we ship TODAY, not only after the re-pin. The
escape is COVERAGE: every fixture put its `Page` among the document's DIRECT
children, where the base arm rejects it before the recursive arm is ever
consulted, so the correct and incorrect exclusions AGREE and no fixture
could discriminate. The composed keystone cannot close this — a page under a
non-page is prohibited by the interim page-hierarchy ruling and the
generator provably never produces one (pages are seed-only at `no_parent`,
ForkB-B1 R8, guarded by `inv-no-page-under-non-page`) — yet production still
reaches the topology through the OPEN whole-set `set_field("tags", […,
"Page"])` hole (2026-07-17 row below), because sync/org-ingest replay must
reproduce any state that was legal where it was authored. ORACLE secondary:
the architecture test that exists to guard this very CTE constrains only the
SHAPE of a `LEFT JOIN` and PASSES on the wrong rewrite — measured,
`lane-logs/t22-ARCH-on-wrong-rewrite.log` (2 passed) taken while
`lane-logs/t22-RED-cterow-membership.log` was RED on the same tree. FIXED:
both sites (`turso_seams.rs` `get_blocks` + `doc_block_topology`) rewritten
to `NOT EXISTS`, which is semantics-preserving, keys unmistakably on `b.id`,
and removes the `LEFT JOIN` shape that fails to terminate after the re-pin;
the third architecture clause is un-ignored and its wrong-remedy doc comment
corrected; the fixture gap is closed by the directed `holon-app` test
`doc_membership_page_boundary_below_root`, which puts the `Page` at depth 1
and is red-for-the-right-reason against the CTE-row rewrite.)

## Missing piece

Every fixture put its `Page` among the document's DIRECT children, where the
base arm rejects it first and both exclusions agree, so nothing could
discriminate; the composed keystone cannot close it either (a page under a
non-page is prohibited and the generator provably never draws one — ForkB-B1
R8 — though production still reaches the topology via the OPEN whole-set
`set_field("tags", […, "Page"])` replay hole, 2026-07-17). ORACLE secondary:
the architecture test guarding this CTE constrains only `LEFT JOIN` SHAPE
and PASSES on the wrong rewrite (`lane-logs/t22-ARCH-on-wrong-rewrite.log`,
2 passed, taken while `lane-logs/t22-RED-cterow-membership.log` was RED on
the same tree).

## Remedy

FIXED — both sites (`turso_seams.rs` `get_blocks` + `doc_block_topology`)
rewritten to `NOT EXISTS`: semantics-preserving, keys unmistakably on
`b.id`, and drops the `LEFT JOIN` shape that fails to terminate after the
re-pin. Third architecture clause un-ignored and its wrong-remedy doc
comment corrected. Fixture gap closed by the directed `holon-app` test
`doc_membership_page_boundary_below_root` (`Page` at depth 1),
red-for-the-right-reason against the CTE-row rewrite.
