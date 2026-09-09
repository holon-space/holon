# D109 — can the Petri net order birth before the first keystroke's write?

Read-only assessment at chain tip `8c8c564d5890` (workspace `.claude/worktrees/hygiene`). No builds, no VCS writes.

## 1. The fundamental requirement

One user gesture (a keystroke on a caret seated in an empty destination) expands into two effects — *the block comes into existence* and *the block's content becomes "z"*. The requirement is a **happens-before**, not a merge: the second effect names an entity the first effect brings into existence, so no merge algebra can repair the order. On the CRDT leg it is not even a lost update, it is a hard `Err`: `LoroBlockOperations::set_field` reads prior state first (`crates/holon-loro/src/loro_block_operations.rs:457`, `set_field('{field}'): capture prior state`) and the read fails with `Block not found`.

What a Petri net offers for exactly this: a **place is a precondition**. If "block *id* exists" is a token in an existence place, and the write transition has that place as an input arc, the write is *not enabled* until the birth transition has produced the token. Ordering stops being a scheduling accident and becomes enabledness — the property the net is built to reason about.

## 2. Today's flow in PN terms

- **Birth is already a declared transition.** `crates/holon-frontend/src/creation_slot.rs:23` defines `BIRTH_TRANSITION_ID = "creation-slot-birth"`, and `ReactiveEngine::birth_creation_affordance` fires it as `OpOrigin::Rule{transition_id}` (`crates/holon-frontend/src/reactive.rs:3039-3046`). But it is fired **fire-and-forget**: `self.runtime_handle.spawn(async move { … })`, and the function returns the minted id synchronously (`reactive.rs:3048`).
- **The keystroke's write goes through the same dispatcher.** `ViewEventHandler::edit_target_id` (`crates/holon-frontend/src/view_event_handler.rs:128-146`) resolves the affordance to the newborn through `caret_block_for_edit`, and the write is dispatched as an ordinary `block.set_field`. Both effects therefore reach `OperationDispatcher::execute_operation_with_provenance`, but as **two independent tokio tasks with no edge between them**.
- **The arcs exist as declarations, and they are the wrong shape.** `crates/holon-core/src/traits.rs:635` declares `create` as `block(structural = produces, text = produces, existence = produces)`. `set_field` at `traits.rs:619-622` declares `block(structural = relocates, text = produces, existence = untouched), varies_by("field")`. **`existence = untouched`, not `reads`** — the precondition this bug violates is not declared anywhere, so no oracle and no guard can see it.
- **There is a seat for the check, and it cannot wait.** The net gate is wired: `enforce_net_guard` runs before the provider (`crates/holon/src/api/operation_dispatcher.rs:1128-1131`), and `NetVerdict` today has exactly two arms, `Confirm` and `Refuse` (`operation_dispatcher.rs:502-509`). ADR 0032 §3 reserves the third, **arbitrate**, as an explicitly deferred item (§5, "Deferred — open, not decided").
- **No marking is durable.** `crates/holon-petri` is the WSJF task-ranking materializer, not the action executor. There is no in-flight/pending-existence register anywhere, so "birth completed" is **not observable as a marking** today.

## 3. The PN-native design

```
        birth-T                         write-T (set_field content)
  [caret on affordance]            [exists(id)] ──┐
        │  consume                                ├─► fires
        ▼                           [text(id)] ───┘  (text token is CRDT-shared,
  [in-flight(id)]  ── begin-T residue, ADR 0032 §5    read, never consumed)
        │  end-T commit
        ▼
  [exists(id)]  ← produced by birth-T, READ by write-T
```

Three changes, all small and all declarative:

1. `set_field` (and `delete`, `move_block`) declare `existence = reads` on `block`. That makes "the entity exists" a stated input arc rather than folklore, and the `varies_by("field")` envelope already carries the per-field granularity.
2. `birth-T` registers `in-flight(id)` **synchronously, at mint time**, before it returns the id — the `begin-T` residue ADR 0032 §5 already reserves. The spawned create is `end-T commit`.
3. The dispatcher, at the net gate, resolves an input arc on `exists(id)` by **awaiting `in-flight(id)`** when the token is pending, and refusing when it is absent outright. That await is the `arbitrate` slot doing its first real job.

What stays: the frontend keeps minting the id and seating the caret synchronously (ADR 0029/D97.a), and `edit_target_id` stays sync. What it generalises to: **every "act on an entity another transition is creating"** case — paste-then-format, template expansion, a rule that creates and immediately tags, sync import followed by a local edit.

**The honest limit.** ADR 0032 §3 says authorization-relevant predicates read the store, never the derived projection, because the projection lags. Existence is exactly such a predicate: the net guard cannot answer "does this newborn exist on the Loro leg yet" from a lagging projection without failing open. So the token that unblocks the write must come from the **write leg's own completion**, not from a projection. That is why the PN framing does not, by itself, remove the need for a readiness handle — it names it and gives it a home.

## 4. Comparison with the D109 options

| | (a) per-entity FIFO in dispatcher | (b) readiness oneshot | (c) sync create |
|---|---|---|---|
| Correctness, Loro leg | **Does not fix it.** A FIFO orders what has *arrived*; the create is spawned, so the set_field can enter the queue first. No happens-before is created at mint. | Correct: the handle is registered synchronously at mint, before the id escapes. | Correct. |
| Correctness, SQL leg | same defect | correct | correct |
| Latency vs 200 ms SLO | µs when it works | one await on a create already in flight, typically sub-ms; worst case one store round trip | **worst**: the caret seat blocks on a store round trip |
| Blast radius | `crates/holon/src/api/operation_dispatcher.rs` | dispatcher register + `reactive.rs:2985-3048` | `reactive.rs`, plus `edit_target_id` is **sync** (`view_event_handler.rs:128`), so (c) needs a `block_on` on the runtime that spawns the create — deadlock-shaped |
| Generality | any op pair on one id | any op naming a pending id, every leg, every caller (MCP, rules, sync) | birth only |

**Bypass caveat that cuts across all three:** ADR 0032 §3 declares text edits bypass the dispatcher through the Loro CRDT cell, and `view_event_handler.rs:100-112`/`handle_text_sync` confirm the per-keystroke `MutableText` writer replaces the `set_field` when a content cell is attached. A dispatcher-only fix therefore does not cover the windowed per-keystroke path. The lane report already states the windowed rung is green with the **mechanism undiagnosed** (`.claude/worktrees/quick-open-caret/lane-report-quick-open-caret.md:579-586`); that gap stays open under every option.

**Red-first pin.** Un-park the empty-destination replay already authored in `hand-authored-regressions/keystone.jsonl`. The transition is `TypeChars` routed through `birth_block_via_creation_slot` with the caret on a `:__virtual:` affordance (`crates/holon-integration-tests/src/pbt/transitions/type_chars.rs`); the invariant is that a `set_field` naming a newborn never returns `Block not found`. It is red today on the `["Loro","Turso"]` wiring (`lane-logs/a5-ha-empty-3.log:295`) — red for the right reason, no new test scaffolding needed.

## 5. Recommendation

**Take (b), but build it in the dispatcher as an in-flight-births register, and declare `set_field`'s `existence = reads` in the same change.** That is (b)'s mechanism with the PN's semantics: the register *is* the `in-flight(id)` place, the synchronous registration *is* `begin-T`, and the declared arc is what lets a future oracle and the net guard's `arbitrate` arm take the check over without a rewrite.

**The decisive tradeoff:** a Petri net gives you the *declaration* and the *seat* for this check, but classical PN enabledness means **refuse when not enabled**, not **wait until enabled**. Waiting is scheduling, and ADR 0032 §5 deliberately defers the scheduler. Committing to a full net-guard resolution now means building that scheduler for one race; committing to (b) alone means the ordering fact stays undeclared and the next such race is found the same way. Doing (b) *shaped as* the in-flight place buys both at (b)'s cost.

**The one thing Martin must decide:** does the readiness gate live in `OperationDispatcher` as a general in-flight-births register with `set_field` re-declared `existence = reads` (a fleet-wide declaration change, and a small ADR 0032 amendment), or does it stay a local frontend handle inside `ReactiveEngine::birth_creation_affordance` with no declaration change?

**What I could not determine from reading:** (i) why the windowed GPUI rung is green — the per-keystroke Loro cell path plausibly writes without a prior-state read, but I did not trace it and the lane's own claim was retracted; (ii) whether re-declaring `existence = reads` on `set_field` reds `assert_declared_arcs_match_schema` (`operation_dispatcher.rs:622`) or the marking-delta oracles — that needs a build, which this assessment did not run.

---

# Round 2 — the net's ordinary mechanism

Martin's objection lands. Round 1's dispatcher register **is** an ad-hoc marking held outside the net, and I reached for it because I mistook "the request is a token" for "queue the firing". They are not the same thing, and the ADR itself draws the line.

## 1. What actually prevents requests from being tokens today

**Nothing in ADR 0032 forbids it. Three concrete things in the code do.**

- **Enabledness is evaluated once, at call time, and the request is dropped on refusal.** `enforce_net_guard` is a straight-line call before the provider (`crates/holon/src/api/operation_dispatcher.rs:1128-1131`); `NetVerdict` has two arms, and `Refuse` becomes `Err(…)` returned to the caller (`operation_dispatcher.rs:502-509`). The firing request lives only in the call frame. When it returns, it is gone.
- **There is no marking persistence between calls.** No places table, no marking table, no projection: ADR 0032's Consequences say the first increment is "**Declared marking deltas** … Data only … **Nothing consumes the declarations yet**". Grepping confirms it — the only hits for the derived projection are a doc comment in `crates/holon-api/tests/descriptor_marking_delta_roundtrip.rs:4`.
- **A re-evaluation trigger exists, but only for rule transitions with a clock subject.** `crates/holon/src/api/holon_rule_watcher.rs:75-93` subscribes with `query_and_watch` and re-fires on every change; `fire_emit` (`:320`) evaluates its inhibitor arc as **a direct read, never a matview** (`:345`). Its module docs state the constraint that shapes everything below: the reactive `block` relation is itself a materialized view, and **a matview reading another matview hangs in Turso IVM**, so a block-subject rule's reactive form has no live binding today (`holon_rule_watcher.rs:11-37`, `:232-242`).

**The sentence Martin should read as the deferral.** It is not deferred item 5; it is §6's firing axiom:

> The marking decides. Occurrences trigger. Deterministic identity dedupes.
> A blocked rule **re-evaluates when the token is released**. It never queues a firing to replay later — a queued firing carries a stale marking, and replaying it fires against a world that has moved on.

Read precisely, this **licenses** Martin's design and forbids mine. A request sitting in a place is re-evaluated against the *live* marking at fire time, so it carries no stale marking; a dispatcher register that holds a pending call and replays it later is exactly the queued firing the axiom rejects. §1's table already names the missing half: intent transitions are "marking-guarded, **plus an explicit firing request**" — the request is a first-class concept in the ADR that has no representation in the store.

**The one real constraint the ADR imposes** is §1, and it is the price of the whole design: "every token a transition produces, consumes, or relocates must **reduce to durable state**. Nothing else carries such marking." So `write-requested(id)` cannot be an in-memory channel. It must be a durable row. That is what makes the net-native answer genuinely larger than a register — and also what makes it survive restart, replicate, and appear in the occurrence journal.

## 2. Minimal implementation of the ordinary mechanism

```
  keystroke                              birth
     │ emit                                │ emit
     ▼                                     ▼
[write-requested(id, field, value)]   [create-requested(id, parent)]
     │                                     │  fires immediately
     │                                     ▼
     │                              [creating(id)]   ← begin-T residue
     │                                     │  async provider returns Ok
     │                                     ▼
     └──────────────┬───────────────► [exists(id)]
                    │ both tokens present
                    ▼
                 write-T fires → consumes write-requested, reads exists
```

- **New durable base table `pending_firing`** (id, transition, entity_id, params, state). It is a *base* table, not a matview, so `query_and_watch` over it is CDC-eligible with no chained matview — the same escape hatch the `clock` table gives the rule watcher (`holon_rule_watcher.rs:11-37`). This is the marking, and it satisfies §1's durability requirement.
- **`exists(id)` is evaluated as a direct read**, not as a join against the `block` matview — following the established inhibitor-arc precedent at `holon_rule_watcher.rs:345`, and using a mode-correct read so it answers on the Loro leg as well as Turso (`current_title_of` at `:426` is the existing example of exactly that).
- **A firing loop scoped to existence arcs.** One watcher over `pending_firing`, re-evaluating on every change to that table and on create completion. It dispatches through `execute_operation` like every other firing — ADR 0032 §1: "A firing is one `execute_operation` dispatch. There is no second execution path."
- **`birth_creation_affordance` stops spawning and forgetting** (`crates/holon-frontend/src/reactive.rs:3036-3048`). It mints the id, seats the caret, and **synchronously inserts `create-requested`**; the spawned dispatch becomes the loop's job. `edit_target_id` (`crates/holon-frontend/src/view_event_handler.rs:128`) stays sync and the keystroke's write becomes an insert of `write-requested` rather than a direct dispatch.
- **Async completion feeds back as a marking change**: the create's provider result flips `creating(id)` → `exists(id)` by writing the row that *is* the block. No callback, no oneshot — the existence token is the durable block itself, so the loop's next evaluation sees it.
- Declaration change unchanged from round 1: `set_field` must declare `existence = reads` (`crates/holon-core/src/traits.rs:619-622`), otherwise the write transition has no input arc on `exists` and the loop has nothing to gate on.

Files: `crates/holon/src/api/` (new watcher + the `pending_firing` schema), `crates/holon-core/src/traits.rs`, `crates/holon-frontend/src/reactive.rs`, plus the Turso migration.

## 3. Latency and failure

The token round trip is two local writes plus one watcher wake — the same shape the rule watcher already pays, comfortably inside the 200 ms interaction→projection-visible SLO, *provided* the loop is woken by CDC rather than polled. Polling would put a tick's latency on every keystroke and is the thing to measure before committing.

**On create failure the request token must not leak.** `create-requested` moves to a terminal refused state and the dependent `write-requested` is refused **loudly** with the create's error attached, never silently dropped and never left pending. This is strictly better than today: the current spawn logs through `surface_op_failure` and the keystroke's write fails separately with an unrelated `Block not found`, so the user sees the symptom and not the cause.

## 4. Red-first keystone PBT

Same replay as round 1 — the parked empty-destination case in `hand-authored-regressions/keystone.jsonl`, driven by `TypeChars` through `birth_block_via_creation_slot` (`crates/holon-integration-tests/src/pbt/transitions/type_chars.rs`) — but the invariant is now stateable in net terms and is stronger than "no error":

> For every id with a `create-requested` token, every `write-requested` on that id produces an occurrence **strictly after** the create's occurrence, exactly once, and never zero times.

Three failure modes, one invariant: fires early (today's `Block not found`), fires twice (the loop lacks deterministic identity dedupe), fires never (the token leaked). The occurrence journal (§7) is the observable that lets the model assert ordering rather than just absence of an `Err`.

## 5. Is the dispatcher register anything other than an ad-hoc marking?

**No.** It is a marking held in process memory, invisible to the projection, absent from the occurrence journal, lost on restart, and unavailable to any other replica or to MCP. It also violates §1's durability rule the moment you notice that a transition writes it. Round 1's framing — "shaped as the in-flight place" — was a way of saying it resembles the real thing; Martin is right that resembling it is not being it.

**What is genuinely larger, honestly:** the firing loop and its durable table. That is a new watcher, a new schema, a new terminal-state discipline, and a new source of duplicate-firing risk that needs deterministic identity to control. It is not a weekend's work, and it touches the write path for every keystroke.

**It scopes down cleanly.** Increment one covers **only transitions with a declared existence arc** — `set_field`, `delete`, `move_block` against an id with an in-flight `create`. Everything else keeps dispatching directly, exactly as today, because a transition with no existence arc has no place to wait on. That bounds the blast radius to the case the bug is in while building the real mechanism rather than a stand-in for it.

## 6. Revised recommendation

**Build the ordinary mechanism, scoped to existence arcs.** The register was the wrong instinct: it moves state out of the net to avoid building the net's one missing organ, and the organ is small when scoped. The three pieces — a durable `pending_firing` table, `existence = reads` on `set_field`, and a CDC-woken loop that fires when both tokens are present — are each independently useful and each land under the `holon-feature` red-first discipline.

**The decisive tradeoff:** the net-native design requires the firing request to become durable state (§1 leaves no other option), which means a schema, a watcher, and dedupe discipline on the keystroke path. The register avoids all three and works, but it is unobservable, un-analysable, and will be reinvented at the next such race — and it is precisely the "queued firing" §6's axiom rejects.

**The one thing Martin must decide:** whether D109 is allowed to grow from a race fix into ADR 0032's next increment — the durable firing-request marking and its loop — or whether that increment is scheduled separately and D109 ships an explicitly-labelled stopgap in the meantime. My recommendation is the former, scoped to existence arcs only.

**What I could not determine without a build:** whether `query_and_watch` over a new base table is genuinely free of the chained-matview wall in Loro mode (the rule watcher's escape is proven for `clock` and Turso), and what the CDC wake latency actually is on the keystroke path.
