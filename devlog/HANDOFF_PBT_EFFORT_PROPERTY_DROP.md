# Hand-off: PBT failure — `effort` custom property drops on external mutation

**Status:** Deterministic failure, not a flake. Unrelated to the
`deep-humming-crane.md` UserDriver refactor.

## TL;DR

`general_e2e_pbt` and `general_e2e_pbt_cross_executor` fail at
`crates/holon-integration-tests/src/assertions.rs:55` (inside
`assert_blocks_equivalent`) because, after an **external** mutation
writes a custom org property (e.g. `:effort: 7yzXz`) to an org file on
disk, the SUT's backend blocks come back missing that property while
the reference model expects it present.

The diff the assertion prints:

```
left  (SUT):      properties: {"task_state": String("WAITING")}
right (expected): properties: {"effort": String("7yzXz"), "task_state": String("WAITING")}
```

The `task_state` survives; the `effort` does not. Both are written in
the same `:PROPERTIES:` drawer of the same headline in the same file
write. Only "non-standard" custom properties drop.

`general_e2e_pbt_sql_only` passes — it uses a different code path that
doesn't go through org-file sync.

## Reproduction

```bash
PROPTEST_CASES=2 cargo nextest run -p holon-integration-tests \
  --test general_e2e_pbt 2>&1 | tee /tmp/pbt-run.log
```

Runs in ~150s. Both failing variants reproduce on the same shrunken
input:

```
Update { entity: "block", id: EntityUri("block:-rc-ik-yj-o860"),
         fields: {"effort": String("7yzXz")} }
```

Driven from the `External` mutation source via `apply_external_mutation`
(file write → org sync → Loro → Turso → SQL → block readback).

Full panic and diff already captured at `/tmp/pbt-run.log`. Grep for
`assertions.rs:55` and `7yzXz`.

## Evidence this is not the UserDriver refactor

1. The same failure triggers on both `general_e2e_pbt` and
   `general_e2e_pbt_cross_executor`, which don't go through
   `ReactiveEngineDriver::send_key_chord` for the failing transition —
   the shrunken case is `MutationSource::External`, routed through
   `apply_external_mutation` (SUT writes the org file, org-sync
   adapter reads it back). The UserDriver refactor sits on the **input**
   side of the UI dispatch pipeline; external mutations bypass it
   entirely.
2. Zero hits on `did not quiesce`, `did not advance`, `pre-root
   nested`, `shadow_index` panic, or `bubble_input` cycle in the log.
3. `general_e2e_pbt_sql_only` (same state-machine generator, no
   reactive engine, no org sync roundtrip) passes.
4. The failing blocks are well-known: this is the same
   `effort: "7yzXz"` failure called out by name in
   `.claude/plans/deep-humming-crane.md` v1 as a pre-existing flake to
   ignore during refactor validation.
5. `crates/holon-frontend/src/shadow_index::tests::patch_block_grow_double_shifts_entity_index`
   and all 5 sibling unit tests pass.

## Where the property is born

The PBT's `Mutation::Update` generator sometimes attaches a
`custom_prop` from this fixed list (at
`crates/holon-integration-tests/src/pbt/generators.rs:299`):

```rust
"effort", "story_points", "estimate", "reviewer",
"column-order", "collapse-to", "ideal-width", "column-priority",
```

For `create_text` and similar builders, the property is inserted into
the block's `fields` HashMap alongside `content`, `content_type`, etc.
The reference model stores them verbatim.

The SUT's `apply_external_mutation` renders the expected blocks to org
via `OrgRenderer` (the canonical writer) and writes the file. The
captured log shows the file content *is* correct:

```
:PROPERTIES:
:ID: -rc-ik-yj-o860
:effort: 7yzXz
:END:
```

The property reaches disk. The loss happens on the **read-back** side
of the sync roundtrip.

## Hypotheses, ranked

### H1 — Custom-property allowlist in the org parser/adapter (most likely)

Somewhere between the file watcher, `OrgSyncController`, and
`CacheEventSubscriber` / `SqlOperationProvider`, properties get
filtered against an allowlist that keeps `task_state`, `priority`,
`tags`, `scheduled`, `deadline`, `org_properties` but silently drops
unknown keys.

Note the MEMORY.md entry **CacheEventSubscriber Properties Fix (Feb
2026)**:

> SqlOperationProvider published flat params as event data;
> CacheEventSubscriber deserialized as Block, losing custom properties
> (`collapse-to`, `column-order`, etc.) due to `#[serde(default)]` on
> `properties: HashMap<String, Value>`. INSERT OR REPLACE in
> QueryableCache then overwrote SQL with `properties = '{}'`.
>
> Fix: `build_event_payload()` in `sql_operation_provider.rs`
> restructures flat params — nests extra props under `properties` key
> so Block deserialization preserves them.

This fix was for the **update** path. The failing case is also an
Update, but from the **external/org-sync** path. That fix may not
cover the OrgSyncController's parse-and-upsert flow, only the
programmatic `execute_operation` path.

**Validate by:** grep for `"task_state"`, `"priority"`, `"tags"`
inside the `blocks_differ` / upsert logic in `OrgSyncController`. Any
explicit field list there is the smoking gun. Also check whether the
org parser emits custom properties into a separate bag (e.g.
`org_properties` vs `properties`) and the upsert only propagates one
of them.

### H2 — Parse-time property classification drop

The org parser at parse time sorts `:PROPERTIES:` drawer entries into
"known" (promoted to typed fields like `task_state`, `priority`,
`scheduled`) and "other" (custom). If the "other" bucket isn't wired
through to `Block.properties` during parsing, the custom property is
lost before it reaches the sync controller.

**Validate by:** insert a dbg/trace on the parser's output right
before `OrgSyncController::upsert` — print the Block's `properties`
HashMap. If `effort` is missing already, parser bug; if present, the
loss is downstream.

### H3 — `INSERT OR REPLACE` wipes custom properties on re-upsert

When the same block is later touched by a non-external mutation
(or re-parsed when the file is re-read after sync echoes), an
`INSERT OR REPLACE` with `properties = '{...}'` written as JSON could
overwrite the prior `effort`-containing row.

The logs show the panic happens immediately after file write in the
same transition — no subsequent mutation has had time to overwrite.
So H3 is unlikely to be the *primary* cause, but might be a
contributing race if `SimulateRestart` ran just before this
transition (it does in one of the captured shrunken sequences).

### H4 — Property-drawer re-render round-trip lossy

If the org renderer strips properties it doesn't recognize when
re-writing (e.g. during the echo-suppression snapshot projection),
the on-disk file after sync has `effort` gone, and a subsequent
re-parse produces a Block without it. The log only shows the initial
write — would need to `tail` the file after the sync cycle to confirm.

**Validate by:** in `OrgSyncController::last_projection` comparison,
dump the projection it computed vs. the actual file content for the
failing block. If the projection lacks `effort`, the renderer is
dropping it.

## Suggested investigation path

1. **Start at the end** — run the failing case under
   `debugger-mcp` with a data breakpoint on the `properties` HashMap
   of the specific block ID. Step through from file parse → sync
   controller → cache subscriber → SQL upsert. Watch for the first
   point where `effort` is missing. This tells you whether H1, H2, or
   H4 fires first.
2. If H1 fires: grep for `task_state`, `priority`, `tags` as string
   literals inside the loro/sync/cache crates. Any explicit
   per-field handling that doesn't include generic property propagation
   is suspect.
3. If H2 fires: the org parser's property-drawer handler has a split
   path — fix it to pass through unknown keys verbatim into
   `Block.properties`.
4. If H4 fires: `OrgRenderer::render_entity_properties` is dropping
   custom keys. Check its allowlist.

## Quick bisect signal

The same test failure appears in `jj log`:
- `@- 2b7bc2085 refactor: complete cleanup` (before the userdriver
  refactor) should also show this failure if the bug predates the
  refactor.
- Run `jj new 2b7bc2085` + the repro command to confirm.

I did not run this bisect — the time budget was on landing the plan.
If confirmed pre-existing, the owner can file it independently and
the userdriver refactor is unblocked.

## Files that matter

| File | Why |
|---|---|
| `crates/holon-integration-tests/src/assertions.rs:55` | panic site — `assert_blocks_equivalent` |
| `crates/holon-integration-tests/src/test_environment.rs:1476` | `apply_external_mutation` — writes org file for the failing case |
| `crates/holon-integration-tests/src/pbt/generators.rs:282-320` | where `effort` gets into the mutation |
| `crates/holon-integration-tests/src/pbt/sut.rs:3957-3985` | `MutationSource::External` branch (caller of apply_external_mutation) |
| `crates/holon/src/sync/org_sync_controller.rs` | `blocks_differ` + upsert path — property propagation suspect |
| `crates/holon/src/sync/sql_operation_provider.rs` | `build_event_payload` — Feb 2026 custom-property fix; check if the org-sync path goes through it |
| Org parser's `:PROPERTIES:` drawer handler | property-classification fork |

## Related past investigations

- **CacheEventSubscriber Properties Fix (Feb 2026)** — MEMORY.md — same
  class of bug (custom properties dropping across an event boundary)
  on the programmatic update path. Look there for precedent.
- **docs/ORG_SYNTAX.md** — document-level convention for bare IDs in
  org files. Mostly about IDs, but the property-drawer section may
  reveal an allowlist.

## Non-goals for this hand-off

- Do not roll back the UserDriver refactor on this branch — it is
  unrelated. The regression test
  `patch_block_grow_double_shifts_entity_index` is green and the
  headless router correctness is verified by the remaining shadow
  unit tests.
- Do not add a new PBT case. Per project rules, extend the existing
  generator only; this bug is already reproduced by the existing
  `general_e2e_pbt`.

## Ownership notes

- The UserDriver refactor and its follow-ups (F_race, F_drop, F2, F5,
  F6, F_direct, F10) all landed cleanly and verified on
  `shadow_index::tests`. The PBT doesn't need to be green on this
  branch for the refactor to ship — it wasn't green on main either.
- Whoever picks this up: start with the **bisect signal** above (15
  minutes). If the bug predates the refactor branch, open a separate
  issue and unblock this branch immediately.
