---
id: 2026-08-22-invalid-uri-id-panics-entity-uri
date: 2026-08-22
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  an `:ID:` that cannot form a URI path PANICS inside EntityUri instead of
  failing the file with a recoverable Err — a SPACE in an id is enough, and it
  is the likeliest real typo
---

## Bug

A headline drawer whose `:ID:` cannot form a URI path does not fail the file.
It PANICS (`crates/holon-api/src/entity_uri.rs:57-59`), taking the process down
on one hand-authored line.

The reachability is much wider than "control characters", which is how this was
first written up. MEASURED, 12 shapes, one fixture each:

| id shape            | example        | verdict  |
|---------------------|----------------|----------|
| interior space      | `has space`    | PANIC    |
| percent             | `has%pct`      | PANIC    |
| backslash           | `has\bs`       | PANIC    |
| open bracket        | `has[br`       | PANIC    |
| control character   | `has\u{7}ctl`  | PANIC    |
| caret               | `has^car`      | PANIC    |
| pipe                | `has\|pipe`    | PANIC    |
| double quote        | `has"dq`       | PANIC    |
| hash                | `has#hash`     | accepted |
| question mark       | `has?q`        | accepted |
| newline             | `has\nnl`      | accepted |
| LEADING space       | ` lead`        | accepted |

The sharpest case is the first: **`:ID: my note` panics**, while `:ID:  note`
(leading space, trimmed) does not. A space inside an id is an ordinary typo,
not an exotic input, and there is no reason a user would expect it to be fatal.

Found by the Increment 2b.2 capability certifier driving hostile-id probes for
`identity.id_constraints`. The probe needed `catch_unwind` to report at all,
which is how the panic surfaced: the certifier expected a refusal as an `Err`
and got an unwind.

Second defect of this shape from this direction today — see
`2026-08-22-bare-drawer-key-destroys-headline-drawer-and-id`. Both are reachable
from ordinary hand-authored or Emacs-written org and unreachable from Holon's
own renderer.

## Root cause

`EntityUri::new` builds `"{scheme}:{path}"` and parses it with `fluent_uri`,
turning a parse failure into a panic (`crates/holon-api/src/entity_uri.rs:57-59`):

```rust
EntityUri(Uri::parse(raw).unwrap_or_else(|e| {
    panic!("EntityUri::new({scheme:?}, {path:?}) produced invalid URI: {e}")
}))
```

The org parser promotes a bare drawer `:ID:` to a `block:` URI at the parse
boundary, so an authored id that cannot form a URI path reaches this
constructor directly. `#`, `?` and newline are accepted because they are legal
in the position `fluent_uri` parses them into, not because anything checked
them.

The CONSTRAINT is correct and intended — an id must form a valid URI path. The
MECHANISM is the defect: CLAUDE.md ranks a clear error far above a crash, and
every sibling refusal on this path (namespaced pages, id cycles, carrier
disagreement) is an `Err` naming the file.

## Missing piece

**No generator emits an id that cannot form a URI path.** The keystone's
vault-file fixtures use well-formed slugs and the org crate's proptests draw ids
from `[a-z]`-shaped alphabets, so nothing reached the constructor's failure
branch. The oracle side is not the gap — an unwind fails any test that reaches
it. Generation-only: COVERAGE, no secondary.

## Remedy

OPEN — reported, not fixed; 2b.2 is a certification increment.

1. Widen the vault-file generator so an authored `:ID:` may contain the eight
   measured shapes (COVERAGE fix; it should panic, red for the right reason,
   before any change).
2. Convert the constructor's failure to a `Result` at the parse boundary so the
   file is refused with a message naming it and the offending id.

Recorded in the org capability profile meanwhile: `identity.id_constraints`
declares `valid_uri_path`, and the certifier DRIVES it with the space case — so
the constraint is visible and measured even while its mechanism is wrong.
