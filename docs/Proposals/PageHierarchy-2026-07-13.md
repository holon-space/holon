# Page hierarchy, page files, and the parent edge (2026-07-13)

**Status:** PARKED — needs a dedicated 30-minute design session with Martin.
**Interim ruling (Martin, 2026-07-13):** pages under non-pages are PROHIBITED for now
(enforce/assume page parents are pages or roots). This sidesteps the path question
below until the real discussion happens, and is an input to Fork B B1 (task #84).

**Direction Martin stated:** every block that is a page should live in its own org
file. The unresolved question is the file path when the structural ancestry mixes
pages and non-pages (`A > b1 > b2 > P` with only A and P pages).

## Options catalogued so far (pros/cons discussed 2026-07-13)

Path schemes:
- **A. Flat pages dir, encoded lineage** (`pages/A%2FP.org`, LogSeq convention) —
  position-independent, LogSeq write-back trivial; not human-browsable, ancestor
  renames rename files.
- **B. Nested dirs by nearest-page-ancestor lineage** (`A/P.org`; the existing
  `Journals/{today}.org` pattern) — human-browsable, Obsidian-natural; page-status
  toggles and re-parenting MOVE files (needs rename-aware writeback), folder-note
  convention needed for pages with sub-pages.
- **C. Id-named files** — stable but unreadable; disqualified given foreign-vault work.
- **D. Per-dialect placement policy (meta-option, recommended)** — placement is part
  of the dialect YAML normal form (Holon-org=B, LogSeq=A, Obsidian=B-with-folder-notes);
  required anyway the moment we write LogSeq vaults. Path is always a hint, id is truth
  (links ruling).

Recording true structural position (the `b2` problem):
- **1. Parent-id property in the page file only** — single truth; parent file becomes
  lossy to foreign readers.
- **2. Stub/reference line `[[<id>][title]]` at the structural slot in the parent
  file (recommended)** — parent file stays complete; stub⟺file reconciliation is the
  same invariant class as #84 companion de-inline.
- **3. Both** — two truths that can disagree; rejected instinct.

## Martin, thinking out loud (preserve for the design session)

> I am thinking about whether we actually want the parent hierarchy to be so special
> as it is right now, or if it is only one way to link different blocks via directed
> edges (we already have other edges — requires, links) that happens to be serialized
> as LoroTree in Loro, as heading hierarchy in org, and maybe as FS hierarchy. But
> maybe FS hierarchy is actually a DIFFERENT type of edge? Maybe one that falls back
> to the existing parent hierarchy if not otherwise specified?

Implication if pursued: "where does this page's file live" stops being derived from
the structural parent and becomes its own (defaultable) edge/field — which would make
option D above the degenerate case (placement edge absent → dialect-default placement
from the parent hierarchy) and would let users pin a file location independently of
outline position. Interacts with: LoroTree cycle-prevention (edges outside the tree
don't get it for free), keystone ref model (parent_id is pervasive), org heading
round-trip, and Fork B writeback.

## Agenda for the design session

1. Is parent one edge kind among several? What invariants does it uniquely carry
   (single parent, acyclic, total order among siblings) that other edges don't?
2. Is FS placement a separate edge kind with parent-fallback?
3. Then re-derive the path scheme (A–D) and position recording (1–3) from that answer.
4. Lift the interim pages-under-non-pages prohibition or make it permanent.
