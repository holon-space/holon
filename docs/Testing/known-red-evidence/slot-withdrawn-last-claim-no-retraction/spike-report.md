# Spike #351 — do locally resolved guarded slots converge after sync?

Card: holon-space/holon-work#351. Concept: #345.
Workspace: /Users/martin/Workspaces/pkm/holon/.claude/worktrees/lane-351-slot-converge
Commit: `owqwlnlnxonx a95d013a8f6e`, base `kxnlxrlrwozp d568744cfb07`.
Test only. No production code changed; `set_function` was never modified.

Two real `holon_net` executors (one per device) run one `Resolve<K>` transition
over two real `loro` documents. The contestant set lives IN the document and
syncs; each device projects the set out of its OWN synced document, feeds it to
its OWN executor, and writes the decision back to its own document. No decision
crosses the wire, only the document does.

Seed and case count, printed in every proptest log line:
`cases=64 rng=ChaCha seed=holon-351-slot-converge-spike-01`.

## Headline answer

The riskiest assumption of #345 — "two devices that resolve locally converge
after sync" — holds for every CONTESTED case and fails for one UNCONTESTED one:
a withdrawn last claim. The preference function is not implicated. What fails is
the completeness of "the winner is a pure function of the contestant SET".

Ruled since, as `345/total-resolve.a`: `Resolve<K>` gets a total domain. This
spike does not implement it; #345 does.

## Verdict per open question

### "Open, to be grounded by the spike #351" (#345, Dialog round 2)

| # | Question | Verdict | Pin |
|---|---|---|---|
| 1 | Do concurrent tree moves of one node to one new parent converge, fractional position included? | YES | `b_concurrent_moves_of_one_child_to_one_parent_converge_with_the_position` |
| 2 | Does delete + undo really give a new op id, justifying the stored stamp? | YES — the stored stamp is justified | `c_delete_then_undo_tells_whether_the_op_id_is_new` |
| 3 | Are block ids random 128-bit values, never content-derived? | YES | `d_block_ids_are_random_never_content_derived` |
| 4 | Edge cases | mostly YES, see below | per test |

Detail on (1): the child ends as ONE node under ONE parent at ONE position both
devices agree on, so "children merge by MOVE, never by copy" is safe, and main's
refinement stands: the read-side join the Fable review proposed is not needed.

Detail on (2): the undo re-creates the node with TreeID counter 4 where the
original was 0. A preference term read off the CURRENT op id would flip on an
undo, exactly as #345 reasoned. Also green: a delete on one device undone on the
OTHER restores exactly one carrier
(`c_delete_on_one_device_undone_on_the_other_restores_one_carrier`).

Detail on (3): 4096 mints of `MintedId::random()`, no repeat, every one UUID v4,
and two mints for one conceptual block disagree — so an edit can never change an
id, and two entities share an id only by being copied.

### Edge cases from the card

| Case | Verdict | Test |
|---|---|---|
| Double ingest of one file | MERGE, no loser | `a_double_ingest_of_one_file_merges_with_no_loser` |
| Copied file, same id, different entity | PICK + deterministic re-mint, identical on both devices | `a_copied_file_with_the_same_id_is_picked_and_re_minted_alike` |
| Offline device returns with an OLDER claim | no edit lost either way | `an_offline_device_returning_with_an_older_claim_loses_no_edit` |
| Delete + undo with no swap | slot does not move | `a_delete_and_undo_with_no_swap` |
| No swapping back and forth after convergence | holds | history oracle in `assert_shared_invariants` |
| Clock skew (HLC) | the stored HLC is never ranked | `the_stored_hlc_is_not_ranked` |
| Each field class | PARTIAL — see "Known gaps" | `merge_into` + `EditKind` |

`the_stored_hlc_is_not_ranked` is metamorphic: it inverts every clock and
asserts the outcome is unchanged. It compares the resolver with itself under a
transformation, never with an expected value.

## Run 1 — equal policies

### Set-function rule: RED on one case, the finding

Minimal case, one device, one claim, no contest:

```
[Claim { device: true, key: 0, origin: 0, hlc: 0 },
 Resolve { device: true },
 Withdraw { device: true, key: 0, nth: 0 }]
```

left `Some({"merge": Boolean(false), "set_hash": String("ed2c5257b017dbee"),
"winner": String("00000000000000000001:0000000000")})`, right `None`.

A resolves the key, then withdraws the only carrier. The slot is never
retracted:

- `keys_in_play` (slot_convergence_pbt.rs:708) enumerates only keys a LIVE node
  claims, so once the carrier is deleted the key is never revisited.
- `resolve_to_fixpoint` additionally skips the empty set outright.

B first saw the key already deleted, so it never resolved it and holds no slot.
The DOCUMENTS converge — equal write-back bytes, both devices agree there is no
carrier — while the net's output places diverge for good.

What this refutes: not the preference function (there is nothing to prefer in a
one-claim case), but the COMPLETENESS of "a pure function of the contestant
SET". A pure function of the set must be TOTAL over sets, and the empty set is
in its domain. A derived place that is never retracted is a function of the
local HISTORY of inputs, not of the input — and that is precisely the property
two devices with different histories cannot share.

Pin: `a_withdrawn_last_claim_leaves_a_slot_no_device_can_retract`. It reproduces
the three steps deterministically and asserts the documents converge, so the
divergence cannot be blamed on Loro. Its failure message says to delete the test
once #345 grows its retraction rule.

### Naive first-arrival rule: RED for the right reason, as required

The negative control. Shrunk counterexample, verbatim:

```
[Claim { device: false, key: 1, origin: 0, hlc: 0 },
 Resolve { device: false },
 Claim { device: true, key: 1, origin: 0, hlc: 0 }]
```

B claims `inbox` and resolves it, then A claims it. B has seen its own stamp
first; A sees both at once and takes the lower. Winners: peer 2 on B, peer 1 on
A. Slot hashes `dc538ce2775b756b` vs `ed2c5257b017dbee`.

The naive rule's only input is the device's LOCAL arrival order, which no sync
equalizes, so it cannot be a function of the contestant set. A run that found NO
counterexample would itself fail the test, so the control has teeth. The naive
variant stays in the file as the negative control.

## Run 2 — policy skew (device B holds one extra well-known-id row)

GREEN on everything the card asks of it: no edit lost, no id live twice, equal
write-back bytes. The slot is not compared, by design.

Skew behaviour, as an observation: the contestant SET converges while the
RANKING of it does not, so each device prefers the other's loser and re-mints
the other's winner. After sync BOTH contestants carry re-minted ids and the key
ends with NO carrier at all. Convergent and symmetric — but a policy difference
does not merely pick a different winner, it can VACATE the slot. Worth a #345
line: the load-time check should probably treat a key whose policy differs
across devices as a refusal, not a divergence to tolerate.

## Second finding — lazy CRDT containers lose data silently

Main ranked this above the retraction finding, because it can lose authored data
in ordinary sync with no contest at all.

A sub-container reached by "get the field, or create it when absent" is NOT a
convergent union. Two devices writing their FIRST tag on one synced node each
call `insert_container("tags", …)`. That is a write of the map KEY, so the two
containers meet under last-write-wins: one survives whole and the other's
entries are unreachable on BOTH devices. The tag is not merged, it is gone.

Consequence for #345: "Sets (tags): union" and "Logs (LOGBOOK): append" converge
only when the container is created ONCE, by the op that creates the node. The
harness now creates `tags` and `logbook` at claim time; a lazily created one
silently drops a concurrent first write.

Pin: `e_a_lazily_created_sub_container_loses_a_concurrent_first_write`
(crates/holon-loro/tests/slot_loro_mechanics.rs). It asserts both that the
devices AGREE which container won and that only one tag survives.

### Where the constraint must be stated (no doc edited in this spike)

Normative home — `docs/Architecture/Replication.md`, heading quoted verbatim:

> `## 9. Invariants (the things that must not be violated)`

That is the canonical list: `docs/Architecture/Model.md` § `## Invariants` opens
with "(1)-(7) are [Replication §9](Replication.md); the rest extend them". Its
seven entries cover bases, order ownership, intent vocabulary, one-writer,
sinks-never-re-merge, causality and the `LoroStore`/`TextMerge` split. None
covers container CREATION, so this is a gap, not a restatement.

Explanatory home — `docs/Architecture/Model.md`, heading quoted verbatim:

> `## Loro is three capabilities, not one`

Its closing line is what the constraint qualifies: "The degradation ladder for
any field is: op-fidelity (store) → base-limited 3-way (transient) → LWW." A
lazily created container does not land on the bottom rung. LWW on a field's
VALUE keeps the field; LWW on the map KEY discards the whole container, so the
loss is the entire tag set or logbook, not one value. As written, the ladder
invites the conclusion that LWW-per-field is the floor, which is false for a
container-valued field.

One more line worth adding — `docs/Architecture/Sync.md` §
`### Loro CRDT Integration Overview` advertises "Automatic Conflict Resolution:
Concurrent edits ... merge deterministically without manual intervention" and
"Strong Eventual Consistency". Both stay TRUE under this constraint: the devices
converge and agree which container won. That is exactly what makes the loss
silent. Convergence is not preservation.

## Third finding — a completed merge erases its own record (NOT ruled)

The `merge` outcome is a transient of the resolution sequence, not a durable
field of the slot. A merge shrinks the contestant set from 2 to 1, which re-fires
`Resolve` on the single survivor, and THAT firing overwrites `Slot` with
`merge = false` and clears the condition. So a completed merge leaves no record
anywhere in the net's places.

This is the retraction finding seen from the other side: one question — what is
the net's output a function of? — with two symptoms. If #345 wants a merge
disclosed, it needs a durable appended fact, not a slot field.

Not ruled. Main takes merge disclosure to Martin as a concept question.

In the test the decision is asserted where it survives: `Device::merges` records
that both devices applied the merge, and the test also asserts the follow-up
firing reports no contest. The rule was not changed to make this pass.

## Gate time — the brief's ~30 s question

No `#[ignore]` variant and no `just` recipe are needed.

- Whole spike file: `test result: FAILED. 8 passed; 1 failed; … finished in 0.69s`
- Slowest single property: 0.34s
- Step 0 mechanics, 6 tests: `finished in 0.03s`
- Per-`cargo test` wall time 1.6-2.2s (`/usr/bin/time -p`), nearly all of it
  cargo's own startup, not the properties.

## Known gaps

- Field classes partially oracled. Children-by-move, tags-union and
  LOGBOOK-append are asserted by the ledger and the every-id-exactly-once
  oracle. Plain-field LWW, the task-state three-way check and the retained-text
  rule are implemented in `merge_into` and exercised by the `EditKind`
  generator, but no oracle asserts their outcome specifically; a divergence in
  them would surface only through the equal-write-back-bytes oracle.
- Two devices only, as the card scopes it. A three-way contest is unmeasured.
- Lane checks RUN and the evidence logs re-taken at the final tree
  `78b6b757078c` (2026-10-19): target/slot-converge-lane-checks.log — fmt,
  clippy, `gate-compile`, `gate-arch`, `arch-validate`, `featuremap.py check`
  and `analyze-arch` all pass. `just known-reds-fixture` is red at the lane's
  BASE too (A/B in this workspace with the base registry restored, same case
  `outcome/move-between-files-read-repeat`), so it is not this lane's.
- The package nextest ran for the first time (it could not while the finding
  stood as a plain red): target/slot-converge-nextest.log,
  `1602 tests run: 1578 passed, 16 failed, 8 timed out`. All 15 of the spike's
  tests PASS and no failure is in a file this lane touches; the classification
  of the 24 non-green is in target/slot-converge-nextest-classified.log and in
  target/slot-converge-summary.md.
- How the deliberate red lands is RULED (main, 2026-10-19): the property is a
  SELF-CHECKING known red. `the_set_function_rule_converges_on_both_devices`
  asserts the failure and its named reason (the empty contestant set), names
  #345, and fails with "remove the known-red marker, #345 fixed it" the moment
  the property holds. Registered as `slot-withdrawn-last-claim-no-retraction`
  in docs/Testing/KeystoneKnownReds.md. Not `#[ignore]`.

## Oracle independence

No oracle calls the resolver. They compare the two devices with each other,
compare a state with itself across an edit-only step, and check absolute
invariants: every id live exactly once, every authored child/tag/log still
present, equal write-back bytes, `Claims` empty at quiescence, one condition per
(key, contestant set) via a set hash shared with the slot, and no winner
oscillation once the set stops changing.

Two oracle defects were found and fixed during the spike, both in the harness:
the ledger demanded tags from a deliberately withdrawn carrier (now keyed by the
carrier's immutable stamp, since a PICK re-mints the id), and the step 0 (c)
measurement was written as an assertion of the outcome it was supposed to
measure.
