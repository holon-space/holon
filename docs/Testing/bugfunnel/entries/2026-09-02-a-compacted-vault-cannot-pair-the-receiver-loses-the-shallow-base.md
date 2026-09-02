---
id: 2026-09-02-a-compacted-vault-cannot-pair-the-receiver-loses-the-shallow-base
date: 2026-09-02
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  A vault whose Loro doc was compacted — which every session does on its first
  save — cannot pair: the receiver imports the owner's shallow snapshot into its
  own already-populated doc, does not inherit the shallow base, and everything
  it then authors is unmergeable by the owner.
---

## Bug

Lane `pair-inc0` (own-device pair, Increment 0), Inc 0b. Increment 0 proved the
ADR 0028 replicate-all path converges under concurrent structure and text when
both peers hold full history. Inc 0b re-ran the same question with the owner's
document compacted, which is not an edge case: `save_all` writes a SHALLOW
snapshot on the FIRST save of every session and every 64th after
(`crates/holon-loro/src/loro_document_store.rs:204-220`, `COMPACT_EVERY = 64`,
kill-switch `HOLON_LORO_COMPACT=off`). An owner is therefore shallow from its
second boot onward.

It does not converge, on either exchange, and it does not panic — every failure
is a loud `Err`, and D70's `tree_state.rs:1198` never fires on this path.

| Variant | Owner doc | Receiver's doc | Exchange | Result |
|---|---|---|---|---|
| control | full history | existing | `push_once`/`pull_once` | CONVERGES |
| shallow, relay leg | shallow | existing | `push_once`/`pull_once` | FAILS on the FORWARD leg |
| shallow, production exchange | shallow | existing | iroh snapshot fallback | FAILS on the REVERSE leg |
| shallow, receiver has no seed | shallow | existing | iroh snapshot fallback | FAILS the same way |
| shallow, receiver only creates | shallow | existing | iroh snapshot fallback | FAILS the same way |
| **P2 bootstrap shape** | shallow | **created by the import** | iroh snapshot fallback | **CONVERGES** |
| **stopgap** | `HOLON_LORO_COMPACT=off` | existing | iroh snapshot fallback | **CONVERGES** |

Every row above was also re-run against the loro-pin lane's REBASED fork
(`loro = "= 1.13.9"`, both patch entries repointed at the local rebase, scoped
`cargo update -p loro -p loro-internal`). The outcome is identical, with
byte-identical diagnostics. The three edits and `Cargo.lock` were reverted and
verified by sha256. **Upstream does not change this**, so it is not the D70 fork
defect and the fork rebase does not unblock it.

## Root cause

Measured, from the failing reverse leg:

```
[receiver->owner] importing 227 bytes FAILED: ImportUpdatesThatDependsOnOutdatedVersion
  destination shallow=true  since=VersionVector({<owner peer>: 46})
  source      shallow=false since=VersionVector({})
```

The two `shallow=` lines are the whole mechanism. The receiver imported the
owner's shallow snapshot into a document that already had history of its own —
at minimum `LoroBackend::initialize_schema`, usually a seed too — and did **not**
inherit the shallow base: its own `shallow_since` is empty, so it believes it
holds history from zero. Everything it subsequently authors causally depends on
containers the owner trimmed, and the owner's shallow document cannot accept an
op depending on history it no longer holds. The wire format has no way to say
"my base is at 46", so nothing the receiver authors on top of the snapshot can
come back.

Two probes bound it, and both widen rather than narrow the problem:

- A receiver with no pre-pairing seed fails identically, so an independent
  lineage on the peer is not the trigger.
- A receiver that only CREATES and never edits pre-compaction text fails
  identically, because a create under a pre-compaction parent depends on that
  parent's trimmed creation op. The rule is not "do not edit old notes" — it is
  that essentially nothing the peer authors is mergeable back.

And one probe isolates the cause exactly. When the receiver's document IS the
pairing payload — a bare `LoroDoc`, no schema init, no seed, into which the
owner's snapshot is imported — it reports

```
[P2] receiver after bootstrap: shallow=true since=VersionVector({<owner peer>: 46})
```

and the same concurrent merge converges both ways. **The defect is not
compaction. It is importing a shallow snapshot into a non-empty document.**

Secondary, and separable: the relay leg fails one step earlier than the
production leg because `push_once` exports
`ExportMode::updates_owned(from)` with no shallow guard
(`crates/holon-sharing/src/sync.rs:167`), while the iroh leg falls back to a
self-contained snapshot for a peer below the shallow base
(`crates/holon-loro/src/iroh_sync_adapter.rs:77-105`). Giving `push_once` the
same guard would align the two paths; it would not make pairing work.

## Missing piece

**ENVIRONMENT (primary).** No automated test has ever held a shallow Loro
document. Every composed session boots a fresh vault and runs on a
first-session, full-history doc, so both shallow branches — the adapter's
snapshot fallback and loro's shallow-import path — execute in production from
the second boot onward and in no test ever. The keystone's `SimulateRestart` is
explicitly a file-touch org re-ingest and states in its own doc comment that it
does "not drop+reopen the Turso handle, rehydrate the Loro container, or restart
the engine — the true persistence-across-restart class (a real `RebootStorage`)
is a separate, unbuilt transition (F9 fork)". So the one transition named
"restart" cannot produce the state.

**COVERAGE (secondary).** Because that reboot transition does not exist, no
sequence in the catalog reaches "compact, restart, then pair". Inc 0b had to
build the state by hand, using the `sync_suite`'s `backend_at` helper, which
rebuilds a peer over the same directory and is the only real restart seam in the
tree.

## Remedy

OPEN. Nothing fixed; this lane owns the convergence question.

Four `#[ignore]`d pins name the shapes, all in
`crates/holon/tests/sync_suite/sync_pbt.rs`:

- `replicate_all_over_the_relay_leg_fails_when_the_owner_doc_is_shallow`
- `replicate_all_converges_when_the_owner_doc_is_shallow`
- `shallow_owner_converges_with_a_receiver_that_has_no_own_history`
- `shallow_owner_and_a_receiver_that_only_creates`

Three tests stay un-ignored and guard the working paths:
`replicate_all_converges_under_concurrent_structure_and_text` (the full-history
control), `shallow_owner_converges_with_a_receiver_bootstrapped_into_an_empty_doc`
(the P2 shape), and `compaction_disabled_on_the_owner_lets_the_pair_converge`
(the stopgap). A fix that helps only the control therefore cannot look complete.

Three candidate remedies, in the order the evidence supports them:

1. **Pair by replacing the fresh peer's document** — measured working today, on
   both pins, with no engine change. It constrains the pairing gesture to a
   peer with nothing of its own to lose, which is what an own-device pair is.
2. **Make the import inherit the shallow base** when the destination is
   non-empty. A loro-level change; it is what the wire format would need to
   express.
3. **`HOLON_LORO_COMPACT=off` for a paired vault** — measured working, buys an
   unbounded oplog. A stopgap, not a design.

Related in the same lane:
`2026-09-02-two-instance-binary-is-red-on-main-and-in-no-gate` (the reboot
transition's absence is the same parity theme).
