---
id: 2026-09-19-navigation-focus-fails-with-a-foreign-key-violation
date: 2026-09-19
gap: ENVIRONMENT
secondary: ORACLE
status: OPEN
summary: >-
  A sidebar click on the real vault failed with `navigation.focus` raising
  `FOREIGN KEY constraint failed` while updating the navigation cursor; the
  click produced no visible error.
---

## Bug

Found by the `dogfood-integ` lane sending pointer clicks into the sidebar of the
real GPUI binary on a copy of Martin's vault. Log
`lane-logs/dogfood-integ-evidence/logs/app-boot1.log`, 20:11:27.259725:

```
ERROR interaction.dispatch{operation.entity="navigation" operation.name="focus"}:
holon_frontend::reactive: Operation navigation.focus failed: Operation 'focus'
on entity 'navigation' failed: Failed to update navigation cursor:
Query error: Failed to fetch row: FOREIGN KEY constraint failed
```

One occurrence in 84 clicks. The error reached the log at ERROR level but not
the screen: no toast, no banner, and the click simply did nothing.

## Root cause

Not isolated. The navigation cursor write references a block row, and the
referenced row was not present at write time. Two candidates, neither confirmed
in this session:

- The click targeted a row whose block had been removed or re-keyed between the
  hit-test and the cursor write — a race the settle in the keystone would mask.
- The target id is one of the ids the vault's duplicate-document-ID refusal
  declined to ingest (one file in this vault carries a `#+ID:` a sibling
  already claims and is refused; see the lane report), so the
  sidebar offered a row whose block never entered `block_raw`.

The second is worth checking first: it would make this a direct consequence of a
partially ingested vault, which is a state the test environment never enters.

## Missing piece

The keystone drives navigation against a vault where every offered row has a
backing block, because ingest either succeeds wholly or the test fails. A vault
with a REFUSED file is a first-class real-world state — the refusal is
deliberate, documented behaviour — and nothing exercises the frontend against
it.

Separately: an operation that fails at dispatch with a database constraint error
produces no user-visible signal. That is a fail-loud gap independent of the
cause.

## Remedy

Open. Determine which of the two candidates holds by correlating the failing
target id against the refused document's blocks. Then, regardless of the
outcome, surface a dispatch-level operation failure to the user — a click that
silently does nothing is the worst available behaviour.
