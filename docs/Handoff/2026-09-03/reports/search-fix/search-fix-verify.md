# Verify: search-fix rev 2 (chain commit `2521d55021bc`, bookmark `sw/search-fix`)

# VERDICT: REFUTED (partial)

The **P0 stack overflow is genuinely fixed** and I reproduced both directions of that
proof myself. What is refuted is the wider claim attached to it: the lane reports
bug-funnel entry `2026-09-03-search-does-not-fold-case-for-non-ascii-letters` as
**FIXED**, and the new keystone `Search` transition declares an oracle
("Unicode simple case folding") that the shipped predicate **does not satisfy**.
I produced seven end-to-end oracle divergences against the real engine, including
ordinary German text. Two further defects: a new loud failure mode on long queries,
and a wall-clock gate that went red on my run while the report records it green.

Workspace for every command below: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/search-fix`
(`pwd` printed and confirmed on the first round). Working copy left clean; the one
probe edit was restored byte-for-byte (sha256 proof in check 3).

---

## STEP 0 — tree identity: PASS

Command: `rg -n "folded_column|LikeOperand" crates/` → exit 1, no output.
`rg -n "GLOB" crates/holon/src/api/query_engine.rs` → 8 hits (lines 387, 408, 411,
412, 418, 431, 443, 448). Correct tree.

---

## Check 1 — correctness probes against the oracle: **REFUTED**

Two probes.

**1A. Pattern-construction sweep** (standalone `rustc` binary replicating
`SearchMatch::new` verbatim plus the transition's `folded_contains` oracle, swept
over U+0000..U+10FFF).

Log: `lane-logs/verify-pattern-probe.log`
Summary line: `SUMMARY divergences=80`

**1B. End-to-end against the real engine.** Throwaway test
`crates/holon-app/tests/zz_verify_throwaway_search_probe.rs` (since deleted),
21 seeded blocks, 43 queries, each result compared to the transition's own
`folded_contains` oracle.

Command: `cargo nextest run -p holon-app --nocapture -E 'test(zz_verify_search_matches_oracle)'`
Log: `lane-logs/verify-oracle-probe.log`
Summary line: `Summary [1.589s] 1 test run: 0 passed, 1 failed` — `ERRORS (0): / DIVERGENCES (7):`

### (a) GLOB/LIKE metacharacters — CONFIRMED, no defect

Queries `*`, `?`, `[`, `]`, `^`, `%`, `_`, `'`, `\`, `[a-z]`, `star*`, `a*z`,
`snake_case`, `100%`, `sn_ke` all matched the oracle exactly. Spellings verified
in the pattern probe:

```
"*"    -> '*[*]*'          "]"  -> '*]*'        "%"     -> '*%*'
"?"    -> '*[?]*'          "^"  -> '*^*'        "_"     -> '*_*'
"["    -> '*[[]*'          "'"  -> '*''*'       "a[b"   -> '*[aA][[][bB]*'
"[^a]" -> '*[[]^[aA]]*'
```

A bare `]` outside a class and a leading `]` inside one are both handled by Turso:
`/Users/martin/.cargo/git/checkouts/turso-6395fa21babebd65/6988b54/core/vdbe/value.rs:1632-1639`
treats a leading `]` as a literal class member, and `]` never reaches the class
branch otherwise. Character classes iterate by `char`, not byte, so non-ASCII
members work (`pattern_compare`, same file line 1536).

### (b) ASCII + non-ASCII case variants — mostly correct

Cyrillic, Greek, Georgian, Turkish dotted/dotless `İ`/`ı`, and umlauts all agreed
with the oracle.

### (c) DEFECT 1 — characters whose simple-lowercase has a different simple-uppercase are unreachable

`crates/holon/src/api/query_engine.rs:426-435`. For each character the builder emits
the class `[simple_lower(c), simple_upper(simple_lower(c))]`. Any *other* character
that folds to the same lowercase is absent from that class, so **stored text
containing it can never be found**, by any spelling of the query. The oracle
(`folded_contains`, `crates/holon-integration-tests/src/pbt/transitions/search.rs:102-105`)
says it must match.

Seven reproduced divergences (block content in the fixture on the left):

| stored content | query | oracle | SUT |
|---|---|---|---|
| `Grüße Straße` | `GRÜẞE` | hit | **miss** |
| `STRAẞE capital` | `straße` | hit | **miss** |
| `STRAẞE capital` + `Grüße Straße` | `ẞ` | both | only `STRAẞE` |
| `STRAẞE capital` + `Grüße Straße` | `ß` | both | only `Grüße` |
| `ǅungla titled` | `ǅ` | hit | **miss** |
| `ǅungla titled` | `ǆ` | hit | **miss** |
| `ǅungla titled` | `Ǆ` | hit | **miss** |

The German pair is the user-facing one: all-caps German writes `STRAẞE`, and no
spelling of the query finds it. The sweep in 1A shows the same shape for the
titlecase digraphs `ǅ ǈ ǋ ǲ`, `ϴ` (U+03F4), the `ᾈ`-family, and — by construction —
the Kelvin sign U+212A, Ohm sign U+2126 and Angstrom sign U+212B, whose classes
come out as `[kK]`, `[ωΩ]`, `[åÅ]` and exclude the character itself.

Note `simple_upper` in `query_engine.rs:401-406` is *identical* to the oracle's own
`simple_upper` in `search.rs:95-100`. The bug is not the fold function; it is that a
two-element class cannot express a many-to-one fold. The keystone passes only
because neither `ADVERSARIAL_QUERIES` (`search.rs:35-49`) nor the generated block
content ever contains `ẞ` or a titlecase digraph — a generator-alphabet coverage
gap, not a passing property.

This is **not a regression** — the rev-1 `replace()` spelling folded with the same
simple map and missed the same characters. It refutes the *entry-FIXED* claim, not
the overflow claim.

### (d) empty class / leading `]` — no defect

Covered under (a). The builder cannot emit an empty class: every class is either two
cased characters or one metacharacter.

### (e) DEFECT 2 — long queries now fail loudly where they used to work

`LONG-5000-cyrillic elapsed=14.718042ms ok=true` — the 5 000-character requirement is met
with room to spare, and SQL grows in length not depth as claimed (pattern probe:
`LEN 5000 cased letters -> body chars = 20000`).

But the pattern is up to 4× the query in ASCII and 6× in Cyrillic **bytes**, against
Turso's hard cap:

```
core/vdbe/value.rs:1311   const MAX_GLOB_PATTERN_LENGTH: usize = 50000;
```

Measured:

```
LONG-20000-ascii elapsed=19.286875ms ok=false
LONG-20000 ERR: SQL execution failed: Query error: Failed to fetch row: GLOB pattern too complex
```

Threshold is roughly 12 500 ASCII cased characters or 8 300 Cyrillic. `LIKE` had no
such expansion, so this failure mode is new. It fails loud (surfaces as a search
error, not as "No matches"), so it is a bounded regression, not a silent one.

---

## Check 2 — bypass hunt: PASS, no defect

Command: `rg -n "SearchMatch" crates/` and
`rg -n "LIKE |GLOB |lower\(|replace\(" crates/holon/src/api/*.rs crates/holon-app/src`
Log: `lane-logs/verify-bypass-hunt.log`
Summary: `SearchMatch` has exactly two call sites — `query_engine.rs:81`
(`search_link_candidates`) and `query_engine.rs:106` (`quick_open_search`) — plus its
definition at 417/423. Every other `LIKE`/`replace(` hit in the sweep is id/quote
escaping or unrelated SQL (`holon_service.rs:271`, `block_domain.rs:573`,
`operation_engine.rs:3186`, the `id.replace('\'', "''")` family). **No search path
folds by nesting and none passes a bare column.** Both call sites go through
`contained_in`/`prefix_of` as claimed.

`frontends/gpui/src/search_ui.rs` adds only two `tracing::error!` calls on dropped
receiver / gone window. The newest-response generation guard is unchanged, which is
consistent: the race was cured by making the query fast, not by touching the guard.

---

## Check 3 — red-first teeth: **CONFIRMED, the test discriminates**

Baseline recorded before touching anything:
`lane-logs/verify-qe-baseline.sha256` →
`f6230933ce740a047dc734fbe34429fc86e39acc06570566749d65dcf0529698`

I copied `query_engine.rs` aside and rewrote `contained_in`/`prefix_of` to emit the
depth-nesting shape (one `replace(col, UPPER, lower)` per distinct cased letter,
then `LIKE '%lowered%'`), leaving everything else identical.

Command: `cargo nextest run -p holon-app --nocapture -E 'test(search_deep_script_query_does_not_overflow)'`
Log: `lane-logs/verify-teeth-red.log`
Summary line:

```
thread 'tokio-rt-worker' (76506639) has overflowed its stack
fatal runtime error: stack overflow, aborting
     SIGABRT [   1.633s] (1/1) holon-app::search_deep_script_query_does_not_overflow
     Summary [   1.635s] 1 test run: 0 passed, 1 failed, 168 skipped
```

Restored from the copy; `shasum -a 256` after restore returns the identical
`f6230933…29698`, and `jj status` reports "The working copy has no changes."

Green with the fix in place, same command, from check 5A:
`PASS [1.673s] (1/2) holon-app::search_deep_script_query_does_not_overflow`

The test has real teeth against exactly the shape the fix removed.

---

## Check 4 — the "P/P" claim: **CONFIRMED**

The test lives in `crates/holon/tests/create_page_from_link.rs` (the report's
`-p holon-app` framing is wrong; it is `-p holon`).

Command: `cargo nextest run -p holon --no-fail-fast --test create_page_from_link`, five times
Log: `lane-logs/verify-cpfl5.log`
Summary lines: 5/5 runs `4 tests run: 4 passed, 1 skipped` (3.34–3.43 s each).

Pass rate 5/5, no seed to capture. On the pre-existing-vs-caused question: the file
contains **no** reference to `quick_open_search`, `search_link_candidates`,
`SearchMatch` or `GLOB`, and the search diff touches no code path it exercises.
The `"P/P"` failure is independent of this diff.

---

## Check 5 — gate rerun of the discriminating subset: **one red I produced**

### 5A. `-p holon-app` filtered to `search`

Command: `cargo nextest run -p holon-app --no-fail-fast search`
(non-zero count asserted: `Starting 2 tests across 35 binaries`)
Log: `lane-logs/verify-gate-search.log`
Summary line: `Summary [47.471s] 2 tests run: 1 passed, 1 failed, 167 skipped`

- `search_deep_script_query_does_not_overflow` — **PASS** (1.673 s)
- `quick_open_search_at_vault_scale` — **FAIL** (47.469 s)

### DEFECT 3 — the vault-scale latency gate is red under normal shared-machine load

```
keystroke "S": pages=20 content=30 in 5.293855042s
panicked at crates/holon-app/tests/quick_open_search_at_vault_scale.rs:157:13:
quick_open_search("S") took 5.293855042s, over the 3s keystroke budget
```

The lane's own log `lane-logs/r2-nextest-111837.log:930` records
`PASS [52.197s] (713/713)` for the same test, so this is load-sensitive rather than
deterministic — the machine carries six lanes. Two things make it worth naming:

- The report (`lane-report-search-fix.md:131`) states "The `KEYSTROKE_BUDGET` is
  deliberately 1500 ms". The code says `Duration::from_millis(3000)`
  (`quick_open_search_at_vault_scale.rs:28`). The report documents a budget that is
  not the one shipped.
- The report characterises the open latency entry as "A 1–2 char query is ~0.8 s".
  I measured 5.29 s for the same one-character query on the same fixture. Whether
  that is contention or a GLOB-vs-LIKE scan cost, the number in the entry is not
  reproducible here.

A wall-clock assertion inside the ordinary `nextest` suite will keep flipping the
land gate red on a busy machine.

### 5B. `just keystone-smoke`

Command: `just keystone-smoke`
Log: `lane-logs/verify-keystone-smoke.log`
Summary line: `test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.01s`, `KSDONE exit=0`
**4/4 green.**

---

## Additional finding — the commit message describes rev 1, not rev 2

`jj log` for `2521d550` reads: "`SearchMatch`/`LikeOperand` carry an escaped pattern
with an **ESCAPE clause**". `LikeOperand` no longer exists, there is no `ESCAPE`
clause, and the predicate is `GLOB`. The landed description contradicts the landed
code.

---

## Claim-by-claim

| claim | verdict |
|---|---|
| `SearchMatch::folded_column` + `LikeOperand` deleted | CONFIRMED (step 0) |
| predicate is GLOB, each cased letter → `[lower upper]`, length not depth | CONFIRMED (1A: 5 000 letters → 20 000 chars) |
| `*?[` as one-element classes; `%`/`_` literal by construction | CONFIRMED (1a, 0 divergences on 15 metacharacter queries) |
| both call sites go through `contained_in`/`prefix_of` | CONFIRMED (check 2) |
| red-first test is RED without the fix, GREEN with it | CONFIRMED, reproduced both directions (check 3) |
| keystone-smoke 4/4 | CONFIRMED (check 5B) |
| `create_page_from_link` "P/P" is an unrelated random draw | CONFIRMED (check 4, 5/5 green, no coupling) |
| P0 stack overflow fixed | CONFIRMED |
| entry `search-does-not-fold-case-for-non-ascii-letters` FIXED | **REFUTED** — 7 oracle divergences, defect 1 |
| no regression in search | **REFUTED in one respect** — defect 2, long queries now error |
| gates green | **REFUTED on my run** — defect 3, vault-scale latency red at 5.29 s |
