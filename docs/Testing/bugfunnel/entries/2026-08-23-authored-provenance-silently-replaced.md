---
id: 2026-08-23-authored-provenance-silently-replaced
date: 2026-08-23
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  A `create`/`update` carrying an authored `_provenance` property returned
  SUCCESS while the engine overwrote the value with its own stamp — the caller
  was told its attribution landed when it had been discarded.
---

## Bug

`_provenance` is the block property that carries the authorship stamp
(`ProvenanceStamp`: origin + session/tool-call/transition ids + `at_millis`).
The engine mints it at the dispatch chokepoint. A caller that put its OWN
`_provenance` into `create`/`update` params got an `Ok` outcome and a block
whose stamp was the engine's — the authored value vanished with no error, no
warning, and no trace in the response.

Found by code audit during the D5 design review, not by a test. Ruled by Martin
as D5.a: an authored engine-owned key is a NAMED ERROR at the write boundary.

## Root cause

`crates/holon/src/api/operation_engine.rs` `stamp_params` used an unconditional
`insert`:

```rust
if PROVENANCE_STAMPED_OPS.contains(&op_name) {
    let stamp = ProvenanceStamp::from_origin(origin, now_millis);
    params.insert(Arc::from(PROVENANCE_PROPERTY), stamp.to_value());
}
```

`HashMap::insert` replaces silently, so the authored value was discarded by the
same statement that minted the engine's. This is the "silently degrades to look
fine" branch the project's error-handling philosophy ranks last: the write
succeeded, the stored state was internally consistent, and only a reader who
compared what they sent against what came back could tell.

### Provenance of the reds — not all three are equal

Route 1 was measured red-for-the-right-reason BEFORE the fix — `d5a-red.log`,
an end-to-end probe through the production write path
(`crates/holon/tests/capability_certification.rs`
`an_authored_engine_owned_key_is_refused_at_the_write_boundary`): the write
carrying `_provenance` returned `Ok`, so the test failed with "'create' carrying
the engine-owned '_provenance' must be REFUSED". Route 2's red was measured the
same way, before its fix.

**Route 3's red was RECOVERED, not red-first, and that is recorded here rather
than smoothed over.** The bag fix was written before its tests. The red in
`r4-red-bag.log` was then obtained by saving `operation_engine.rs`, disabling
ONLY the route-3 bag descent (routes 1–2 left intact), running, and restoring
from the copy — byte-identity verified by sha256 before and after
(`b5c2faf0…7fae`), with zero disable markers left in the tree. It is therefore a
genuine measurement of pre-fix behaviour, but it is weaker evidence than a
red-first red: the test was authored by someone who already knew the fix, so it
cannot testify that the fix was designed to satisfy an independent test.

Independently corroborated by the R3 verifier, which cross-checked the two
states rather than taking the log's word: the red state carries compiler
warnings that the frozen tree does not (a dead `bag_keys`, an unused `op_name`),
and the counts differ 3-red / 14-green. That is what makes the two-state claim
checkable by a later reader.

No production caller sends the key. Grep across the tree (including
`assets/integrations/*.yaml`, the MCP client paths and the org-ingest param
builder) found only readers, plus `template_instantiation.rs:48`, which already
lists `_provenance` in `NON_COPYABLE_PROPERTIES` — the one place that could
have propagated it deliberately excluded it. So the refusal breaks nothing that
exists.

It closes THREE routes into the same declared leg, and they fail in two
different ways. The distinction matters: a DISCARD lies to the author about
their own write, a FORGE lies to every later reader about who wrote it.

| # | Route | Key sits in | Pre-fix behaviour |
|---|---|---|---|
| 1 | `create` / `update` | the param KEYS | **DISCARD** — `insert` replaces the authored value with the engine stamp |
| 2 | `set_field(field="_provenance")` | the VALUE of the `field` param | **FORGE** — nothing overwrites it, so the authored stamp stands |
| 3a | `set_field(field="properties", value=<bag>)` | one level deeper, inside the BAG | **FORGE** — `properties` is a real `block_raw` column, so this takes the direct-column branch and replaces the WHOLE blob |
| 3b | `create`/`update` with a nested `properties` bag | one level deeper, inside the BAG | **DISCARD** — `partition_params` + `or_insert_with` let the engine stamp win the merge |

Routes 2 and 3a are the dangerous half: `history_store.rs` (the
substrate-rebuild read) and `trust_proposals_matview.sql` both read
`_provenance` as authoritative, so an origin able to issue `set_field` could
name a DIFFERENT origin as the author. Routes 2 and 3 were each found by
adversarial verification of the previous fix, not by a test — the first fix
scoped its check to an op allowlist, and the second still read only the NAMED
field.

No production caller uses any of them. Grep across the tree (including
`assets/integrations/*.yaml`, the MCP client paths and the org-ingest param
builder) found only readers, plus `template_instantiation.rs:48`, which already
lists `_provenance` in `NON_COPYABLE_PROPERTIES`. Specifically checked for the
bag routes: the org-ingest param builder emits FLAT keys and never a
`properties` bag, and undo/redo inverses — which DO carry a whole captured bag
including the stamp — replay through `self.dispatcher`, not through
`OperationEngine::execute_operation`, so they never meet this refusal. The
`FIELD_NAMING_PARAMS` list was verified complete by enumerating every declared
`OperationParam` name, not by reading the ops that looked relevant.

## Missing piece

COVERAGE, with an ORACLE blind spot behind it — and the second is why the first
could never have been caught by accident.

**COVERAGE.** Nothing in the catalog ever authored a reserved key: every
generated `create`/`update` builds params from ordinary block fields, so no case
could reach the state where the engine's insert had something to overwrite.

**ORACLE (secondary).** `crates/holon-pbt-core/src/block_compare.rs:40,58` lists
`PROVENANCE_PROPERTY` in the properties stripped before a reference block is
compared against a store-projected one. The exclusion is correct for its own
purpose — the stamp is system-authored metadata that cannot round-trip through
org — but it means that even if a case HAD authored the key, the keystone
comparator could not have seen the substitution. Closing only the coverage gap
would therefore not have produced a red. That is the same shape as
[2026-08-22-sql-authority-org-ingest-loses-fold-state](2026-08-22-sql-authority-org-ingest-loses-fold-state.md):
an invariant that runs, reports success, and never examines the field in
question.

The deeper missing piece is a DECLARATION: no profile said which property keys
the engine owns, so there was nothing for a certifier to drive. A rule that
lives only in an `insert` statement is a rule no harness can range over — and,
as the `set_field` route showed, a rule enforced at one call site says nothing
about the others.

## Remedy

FIXED. Three parts:

1. **A route-agnostic refusal, at one chokepoint.**
   `ENGINE_OWNED_PARAM_KEYS` (`crates/holon-api/src/provenance.rs`) names the
   keys the engine mints — exactly `_provenance` today.
   `reject_engine_owned_keys` (`crates/holon/src/api/operation_engine.rs`)
   refuses ANY operation that would write one, and `authored_property_keys`
   reads all three places a route can name a key: every param KEY, the VALUE of
   every `FIELD_NAMING_PARAMS` pair (`("field", "value")`), and the TOP-LEVEL
   keys of a property BAG — both when the bag is handed over as a param and
   when a field-naming param names the overflow column.

   Which param carries a bag is derived from the SCHEMA
   (`WriteSchema::OVERFLOW_COLUMN`), not from a list of operations, so a fourth
   route cannot be opened by inventing another op name. Deliberately NOT an op
   allowlist: the first fix scoped the check to `create`/`update` and that is
   exactly what let `set_field` through. A bag is read in both encodings that
   reach this boundary (decoded `Object` and JSON string), and a bag whose
   shape cannot be read is REFUSED rather than trusted unread — "I could not
   tell whether this carries a reserved key" is not evidence that it does not.

   The check fires at the top of
   `DispatchingOperationEngine::execute_operation`, above both write providers
   and ahead of the trust gate (a sub-threshold op is captured into a proposal
   VERBATIM, so a later refusal would store the reserved key and only reject it
   at accept time). The error names the key and the operation.

2. **EXACT spellings, not the `_` prefix.** `_drawer_order` is an authored
   carrier the org ingest path legitimately puts into create params
   (`crates/holon-orgmode/src/block_params.rs:167`), and `_proposed_by` is
   re-dispatched through the engine's own boundary when a proposal is promoted.
   A prefix ban would refuse the vault's own write leg and `accept_proposal`.
   Pinned on both routes by
   `other_underscored_keys_still_pass_the_boundary{,_on_both_routes}`.

3. **The declaration is certified on EVERY route, not asserted.**
   `property_keys` gained `engine_owned_keys`, and the
   `property_keys_engine_owned_keys` clause is member-covered. A declaration
   names a LEG, so the certifier drives every authored route into it:
   `CertifiableFormat::extra_property_write_routes` reports the extra routes and
   a finding names which one failed (`write route: create` /
   `write route: set_field` / `write route: set_field(properties bag)`).
   Measured: declaring a bogus `Plain` produces THREE violations, one per
   route. Adding a key to the yaml without implementing its
   refusal on both routes now goes red.

   One honesty note recorded at the probe: an ordinary key written through
   `set_field` reads back `Absent` in the certification wiring, because
   `set_field` offers the write to the `BlockCellRegistry` first
   (`sql_block_operations.rs:1052`) and no outbound projector runs there. So
   that route gets no round-trip control, and what it certifies is the ENGINE
   boundary rather than this wiring's storage leg. The probe is still sound: a
   dead route answers `Absent`, and only a real refusal can answer `Refused`
   with the key and the operation named — silence cannot be mistaken for
   enforcement.

The org and logseq-db profiles declare the clause honestly rather than copying
native's value: org's own `_`-key handling is a render-time STRIP already stated
by `reserved_prefixes: ["_"]`, which is a different fact and is left untouched.

## Adjacent hazards — recorded, deliberately NOT fixed here

Found while tracing the write path. Neither is a defect today and neither is in
scope for this entry's fix; both are the kind of thing that becomes one later.

- **A duplicated overflow-column literal.** `authored_property_keys` derives the
  bag param from `WriteSchema::OVERFLOW_COLUMN`, but `partition_params`
  (`crates/holon/src/core/sql_operation_provider.rs:503`) still compares against
  a hard-coded `"properties"` string. They agree today. If the const ever moves,
  the refusal follows it and the partition does not — and the two would then
  disagree about which param is the bag, which is exactly the gap route 3 came
  through. Left alone on purpose: a same-night refactor of the write path was
  out of scope for a fix round.

- **A hydrate/write asymmetry.** The write boundary REFUSES a properties blob it
  cannot read (see the Remedy). The hydrate path does the opposite:
  `crates/holon-turso/src/turso.rs:1548` maps an unreadable blob to an EMPTY
  object. Both choices are defensible in isolation — refusing to write unread
  data, versus not failing a read on one bad row — but a blob that is silently
  empty on the way in and refused on the way out is a disagreement about what
  "unreadable" means, and worth deciding deliberately rather than by accident.
