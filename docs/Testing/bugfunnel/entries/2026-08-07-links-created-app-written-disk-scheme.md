---
id: 2026-08-07-links-created-app-written-disk-scheme
date: 2026-08-07
gap: ORACLE
secondary: null
status: OPEN
summary: >-
  Links created in the app are written to disk WITH the `block:` scheme prefix
source_line: 1165
---

## Bug

(overnight dogfood-explorer, same session) **Links created in the app are
written to disk WITH the `block:` scheme prefix**, violating the invariant
`docs/Reference/ORG_SYNTAX.md` states outright — "org files store **bare
IDs** without `block:`/`doc:` scheme prefixes. The parser adds schemes at
the boundary, the renderer strips them." Typing `[[Links]]` into a block
produced, in `Deep.org`,
`[[block:ed7553a1-7d1a-1216-6623-ac99f8de27e9][Links]]`. Direct control in
the same vault: a hand-authored `[[d-l3a][level three alpha]]` in
`Links.org` round-tripped bare and byte-unchanged through the same
write-back. So the parse side honours the rule and the render side does not,
for app-created links specifically. The doc's own reason #2 for the rule is
RFC-3986 mis-parsing of scheme-shaped bare ids, which this output walks
straight into.

## Root cause

overnight dogfood — links CREATED IN THE APP are written to disk WITH the
`block:` scheme prefix, violating the invariant
`docs/Reference/ORG_SYNTAX.md` states outright ("org files store bare IDs
without `block:`/`doc:` scheme prefixes … the renderer strips them"). Typing
`[[Links]]` in a block wrote
`[[block:ed7553a1-7d1a-1216-6623-ac99f8de27e9][Links]]` into `Deep.org`,
while a hand-authored `[[d-l3a][level three alpha]]` in the same vault
round-tripped bare and unchanged. This is exactly the "URI parsing
ambiguity" the doc gives as reason #2 for the bare-ID rule)

## Missing piece

The keystone can and does generate link creation; nothing anywhere asserts
the SHAPE of a link as it lands on disk. Missing piece = a write-back
invariant that no rendered org line contains a scheme-prefixed target
(`[[block:` / `[[doc:`), which is a one-regex oracle over the vault and
would also cover the `:REQUIRES:` bare-id rule documented alongside it.

## Remedy

OPEN 2026-08-07 — diagnosis only.
