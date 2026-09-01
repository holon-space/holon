---
id: 2026-09-01-shopping-retry-remints-idempotency-key
date: 2026-09-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A shopping sync round whose verifying re-pull was served stale committed the
  same Add a second time under a fresh command id, duplicating the item on the
  peer while reporting success.
---

## Bug

`holon_kitchen::shopping_sync::sync_once` returned `Ok` having added one item to
the shopping-list peer **twice**. Found by an adversarial verifier probing the
kitchen-c2 lane, not by any test in the suite.

Reproduction: one local row `("Milk","R")` with `last_seen_remote: None` (a
pending Add); peer empty at version 1; the peer applies commits faithfully; the
only perturbation is that the *verifying* re-pull is served one write stale — an
ordinary cached GET. The round committed the Add, read a snapshot that did not
yet show it, concluded the commit had not landed, and committed it again. The
peer ended with two `Milk` rows.

The damage is invisible from Holon's side: `CompleteSnapshot`'s `fold` collapses
duplicates on `ItemKey`, so the local list looks correct no matter how many
copies the peer holds.

## Root cause

Two independent contributors:

1. **The idempotency key was re-minted per attempt.**
   `CommitBatch::from_push_intents` built `id = format!("{now_ms}_{seq}")` and
   `sync_once` passed `now_ms + attempt`. The field the wire contract documents
   as the client-generated idempotency/ordering key
   (`docs/Plans/ThatShoppingList-API-2026-09-01.md`) therefore changed between
   the two sends of one logical command, so no peer-side deduplication could
   ever fire — the key was defeated in exactly the one case it exists for. The
   `seq` was positional too, so a retry with a shorter push list would have
   re-numbered surviving commands.

2. **A stale read was allowed to decide.** The round's conflict rule is "commit,
   then re-pull and re-reconcile". That is sound only if the re-pull is at least
   as new as the commit; nothing checked it. The shipped sidecar made a stale
   read likelier by dropping the `_nocache` / `version` / `oldVersion` query
   parameters that `docs/Plans/Kitchen.md` P36 records as the authoritative read
   request — the cache-buster was missing from the one request the retry rule
   depends on being fresh.

Evidence: `lane-logs/red-c2b-pbt.step.log` —
`the peer holds duplicate item(s) [("Milk", "R")] — a command was applied twice;
applied log: [("add","Milk","R","1756700000000_0"), ("add","Milk","R","1756700000001_0")]`.
The two ids differ by the attempt seed.

## Missing piece

**COVERAGE.** The interleaving was ungeneratable. `shopping_pull_mock.rs` was an
example suite — 13 hand-written `#[tokio::test]` cases with no generator and no
strategy — and it staged exactly one concurrent interleaving (`StaleFirstCommit`,
a *rejected* commit). The one that mattered, a commit that **succeeded** followed
by a **stale verifying read**, was never written down, so no case could reach it.

Not ORACLE: an invariant over the peer's contents would have fired immediately
had the state been reached. The lane report also framed the example suite as a
PBT, which overstated the coverage and is corrected.

## Remedy

* `crates/holon-kitchen/tests/shopping_sync_pbt.rs` — a proptest generator over
  peer/local mutation interleavings including a stale verifying re-pull and a
  concurrent writer landing between commit and re-pull. Oracle: Kitchen §4 plus
  "no logical command is applied twice at the peer" and "no local Add is lost".
  The mock peer honours the documented idempotency key and deliberately permits
  duplicate `(name, cat)` entries, so damage the sync causes stays observable.
* `command_id(round_ms, verb, key)` derives the id from the command, not from
  its position or the attempt, and `sync_once` passes one `now_ms` for the whole
  round.
* `pull_at_least(peer, floor)` refuses a snapshot older than a commit known to
  have landed — bounded re-reads, then a loud failure — so a provably stale read
  never decides whether to commit again.
* `assets/integrations/shopping.yaml` restores the captured `oldVersion` /
  `version` / `_nocache` parameters on the read call.

Note on teeth: the property is satisfied by *either* fix, so it cannot isolate
them. `a_retried_command_keeps_its_idempotency_key` and
`a_permanently_stale_read_fails_the_round_instead_of_re_committing` pin them
separately.

## Keystone repro

Not applicable to `general_e2e_composed_pbt.rs`: the composed keystone has no
shopping-peer component and no external-HTTP transition, so the interaction is
not reachable there. The covering property lives with the crate that owns the
peer, per the plan's staging of this connector.
