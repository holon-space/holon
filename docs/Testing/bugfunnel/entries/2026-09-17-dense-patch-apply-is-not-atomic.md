---
id: 2026-09-17-dense-patch-apply-is-not-atomic
date: 2026-09-17
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  Reported as "dense_patch applied its batch and then returned an error, twice";
  the two gates that errored both return BEFORE any dispatch, so those errors
  applied nothing — but the handler holds no transaction, so a mid-loop failure
  really can leave a batch half-applied and a retry really can duplicate rows.
---

## Bug

Reported by the orchestrator at ~00:50 on 2026-09-17 against the running app
and the live vault, on Now.org block
`block:c0450284-7413-44e6-a5dd-4680d09ad9f8`. Two consecutive `dense_patch`
calls returned errors while the caller observed the batch's rows present in the
store:

1. `unknown projection handle proj:0cf2dc8b-… — it was never issued or has been
   evicted; re-run dense_query`
2. on the retry, `conflict: 3 block(s) changed since dense_query — patch
   rejected`

The natural reading is "applied, then reported failure", whose consequence is
stated plainly by the reporter: **a caller who trusts the error re-applies and
duplicates rows.** That consequence is why this is recorded even though the
reading does not survive inspection.

## Root cause

What the source says, verified rather than assumed.

The running handler is `frontends/mcp/src/tools.rs` (`dense_patch`), and
`md5` is `743c933e3db70df4d6be51d622fead11` in BOTH the primary repo and this
workspace, so the running build's handler is this code. Its order is:

| line | step |
|---|---|
| `tools.rs:3149` | handle lookup → `dense_projection.rs:148` |
| `tools.rs:3152-3161` | parse the dense text, parse delete aliases |
| `tools.rs:3163-3218` | conflict re-read of `updated_at`, `return Err` at `:3211` |
| `tools.rs:3220-3240` | `dry_run` early return |
| `tools.rs:3246` | **apply loop begins** |

Both gates `return Err` before `:3246`. Neither of the two reported errors can
therefore follow a dispatch: **for these two errors the "applied then failed"
reading is refuted, and the rows the caller attributed to those batches were not
written by them.**

Which branch fired is pinned by the quoted text. `dense_projection.rs:148-165`
has two DISTINCT messages:

```
Some(p) if p.created.elapsed() < self.ttl => Ok(p.clone()),
Some(_) => bail!("projection handle {handle} has expired (TTL {}s) — re-run dense_query …"),
None    => bail!("unknown projection handle {handle} — it was never issued or has been evicted; …"),
```

The report quotes the `None` arm, so the handle was **absent from the registry**,
not present-and-expired. Two ways to be absent, both real:

- the 30-minute TTL sweep in `insert` removed it — `dense_projection.rs:138-146`
  does `map.retain(|_, p| p.created.elapsed() < self.ttl)` before inserting a
  NEW projection, so the next `dense_query` silently sweeps an expired handle
  and subsequent lookups take the `None` arm rather than the expired one;
- the registry is per-server-instance (`server.rs:481`,
  `ProjectionRegistry::new(Duration::from_secs(60 * 30))`), so a handle issued by
  a different MCP instance is unknown by construction, not by eviction.

### The defects that are real, and why this is filed rather than dismissed

1. **No atomic apply.** The handler holds no transaction — a grep for
   `transaction`/`rollback`/`begin()`/`commit()` over `tools.rs:3140-3560`
   returns 0 — and dispatches each op individually (create at `:3285`, the
   `UpdateTitle` arm at `:3293`, and so on for update/move/delete). Any failure mid-loop returns `Err` with the earlier ops
   already committed. So a batch really can be partially applied and reported as
   failed; the mechanism is a mid-loop failure, not the two gates the report
   named. That is the class to hunt.
2. **The snapshot is invalidated by the tool's own writes.** The conflict gate
   re-reads `updated_at` from the `block` matview (`:3176`) and compares it with
   the snapshot `dense_query` took. A successful patch writes the store, the org
   write-back renders the file, and the file watcher re-ingests, bumping
   `updated_at` on exactly the blocks the patch touched. So a retry with the
   pre-patch projection legitimately conflicts — which is what error 2 looks
   like. Composed with (1), this is the duplicate-rows hazard: the error says
   "re-run dense_query and retry", and a caller who does so without knowing the
   first patch landed re-applies it.
3. **The TTL is undisclosed and the two failures are indistinguishable in
   effect.** The handle lives 30 minutes; the tool description states no TTL;
   and for the caller's only real question — did anything land? — "never issued"
   and "expired" mean the same thing (nothing landed) yet read differently.
   A rejection message that said "a rejected patch applied NOTHING" would remove
   the trap on its own.

### Store check (read-only, live)

`SELECT id, content, task_state, updated_at FROM block WHERE parent_id =
'block:c0450284-7413-44e6-a5dd-4680d09ad9f8'` → **9 rows**; the same query
grouped by content `HAVING COUNT(*) > 1` → **0 groups**. No duplication is
visible now. That is not evidence against the hazard (the caller may have
reconciled between then and now), but it does mean this entry does not claim the
duplicate rows are in the store.

## Missing piece

**ORACLE.** The interaction is generatable and IS generated:
`crates/holon-integration-tests/src/pbt/composed/live_mcp.rs:718` drives a real
`dense_query → edit → dense_patch` round trip through the real MCP tool and back
into the store. What is absent is any judgement on failure: no invariant
anywhere under `crates/holon-integration-tests/src/pbt/` mentions all-or-nothing,
partial apply, or atomicity (verified by grep), and the drive exercises the
happy path only. So a batch that fails halfway and leaves half its ops committed
passes every invariant the fleet has.

**Secondary ENVIRONMENT.** Error 2 needs a store whose `updated_at` moves under
the caller's feet — a document with write-back enabled and a live file watcher.
The `:718` drive is an append-and-confirm round trip and the same harness states
"Watches are not driven over MCP", so the echo that invalidates the snapshot is
plausibly absent in the test environment. FLAGGED, not asserted: it needs one
look at that fixture's vault before it is used for anything.

## Remedy

OPEN. Ordered by value per unit of work:

1. **Write the tool descriptions' promise and make it true.** The description
   says "A stale/unknown handle is a loud error"; the caller's actual need is to
   know whether a rejection wrote anything. State it: a rejected or unknown-handle
   patch applies nothing, and a mid-loop failure currently does not honour that.
2. **Make the apply atomic** — one `transaction()` around the loop, or a
   plan-level commit — which removes the partial-apply class entirely. This is
   the change that makes (1) true rather than aspirational.
3. **Make the conflict rejection say what it knows**, and publish the handle TTL
   in the tool descriptions (or return the handle's issue time with it), so the
   caller is not left inferring whether to re-apply.
4. **Test work that closes the gap**: a fault-injected mid-loop failure plus an
   invariant asserting all-or-nothing (when `dense_patch` returns an error, the
   store equals the pre-patch state). The `:718` drive already has the MCP round
   trip to hang it on.

Not fixed here: this is a docs/triage lane and touches no code.

## Not established

Whether a mid-loop failure actually occurred at 00:50. The app-side log for that
session was not located in this lane, so the entry claims only what the source
proves: the two reported errors cannot have followed a dispatch, and a partial
apply is reachable by a route the report did not name. The reporter's own
inference is superseded on that point but its consequence — re-apply on a
misread error duplicates rows — is the reason the remedy above is written that
way.
