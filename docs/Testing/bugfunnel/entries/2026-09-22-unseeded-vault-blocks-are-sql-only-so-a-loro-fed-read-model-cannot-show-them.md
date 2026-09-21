---
id: 2026-09-22-unseeded-vault-blocks-are-sql-only-so-a-loro-fed-read-model-cannot-show-them
date: 2026-09-22
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A block with no Loro tree node (the declared "unseeded vault, SQL owns its
  order" class) lives in block_raw and nowhere else, so the D172.a read model,
  which is fed from Loro, structurally cannot publish it — the UI would stop
  showing those blocks the moment a read moves off SQL in F1b.
---

## Bug

After one `Reboot` in the keystone case
`reboot-orphans-the-previous-boots-watchers`, `block_raw` holds 44 blocks and
the Loro authority holds 42. The two extra rows are the cook file's
ingest-minted steps, with real content and a real parent:

```
sql (44 rows) only: [["block:keystone-recipe.cook::b::0", "block:5f54ed1a-…", "80",
                      "Crack the eggs into a bowl."],
                     ["block:keystone-recipe.cook::b::1", "block:5f54ed1a-…", "8180",
                      "Whisk in the flour."]]
read model (42 rows) only: []   live (42 rows) only: []
sql-only rows, as the Loro tree sees them:
  ["block:keystone-recipe.cook::b::0: loro_node=Never projection_armed=true",
   "block:keystone-recipe.cook::b::1: loro_node=Never projection_armed=true"]
```

**Measured, 3 of 10 runs** of that case in isolation
(`lane-logs/r5-reboot.log`; reproduce with
`HOLON_HAND_AUTHORED_CASE=reboot-orphans-the-previous-boots-watchers just hand-authored`).
Intermittent, which is why single runs have come back green.

The per-row diagnostic settles three competing readings:

- **`loro_node=Never`** — the tree has no node for these ids, live OR
  tombstoned. So they are NOT withheld deletes (that would read
  `Tombstoned`); the authority never held them.
- **`projection_armed=true`** — the DELETE pass was running. So this is not
  the unarmed-boot window either, although the same run logs
  `withholding 6 delete(s) (armed=false…)` during an EARLIER phase; by the
  time the oracle compares, the projection is armed and the rows remain.
- The oracle's bounded wait is 5 s and the divergence outlives it, so it is
  not pre-quiescence lag.

Pre-existing: the lane touches no ingest, arming or unseeded-vault code.

## Missing piece

COVERAGE, in two places.

The keystone could not see it before this lane: no invariant compared the Loro
authority against `block_raw` over the whole block set, so a row present in
one and not the other was nobody's business. `inv-blocks-match-ref/loro` and
`inv-blocks-match-ref/block_raw` each compare their own store against the
reference, and the reference is seeded from whichever store the draw treats as
authoritative — so a row that only SQL has passes both.

The lane's census (`loro_suite::read_model_second_writer_census`) measured 0
SQL-only rows on the shipped corpus and on the keystone recipe, which is true
at quiescence on a FIRST boot and was mistaken for the general case. The
reboot-into-an-unseeded-vault path is the one that produces them, and the
census does not reboot.

## Remedy

Not fixed here — it needs a ruling on whether the unseeded-vault class gets a
seed/repair pass or stays SQL-owned, and F1b cannot be planned without the
answer. Two shapes:

1. **A seed/repair pass** mirrors SQL-only blocks into the Loro tree, so the
   authority becomes complete and the read model can serve every block. Costs
   a migration; removes the class.
2. **The class stays**, and every read-model consumer must be declared partial
   — the UI keeps a SQL read for SQL-owned subtrees. Cheaper now, but it means
   F never fully replaces the SQL read path, which is most of its value.

The keystone does NOT exclude these rows. An exclusion was tried and
withdrawn: it could not tell this class from a block Loro deleted whose index
row survived (an index fault), and no shipped case exercised it — an exclusion
with no witness is a hole. The comparison is strict, so
`inv-view-model-matches-store-at-quiescence` is RED on this case roughly 3
runs in 10 until the ruling lands, and
`ReadModelObservation::sql_only_diagnostics` attaches the
live/tombstoned/never verdict and the armed flag to every failure.
