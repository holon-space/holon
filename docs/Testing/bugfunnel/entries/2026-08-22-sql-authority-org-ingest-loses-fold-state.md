---
id: 2026-08-22-sql-authority-org-ingest-loses-fold-state
date: 2026-08-22
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  Under the shipped default Sql authority, ingesting an org file that carries
  `:COLLAPSED: t` into a vault that already holds documents lands the block
  unfolded in both block_raw and the matview — a real import-time data loss,
  distinct from the two Loro-path drops fixed alongside it.
---

## Bug

Un-`@wip`ing the `logseq-parity` log:4 ingest scenario on the train reds:

```
authority: block-CRUD=Sql(SqlOperationProvider); projection-sinks=Sql(block_raw,matview); org-writeback=on
inv-blocks-match-ref/matview: 14 blocks, reference: 14 blocks
  field deltas (1):
    block:folded-parent: collapsed: sut=false ref=true
```

Found in lane `collapsed-bug` while un-`@wip`ing that scenario as the
corpus-level proof of the two Loro-path drops recorded in
[2026-08-22-org-ingest-drops-collapsed-into-property-bag](2026-08-22-org-ingest-drops-collapsed-into-property-bag.md)
and
[2026-08-22-loro-create-projection-drops-fold-state](2026-08-22-loro-create-projection-drops-fold-state.md).
The scenario executed and red — on a DIFFERENT authority than either fix
addresses. Evidence: `lane-logs/probe3.log`, `lane-logs/probe5.log`.

## Root cause

NOT LOCALIZED to a line. It IS localized to a seam, and five hypotheses are
dead by measurement — recorded so the next lane starts from the survivor rather
than re-running them.

**Measured facts, in the order they were established.**

1. Cold-boot ingest is CORRECT. Booting from a `:COLLAPSED:` file gives
   `block_raw.collapsed=1` and `matview.collapsed=1` (verifier's discriminator;
   SCOPE: ingest-then-immediate-read, 3-block vault, no later transitions).
2. The docstring→blocks→org-text round trip is CLEAN. `WriteOrgFile::parse_step`
   yields `collapsed=true` on the parsed block and `render_step` re-emits
   `:COLLAPSED: t`, so the SUT is handed a file that genuinely carries the
   marker.
3. Live-watcher ingest is ALSO correct — in the frontend_slice harness. Boot
   empty, then `SutFixtureFs::write_org_file`, read at first appearance AND after
   a settle: `block_raw=Some(Integer(1))`, `matview=Some(Integer(1))` at both
   times (`lane-logs/probe5.log`, ARM A and ARM B).
4. In the COMPOSED harness the same scenario gives, read as TYPED snapshots at
   the moment the invariant fires: `block_raw.collapsed=Some(false)` and
   `matview.collapsed=Some(false)`. **Both stores, not just the matview.**
5. Per-TICK timeline in the composed harness: the block's FIRST appearance
   already reads `block_raw=Some(false) matview=Some(false)`. It is never true
   and later cleared — it is BORN unfolded.

6. The composed harness DOES write a real file, through the production path,
   and the bytes CARRY the marker. Instrumenting `write_org_file` itself:

   ```
   PROBE@write_org_file impl=HeadlessFrontendComponent file=Folded.org has_COLLAPSED=true
   content=<<<#+ID: ref-doc-0
   * Folded parent
   :PROPERTIES:
   :ID: folded-parent
   :COLLAPSED: t
   :END:
   …>>>
   ```

**CLASS: PRODUCTION, not test infrastructure.** Fact 6 settles it. There is
exactly ONE `SutFixtureFs` implementation in the tree
(`frontend_slice/components.rs:3805`), the composed CapMap resolves to it (the
probe fired from inside it), `WriteOrgFile`'s `cap_transition!` body has no
alternative materialisation route, and the file that lands on disk carries
`:COLLAPSED: t`. So the real `FileSyncController` ingests a correct file through
the production path and still produces `collapsed = false`. The harness is not
faking the write; the ingest is genuinely losing the field under this
configuration.

**Leading hypothesis for WHY this configuration and not the probes' — the file
is an UPDATE to an EXISTING document, not a create.** The written header is
`#+ID: ref-doc-0`, and `ref-doc-0` is already one of the composed harness's 14
seeded blocks. Every green probe wrote a file for a NEW document. So the
composed run plausibly takes `FileSyncController`'s diff-against-previous-parse
arm (`build_block_params(block, …, Some(previous))` and the `old_blocks`
branches) rather than the create arm — and the create arm is exactly what
`BlockCreateRequest::of` fixed for the sibling entry. UNMEASURED: no probe has
yet confirmed which arm runs, and a second candidate is live — the rendered
drawer emits `:ID:` BEFORE `:COLLAPSED:`, whereas every green probe's fixture had
`:COLLAPSED:` first, so a position-dependent parse or `_drawer_order` replay is
not excluded. Both are one instrumented run away; neither should be asserted
before that run.

**A trap this bug sets, which cost this lane two wrong turns.** The failure names
only `inv-blocks-match-ref/matview`, inviting "so block_raw is fine". It is not:
`compare_block_raw_subset` (`holon-turso-testing/src/correspondences.rs:187-199`)
compares only `{Content, Properties, Marks}`, so the `block_raw` arm CANNOT fire
on `collapsed` whatever the row holds. Read the field directly, as a typed
snapshot, before concluding anything about that arm's silence.

**Dead hypotheses** (all refuted by measurement, do not re-run): the ingest seam;
"block_raw green, matview stale"; the render round trip; cold-boot vs
live-watcher as the discriminator; a post-ingest clear.

**Product-vs-harness is SETTLED (fact 6): PRODUCT.** An earlier revision of this
entry left it open; the `write_org_file` instrumentation closed it. This is
user-visible data loss on the shipped default wiring — importing a folded block
into a vault that already has documents silently unfolds it.

## Missing piece

COVERAGE. Applying the litmus questions:

1. "If a case had hit this state, would any invariant have gone red?" YES —
   `inv-blocks-match-ref/matview` caught it on the scenario's FIRST execution. So
   this is NOT an ORACLE gap, and `secondary` is null rather than a reflexive
   second label.
2. "Is there a transition sequence in the current catalog+wiring that reaches
   this state?" YES — the parity corpus carries exactly that scenario, under the
   DEFAULT wiring, and it reaches the state on the first try.

Both yes makes this a latent red rather than a true generation gap: the sequence
existed and the oracle worked, and the only thing between them was the `@wip`
tag deselecting the scenario. Filed COVERAGE because a deselected scenario is
operationally a sequence the suite cannot generate — but the remedy is not to
write new coverage, it is to stop hiding the coverage that exists.

The tag was applied for a legitimate reason (the scenario found a real bug and
could not be left red), which is exactly how a second bug hid behind the first.
The lesson generalises: a scenario parked `@wip` "because it found a bug" must be
un-`@wip`ed as part of that bug's fix gate, so the tag cannot outlive its
justification. So must the gate itself — the fix that preceded this entry gated
only the wiring where its bug reproduced, never the shipped default, which is
why this drop survived a CONFIRMED verification.

## Remedy

OPEN. Next job for lane `collapsed-bug`.

The red needs no construction: un-`@wip` the log:4 ingest scenario in
`logseq-parity/outliner.feature` and the parity replay reds on this divergence
under the default wiring.

Start by discriminating the two live hypotheses, in this order — each is one
instrumented run, and the first is far cheaper to test:

1. UPDATE-vs-CREATE arm. Write the same fixture twice into a
   frontend_slice-style probe: once as a NEW document (known green) and once
   under a `#+ID:` that already exists in the vault. If only the second reds, the
   seam is `FileSyncController`'s diff-against-previous-parse arm, and the fix
   is the update-path analogue of `BlockCreateRequest::of`.
2. Drawer POSITION. Same probe, one fixture with `:COLLAPSED:` before `:ID:` and
   one after. If only the second reds, the seam is a position-dependent parse or
   `_drawer_order` replay.

Then fix, then un-`@wip` for real — at which point the scenario becomes the
standing gate for all three drops at once. Note for whoever picks this up: five
path hypotheses are already dead above, and this bug has been mis-attributed
four times by reasoning rather than measurement. Probe first.
