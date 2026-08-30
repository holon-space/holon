# Org → TOON syntax mapping

How each Org structural construct is represented in the TOON projection, and
what it costs. **Scope reminder:** TOON replaces only the *structural* layer
(hierarchy, task metadata, drawers, block identity). Inline content —
`[[links]]`, `*bold*`, `/italic/`, `~code~`, `#+BEGIN_SRC` bodies — is carried
**verbatim** as opaque text inside a cell; the TOON layer never parses or
rewrites it.

## The representation in one glance

The whole block forest is a **single TOON tabular array**, one row per block in
depth-first pre-order:

```
blocks[N]{id,depth,state,props,body,title}:
  1f2d3c4b-0a9e-4d21-b7c6-5e4f3a2b1c0d,0,,,,Example project page
  7a8b9c0d-1e2f-4a3b-8c4d-5e6f7a8b9c0d,1,DOING,,,Fix bugs
  3c4d5e6f-7a8b-4c9d-a0e1-2f3a4b5c6d7e,2,DOING,,,"Backlink list shows each entry twice ... deduplicate it"
  9e8d7c6b-5a4f-4e3d-9c2b-1a0f9e8d7c6b,2,DONE,,,Make autocomplete results stable
```

Six columns were chosen so the **common row stays as narrow as possible**:

| Column  | Holds                                                              |
|---------|-------------------------------------------------------------------|
| `id`    | bare block id (no `block:` scheme, per `ORG_SYNTAX.md`)            |
| `depth` | 0-based nesting depth — reconstructs the parent/child tree         |
| `state` | TODO keyword (`TODO`/`DOING`/`DONE`/…) or empty                    |
| `props` | everything rare, folded into one `key=value` cell (see below)      |
| `body`  | multi-line body (Text) / source code (Source) / file path (Image) |
| `title` | headline text, inline-verbatim (Text only)                        |

Everything that is **near-universal** (id, depth, state, title) gets a real
column. Everything **rare** (priority, tags, kind, language, scheduling,
`requires`, arbitrary drawer keys, collapse) is folded into the `props` cell,
which is empty — a bare `,,` — for the overwhelmingly common bare task row. This
keeps the delimiter tax on the common row at five commas instead of a dozen.

### The `props` sub-format

`props` is a space-separated `key=value` list packed into one TOON scalar.
Reserved keys carry a leading `@` sigil (`@pri`, `@tags`, `@kind`, `@lang`,
`@name`, `@sched`, `@dead`, `@req`, `@adv`, `@con`, `@col`, `@wo`) so they can **never**
collide with an arbitrary org drawer key (which is always alphanumeric/dash and
never starts with `@`). Within a token, ` `, `=`, `,`, `\` are backslash-escaped;
list-valued fields (`@tags`, `@req`, `@adv`, `@con`) escape element commas one layer
deeper so a comma *inside* a tag survives. The whole cell is then TOON-quoted if
it contains a structural char (it usually does — timestamps carry `:`).

Example (a claimed-block drawer, same shape as real vault data) becomes one cell:

```
"@col=t assigned-to=agent-1234 claimed-at=2026-01-02T03:04:05.678+00:00 claimed-from=/tmp/example/workspace"
```

## Per-construct verdict table

| Org construct | TOON representation | Verdict |
|---|---|---|
| **Headline hierarchy** (`*`, `**`, …) | `depth` column + pre-order row position; parent = nearest earlier row with smaller depth | **maps cleanly** |
| **TODO state** (`DONE`, `DOING`, custom keyword) | `state` column, bare | **maps cleanly** |
| **Priority** (`[#A]`) | `@pri=A` in props | **maps cleanly** |
| **Tags** (`:a:b:`) | `@tags=a,b` in props (element-comma-escaped) | **maps cleanly** |
| **Headline title** (with inline `[[..]]`, `*bold*`) | `title` column, verbatim | **maps with escaping cost** — a title containing `:` `[` `]` `{` `}` `,` `"` `\` is wrapped in `"..."` (+2 chars); `[[block:id][txt]]` links always trip this. Inner text is otherwise untouched. |
| **`:ID:` drawer** | `id` column (bare, scheme-stripped) | **maps cleanly** — the big structural win: the 3-line `:PROPERTIES:/:ID:/:END:` scaffolding collapses into one cell |
| **Arbitrary drawer keys** (`:assigned-to:`, `:Effort:`, `:status:`, `:source-file:`) | bare `key=value` tokens in props | **maps cleanly** — case preserved; multiple keys space-joined |
| **`:REQUIRES:` / `:BLOCKED-BY:` edge** | `@req=id1,id2` in props | **maps cleanly** (converges to one spelling, like the org renderer) |
| **`:ADVICE_SUPPRESSED:` edge** | `@adv=…` in props | **maps cleanly** |
| **`:contributes-to:` edge** | `@con=id1,id2` in props; the authored `none` sentinel is dropped per slug, so it never reaches TOON | **maps cleanly** |
| **`:COLLAPSED:`** | `@col=t` in props | **maps cleanly** |
| **`:WIDGET_ONLY:`** | `@wo=t` in props | **maps cleanly** |
| **Multi-line body** (paragraph text under a headline) | `body` column, newlines escaped as `\n` | **maps with escaping cost** — TOON has **no block-scalar / folded form**; every newline becomes a literal `\n` and the whole cell is quoted. A 10-line body becomes one long quoted string. Round-trips losslessly but is far less human-legible than org's native indented text. |
| **Source block** (`#+BEGIN_SRC lang … #+END_SRC`) | `@kind=src @lang=lang` in props; code in `body` (quoted, `\n`-escaped); `#+NAME:` → `@name` | **maps with escaping cost** — the payoff construct for the "clashing characters" question: SQL/Rhai bodies are *full* of `:` `,` `{` `}` `[` `]` and newlines, so the cell is always quoted and every line-break is `\n`. Semantically lossless (colons/brackets inside a quoted TOON string need no escaping beyond `"`/`\`/newline), but the source becomes a single unreadable line. Org's fenced block is strictly more readable here. |
| **Image block** (`[[file:path.png]]`) | `@kind=img` in props; bare path in `body` | **maps cleanly** |
| **`SCHEDULED:`/`DEADLINE:`** | `@sched=…` / `@dead=…` in props (timestamp verbatim) | **maps cleanly** (none present in the sampled files) |
| **`:LOGBOOK:` drawer** | not modelled | **does not map (out of scope)** — the sampled vault has none; would need either a reserved multi-line props field or its own side-table. Flagged, not solved. |
| **File-level `#+TITLE` / `#+TODO` / `#+ID`** | not modelled (projection is block-level) | **out of scope** — the projection is the block set an agent queries, not the file header |

### Characters that clash with TOON, and their cost

- **Structural chars in a cell** (`: , " [ ] { } \`, control chars, leading
  `-`/`#`): force the whole cell to be quoted (`+2` chars). Cheap. Extremely
  common in titles (links, colons) and universal in source bodies.
- **Newlines**: no folded form exists — escaped to `\n`. This is the real cost:
  multi-line bodies and source blocks collapse to one line and lose org's
  native legibility, even though they round-trip byte-for-byte.
- **Indentation**: irrelevant — the representation is single-line-per-block, so
  org's indentation-sensitive constructs never appear structurally; source-body
  indentation is preserved verbatim inside the quoted cell.

**Net:** every sampled construct round-trips losslessly (proven by the PBT). The
only constructs that map *with a real cost* are the multi-line ones (body,
source), and the cost is legibility, not correctness — the exact opposite of
where org is weak.
