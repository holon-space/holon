# Verify: org-priority lane (D101.a) — CONFIRMED with reservations

pwd for every command: /Users/martin/Workspaces/pkm/holon/.claude/worktrees/org-priority
Base asserted: @- = 91b1501d4016. Tree asserts pass (no Low/Medium/High, no to_int/from_int;
RENDERER_VERSION = "2" at file_sync_controller.rs:82).

## Verdicts

C1 CONFIRMED. Priority(u8) over the letter, rank() 1..26, derived Ord == rank order
(crates/holon-api/src/types.rs:661-706); private field, both constructors enforce
uppercase-ASCII. cargo check --workspace --all-targets = 0 (my run) => all call sites
migrated. No production DESC consumer: holon-profiles / turso.rs / block_profile.yaml
`priority` is the unrelated variant-resolution field. Keystone seeds Integer(0) at
query_ast.rs:702/714, invalid under from_rank as it was under from_int — unchanged.

C2 CONFIRMED. parser.rs:889-910 resolves the drawer spelling before the generic loop,
which continues at :1000; cookie parse at :817 uses with_context, no panic. Disclosure
path is real, not just an Err: parse Err -> ingest_file Err -> quarantine +
writeback_disclosure degraded condition (file_sync_controller.rs:2352+).
My runs: holon-org-format 268/268; holon-orgmode --features di 13/13.

C3 CONFIRMED. `_priority_drawer_only` is STORE-ONLY: models.rs:938
(`!INTERNAL_KEYS.contains(k) && !k.starts_with('_')`) keeps every `_` key out of the
drawer, so the known erasure hazard does not bite. It reaches the store by explicit
forward on the ingest leg (holon-orgmode/src/block_params.rs:143-147) and by the
wholesale properties flatten on the Loro leg (holon-loro/src/loro_sync_controller.rs:2075).
Drawer re-emission at models.rs:945-957 keys off authored_drawer_order().
My run: holon-app --test org_store_org_round_trip 9/9, incl.
every_authored_priority_carrier_survives_the_store_byte_identical (3 shapes x 2 legs,
asserts byte identity) and every_authored_carrier_stores_the_same_canonical_rank.
Red-proof lane-logs/app-redproof-11229.log read by me: genuine, fails on
"[drawer/loro] control: the format-only leg must already reproduce the authored bytes".

C4 CONFIRMED (mechanism), not reproduced end-to-end; two holes.
Gate: file_sync_controller.rs:2891 skips only when stored == projection_hash(disk_bytes),
and RENDERER_VERSION is mixed in at :1401 => bump forces full ingest. One-shot: after
ingest the hash is re-stamped from `rendered` at :4553 + persist_disk_hash_for.
Legacy values do get corrected: on cold boot last_projection is empty, so the diff base
is seeded from the STORE (:3124-3140), and OrgFormatAdapter::content_differs compares
a.priority() != b.priority() (holon-orgmode/src/file_format.rs:113) — legacy 3-for-A and
legacy raw-string "A" both differ from the parsed value => update op.
I did NOT boot a synthesized vault under version "1"; verdict rests on code reading.
- Non-org files: YES, re-ingested needlessly. last_projection_doc is loaded at boot
  (:1318), so ByRecordedHome formats (.cook / wasm plugins) normally take the skip; the
  bump re-parses every one of them once through the interpreter.
- Store-only / already-erased priority: LEFT STALE. block_params.rs has
  `if let Some(priority) = block.priority()` with no else, so a block whose file carries
  no priority never gets its stored value nulled — a block the erasure bug already
  stripped on disk keeps its legacy (possibly inverted, or raw-string) rank forever, and
  content_differs sees None == None so no update is even emitted.
- TOCTOU branch at :4514 returns without re-stamping; that file re-ingests next boot too
  (pre-existing, rare).

C5 CONFIRMED except the vault line. Both entries exist under docs/Testing/bugfunnel/entries/
(inversion = gap ORACLE, status FIXED; erasure = gap COVERAGE, status FIXED).
bugfunnel.py check = 662 entries, 0 problems (my run).
VAULT: `git -C /Users/martin/Workspaces/pkm/holon-pkm status --short Projects/Holon/Now.org`
=> ` M Projects/Holon/Now.org` (115+/63-, mtime 2026-09-08 12:31). "Now.org untouched"
is NOT confirmable. Attribution to this lane is not established and the diff contains no
priority-related text; another agent in this session most likely wrote it.

C6 CONFIRMED with one flake.
- cargo check --workspace --all-targets: exit 0 (my run).
- just hand-authored: 9 passed, 0 failed (my run, 1314s).
- just keystone-smoke: FAILED on my first run under heavy load (20 concurrent
  cargo/rustc; 3 passed, 1 failed, general_e2e_composed_pbt, 303s). My script tail-30'd
  the output, so the panic signature was LOST and could not be classified. Isolated
  rerun: GREEN, and scripts/keystone-known-reds.sh classified it "GREEN: run passed".
  Consistent with the brief's load-induced-deadline-miss flake, but not proven so.

## Defects found (evidence, not fixed)

D1. Migration cannot repair, and never clears, a stale stored priority for a block whose
    file carries none — see C4. Files the erasure bug already stripped stay wrong.
    crates/holon-orgmode/src/block_params.rs (the `if let Some(priority)` with no else).
    The lane report notes the missing null as "out of scope" but does not connect it to
    the migration.
D2. Docs still teach the inverted mapping: docs/Vision/PetriNet.md:959,
    wiki/concepts/petri-net-wsjf.md:61, ARCHITECTURE.md:2100/2111 and the
    holon-petri/holon-api test fixtures all use
    `switch priority { 3.0 => 100.0, 2.0 => 40.0, 1.0 => 15.0 }`, i.e. rank 3 (= C, the
    LEAST important) mapped to the highest weight. Fixtures only, no production yaml.
D3. holon-core/src/traits.rs TaskEntity::priority() reads props.get("PRIORITY")
    (uppercase) while the canonical stored key is lowercase "priority" — it returns None
    for every org-written block. Pre-existing, but the lane touched this function and
    left the mismatch.
D4. holon-toon/src/models.rs:58 keeps its own A/B/C-only Priority whose from_letter
    returns Option and silently drops `[#D]` (org_reader.rs:165) — the exact behaviour
    this lane removed from the main parser. Divergent parse semantics between two org
    readers in one tree. (The lane report flags the type's existence, not the divergence.)
D5. Only the first matching drawer key is examined (parser.rs:889 `.find`), so
    `:priority: A` + `:PRIORITY: B` in one drawer is NOT caught by the disagreement
    refusal. Not reachable in the real vault (see below).

## Abuse / real-data probes I ran

- Whole vault scan (/Users/martin/Workspaces/pkm/holon-pkm, read-only python walk):
  948 cookies, 101 `:priority:` drawer lines, 0 non-`[A-Z]` drawer values, 0
  cookie/drawer disagreements, 0 duplicate drawer priorities in one headline, 0 non-ABC
  cookies. So the new LOUD refusal (which quarantines the whole file) cannot fire on the
  current vault. Note the escalation anyway: `:priority: a` (lowercase) or `:priority: 1`
  now refuses the ENTIRE file where it used to be an inert drawer string.

## Gaps

- No end-to-end migration test (the lane report says so openly, arguing the marker is the
  renderer version). I did not build one either — verdict on C4 is code-reading, not a boot.
- The 4th authored shape ("none") is not in the new byte-stability test; covered
  incidentally by the other round-trip tests in the same file.
- docs/Reference/CompassConventions.md documents `_drawer_order` but not the new sibling
  carrier `_priority_drawer_only`.

---

# Rev 2 — CONFIRMED, safe to weave

pwd asserted in-script on every gate (`PWD=/Users/martin/Workspaces/pkm/holon/.claude/worktrees/org-priority`);
base still @- = 91b1501d4016.

## Gates I ran myself (semaphore-wrapped script file, RUSTC_WRAPPER=)

| Gate | Result | Log |
|---|---|---|
| `cargo check --workspace --all-targets` | **0** | rev2-v-check.log |
| nextest `-p holon-org-format -p holon-toon -p holon-petri -p holon-app -p holon-orgmode --features holon-orgmode/di` | **712 run, 712 passed, 2 skipped** | rev2-v-nextest.log |
| `just keystone-smoke` | **4 passed, 0 failed**; keystone-known-reds.sh = GREEN | rev2-v-keystone.log |
| `just hand-authored` | **9 passed, 0 failed** (1201s) | rev2-v-hand.log |
| `bugfunnel.py check` | **665 entries, 0 problems** | — |

(No repeat of rev 1's load flake. The keystone log carries one
`settle_budget total_ms=203 slo_ms=200` WARN, explicitly "within the machine-load
slack" — the run passed.)

## The five reservations

D1 stale rank — CLOSED on BOTH legs. `block_params.rs` now always emits the column
(`Some -> Integer(rank)`, `None -> Value::Null`); `loro_sync_controller.rs`
`block_to_params` `.entry("priority").or_insert(Null)` and `block_diff_params` clears
the real COLUMN with `Null` instead of the `Value::REMOVED` property sentinel, which
only edited the JSON bag. Load-bearing companion I checked: `is_typed_field_drawer_key`
now matches `priority` case-insensitively, which keeps `drawer_properties()`'s
round-trip letter from re-clobbering the integer column through the generic emit loop.
RED-c genuine and end-to-end through the real SqlOperationProvider
(`left: Some(Priority(65))  right: None`); the green test does create->update->read-back.

D3 traits.rs — CLOSED. Reads the canonical lowercase `priority`, typed via
`TryFrom<Value>`, WARN instead of the old `panic!` on stored data. Pinned both
directions: uppercase asserted NOT a second storage key. RED-a genuine (2 fails).

D5 multi-carrier — CLOSED. Every drawer carrier is parsed and compared against the
running survivor, then against the cookie; each disagreement bails by name.
RED-b genuine.

D2 WSJF — I WAS WRONG, the lane is right. `default_computed_props`
(crates/holon-petri/src/lib.rs) is the PRODUCTION default prototype, not a fixture.
RED-d reproduces the `[#C]`-outranks-`[#A]` inversion exactly as asked: with the
production expression alone reverted to rank-3-highest,
`holon-petri::tests::rank_output_is_pinned_for_priority_ordering` fails at lib.rs:1632
with "priority 1 (`[#A]`) must rank first". So the fix's test DOES fail on
rank-3-highest — the coupling is proven by the log, not asserted. Mapping corrected in
all 7 places I checked; `DESIGN_EXTERNAL_ELEMENT_EMBEDDING.md:418` correctly left alone
(that block is explicitly "real indexed columns in the source table" for Todoist's own
1-4 scale, where 4 really is urgent).

Red-probe hygiene verified independently: `diff` of sha256-before/after is clean, and
the live `crates/holon-petri/src/lib.rs` hashes to 2fc41efcfe6994… — the value the
report claims it was restored to. No leftover revert.

## D4 deviation — assessed: ACCEPTED, but the readers still diverge

The two TYPES agree exactly. Both `from_letter` reduce to `c.is_ascii_uppercase()`
(holon-api/src/types.rs, holon-toon/src/models.rs), so the accepted/refused letter set
is identical, and toon now refuses a non-letter cookie loudly via
`ToonError::BadOrgPriority`. Keeping the copy is defensible: holon-toon is a
zero-runtime-dep codec and holon-api pulls rhai/tokio/opentelemetry.

The two READERS do not agree. Probe I ran (scratch crate against the real
`holon_toon::org_reader::parse_org`, scratchpad/verify-priority/toonprobe):

```
   ascii-A => OK  priority=Some(Priority(65)) title="Ship it"
  letter-D => OK  priority=Some(Priority(68)) title="Ship it"
   digit-1 => ERR ...BadOrgPriority
   lower-a => ERR ...BadOrgPriority
    two-AB => OK  priority=None title="[#AB] Ship it"
     empty => OK  priority=None title="[#] Ship it"
  nonascii => OK  priority=None title="[#Ä] Ship it"
    drawer => OK  priority=None title="Ship it"      <-- DIVERGENCE
```

DEFECT R1 (latent). For the identical drawer-only file
`* TODO Ship it` + `:priority: A`, holon-org-format yields the typed
`Some(Priority::A)` (pinned by its own green test) while holon-toon yields `None` and
leaves the letter as a raw string prop — rev 1's ORIGINAL defect, surviving verbatim in
the second reader. `org_reader.rs`'s new comment "the same verdict the org parser
reaches, so the two readers cannot disagree about what a file means" is false as
written. Cookie parity was fixed; drawer parity was not.

DEFECT R2 (latent, smaller). `[#Ä]`: toon is byte-indexed
(`after.as_bytes()[1] == b']'`) so a multi-byte cookie silently falls through and stays
in the title. orgize's grammar is `l_bracket, hash, anychar, r_bracket` (char-based;
$CARGO_GIT/orgize-*/src/syntax/headline.rs:224), so org-format gets the token "Ä" and
`from_letter` refuses the file. Toon side run; org-format side read from the grammar,
not executed.

BOUNDED: `holon_toon::org_reader` has NO in-tree consumer — the only dependant,
`frontends/mcp`, uses just the table codec (`Row`/`ToonValue`/`Table::from_rows`).
So R1/R2 are latent, not live, and do not block the weave. They belong in the same
follow-up as the "extract a tiny holon-priority crate" option the lane offers.

## Gaps (non-blocking)

- Two drawer spellings that AGREE (`:priority: A` + `:PRIORITY: A`) round-trip lossily:
  models.rs re-emits via a single `.find()`, so the second line is dropped on
  write-back. Zero occurrences in the real vault (rev-1 scan).
- `the_uppercase_drawer_spelling_normalises_onto_the_canonical_key` asserts the typed
  field and the absence of a second key, but not byte-stable write-back of `:PRIORITY:`.
- Still no booted migration test (the lane says so itself), and `Value::REMOVED` remains
  wrong for `task_state`/`scheduled`/`deadline`/`task_state_category` — the lane names
  this and defers it to its own lane. Correct call; it is pre-existing, not introduced.

## Verdict

CONFIRMED. All five reservations closed red-first with genuine logs, all gates green on
my own run, red-probe hygiene clean, my D2 understatement corrected with proof.
SAFE TO WEAVE, carrying R1/R2 as a follow-up lane.

---

# Rev 2+3 — CONSOLIDATED VERDICT: CONFIRMED, safe to weave
(supersedes the interim "Rev 2" section above, which was written before rev 3 landed)

pwd asserted in-script on every gate. Base @- = 91b1501d4016 throughout.
Rev-2 gates ran against @ = 16f6be988107; rev 3 amended @ to 1440411d51db at
01:43-01:50, AFTER my rev-2 nextest finished — so I re-ran the affected gates.

## Gates I ran myself

Against @ = 16f6be988107 (rev 2):
| `cargo check --workspace --all-targets` | 0 |
| nextest `-p holon-org-format -p holon-toon -p holon-petri -p holon-app -p holon-orgmode --features holon-orgmode/di` | **712 run, 712 passed, 2 skipped** |
| `just keystone-smoke` | **4 passed, 0 failed**; keystone-known-reds.sh GREEN |
| `just hand-authored` | **9 passed, 0 failed** (1201s) |

Against @ = 1440411d51db (rev 3, tree asserted in-script):
| `cargo check --workspace --all-targets` | **0** |
| `cargo tree -p holon-toon -e normal` | **only `thiserror`** — no holon-api, no holon-org-format |
| nextest `-p holon-toon -p holon-org-format` | **306 run, 306 passed, 1 skipped** |
| `bugfunnel.py check` | **665 entries, 0 problems** |

keystone/hand-authored were NOT re-run against rev 3. Justified: rev 3 touches only
`crates/holon-toon` (+ its Cargo dev-deps and one new test), and holon-toon has no
in-tree consumer on the keystone path — `frontends/mcp` uses only the table codec.

## Rev 2 — the five reservations, all CLOSED (detail in the section above)

D1 stale rank closed on BOTH legs (`Value::Null` clears the real column; the
`REMOVED` sentinel only edited the JSON bag). D3 traits.rs reads the canonical
lowercase key, pinned both ways, `panic!` gone. D5 every carrier compared.
D2: **I was wrong and the lane is right** — `default_computed_props` is production,
and `lane-logs/rev2-RED-d.log` reproduces `[#C]`-outranks-`[#A]` exactly: with the
production expression alone reverted to rank-3-highest,
`rank_output_is_pinned_for_priority_ordering` fails at lib.rs:1632 with
"priority 1 (`[#A]`) must rank first". The fix's test DOES fail on rank-3-highest —
proven by log, not asserted. Red-probe hygiene independently checked: sha256
before/after diff clean, and live `holon-petri/src/lib.rs` = 2fc41efcfe6994…, the
claimed restore value.

## Rev 3 — my two divergences CLOSED, verified by my own probe

`lane-logs/rev3-RED.log` is red for the right reason: the differential test fails
`left: None  right: Letter('A')` — my drawer-only defect, with the production
reader's answer as the control.

Re-ran my scratch probe (scratchpad/verify-priority/toonprobe) against rev 3:

```
     drawer => Some(Priority(65))   [was None — R1 CLOSED]
drawer-upper => Some(Priority(65))
   nonascii => ERR BadOrgPriority   [was silent None — R2 CLOSED]
 both-differ => ERR DisagreeingOrgPriority
 two-drawers => ERR DisagreeingOrgPriority
    digit-1 / lower-a => ERR;  ascii-A / letter-D => Some;  two-AB / empty => None
```

The differential test's 10 cases include both of my defects ("drawer only",
"non-ascii cookie letter") AND both disagreement shapes (cookie-vs-drawer, two
drawer spellings). It asserts the org-format verdict FIRST as the control, so two
wrongs cannot match. Dev-deps-only confirmed twice: Cargo.toml `[dev-dependencies]`
and my own `cargo tree -e normal`.

## Residual divergences I found in rev 3 (NOT pinned by the differential test)

R3 (plausible, worth adding). `:priority:` with an EMPTY value. Toon refuses
(my probe: `ERR ... the priority cookie [#]`). org-format almost certainly answers
`None`: CompassConventions records that an empty property value drops its key
entirely, so `priority` never reaches the drawer scan. Toon side RUN; org side
INFERRED — I did not execute org-format on it. Not exotic: an author can plausibly
type a bare `:priority:`.

R4 (exotic). `[#]]` — toon scans to the FIRST `]`, gets an empty inner, and falls
through to title text (my probe: `priority=None title="[#]] Ship it"`). orgize's
grammar is `l_bracket, hash, anychar, r_bracket`, so it yields the token `]`, which
`from_letter` refuses. Toon side RUN; org side READ from the grammar.

R5 (cosmetic). `ToonError::BadOrgPriority` says "the priority cookie [#…]" even when
the offending carrier is a DRAWER key — my probe shows a bad `:priority: AB` reported
as "the priority cookie [#AB]", naming a cookie the file never had.

All three are LATENT: `holon_toon::org_reader` still has no in-tree consumer. None
blocks the weave; they belong with the "extract a tiny holon-priority crate" follow-up.

## Other gaps carried forward (non-blocking)

- Two drawer spellings that AGREE round-trip lossily (models.rs re-emits via one
  `.find()`); zero occurrences in the real vault.
- The differential `Verdict` compares the priority only, never the resulting TITLE, so
  a cookie one reader strips and the other keeps would pass. Stated scope limit.
- Still no booted migration test; `Value::REMOVED` remains wrong for
  `task_state`/`scheduled`/`deadline` — the lane names both and defers them. Correct.

## Verdict

CONFIRMED. All five rev-2 reservations and both rev-3 divergences closed red-first
with genuine logs; every gate green on my own runs against the exact revs; the
zero-runtime-dep claim verified by `cargo tree`. **SAFE TO WEAVE**, carrying R3/R4/R5
as a follow-up.
