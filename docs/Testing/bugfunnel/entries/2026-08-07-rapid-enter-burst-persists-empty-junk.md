---
id: 2026-08-07-rapid-enter-burst-persists-empty-junk
date: 2026-08-07
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  The rapid-Enter burst persists EMPTY junk blocks into the user's org file.
source_line: 1164
---

## Bug

(overnight dogfood-explorer, same session) **The rapid-Enter burst persists
EMPTY junk blocks into the user's org file.** Two content-less headlines
were written into `Deep.org` — `* ` (trailing space) each with its own
minted `:ID:` (`83f362a9-998d-44d9-af01-199fca7a2d11`,
`f1da47ef-15c4-4e34-be85-319a482d4269`) — and they survive a full
application restart. In the UI they paint as bare `Type here` placeholder
rows, visually identical to the panel's own `__virtual:` creation slot, so
nothing distinguishes two real persisted empty blocks in the vault from the
one affordance that is supposed to be there.

## Root cause

secondary ENVIRONMENT: overnight dogfood — the same rapid-Enter burst
PERSISTS empty junk blocks into the user's org file. Two content-less
headlines (`* ` with a trailing space, each with a minted `:ID:`) were
written into `Deep.org` and survive a full app restart. In the UI they
render as bare `Type here` placeholder rows, visually indistinguishable from
the panel's own `__virtual:` creation slot, so the user has no way to tell
that two real, persisted, empty blocks now sit in their vault)

## Missing piece

No layer drives a second interaction into an in-flight projection (see the
row above), so the empty-block artifact it produces has never been generated
either. Missing piece = the same no-settle rung, plus a vault-level
invariant that write-back never emits a content-less headline (cheap,
mechanical, and independently useful against any other source of empty
blocks).

## Remedy

OPEN 2026-08-07 — diagnosis only.
