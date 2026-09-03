# Adversarial verification — search-fix rev 3

Tree: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/search-fix`
Identity: `FOLD_CLASSES` and `SearchQueryTooLong` both present in
`crates/holon/src/api/query_engine.rs`. Not `wrong-tree`.

Workspace integrity after all probes (cp + sha256, no jj/git writes):
- `crates/holon/src/api/query_engine.rs` = `e4a749acf9c045ff4c2122418198f0f5d5be8f4a8f8f88f7b92854f2ca3637c6` (unchanged)
- `crates/holon-app/tests/quick_open_search_at_vault_scale.rs` = `cc5ef47e5c87f9cce2b8701bdb34a161abdab3fae207291fa3394eab4d28b22c` (unchanged)
- `jj diff -r @ --stat` = the same 8 files / 560 insertions / 56 deletions as at start.

Logs: `/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/search-fix-verify-logs/`

## OVERALL: CONFIRMED (6/6 claims), with 4 non-blocking DEFECTS, all documentation/disclosure

---

## Claim 1 — fold completeness — CONFIRMED

Probe method: `SearchMatch` and `FOLD_CLASSES` are private, so an external
`crates/holon/tests/` file cannot reach them. I appended a temporary
`#[cfg(test)] mod verifier_probe_r3` to `crates/holon/src/api/query_engine.rs`,
ran it, then restored the file from a pre-probe copy — sha256 above proves the
restore is byte-identical and `grep -c verifier_probe_r3` returns 0.

`fold-probe2.log` (all 9 tests pass, `Summary [ 1.467s] 9 tests run: 9 passed`):

- `probe_full_plane_oracle_equivalence` — line 178: `PROBE full-plane:
  checked=4632 divergences=0`. Sweeps **every Unicode scalar 0..=0x10FFFF**
  (the lane's own sweep stops at 0xFFFF) and asserts the emitted GLOB class
  equals the reference oracle's fold class for all 4632 cased characters.
  Zero divergences — astral planes included.
- `probe_astral_named_pairs` (line 127, PASS) — Deseret U+10400/U+10428, Osage
  U+104B0/U+104D8, Adlam U+1E900/U+1E922, Old Hungarian U+10C80/U+10CC0,
  Warang Citi U+118A0/U+118C0, Medefaidrin U+16E40/U+16E60: each case's class
  contains both spellings, from either query direction.
- `probe_rev2_divergence_cases` (line 115, PASS) — all 13 rev-2 divergence
  directions reach their counterpart: ß↔ẞ, the full ǅ/ǆ/Ǆ digraph family in
  every direction, K/k↔U+212A, Ω/ω↔U+2126.
  Line 119-121: `PROBE i-class = Some({'I','i'})`, `dotless-i-class = None`,
  `dotted-I-class = None` — Turkish ı (U+0131) and İ (U+0130) are correctly
  their own singletons under the declared oracle, emitted bare.
- `probe_class_wellformedness` (line 84) — `PROBE classes=1479 biggest=3 for
  U+01C6 missing_key=[] meta=[]`. Every class contains its own key; no member
  is a GLOB class metacharacter (`] - ^ [ * ? ' \`) that could reinterpret the
  bracket expression; every member's `simple_lower` equals its class key.
  This closes the "empty class / leading `]`" family the rev-2 report left open
  only by construction argument.

Multi-character-fold probe (`probe_multichar_full_folds`, line 94-100):

    ß  U+00DF: to_uppercase="SS"    emitted "[ßẞ]"
    ŉ  U+0149: to_uppercase="ʼN"    emitted "ŉ"
    ǰ  U+01F0: to_uppercase="J\u{30c}" emitted "ǰ"
    ﬁ U+FB01: to_uppercase="FI"    emitted "ﬁ"
    İ  U+0130: to_lowercase="i\u{307}" emitted "İ"
    ẖ  U+1E96: to_uppercase="H\u{331}" emitted "ẖ"
    PROBE ss body = "[Ss][Ss]"

What the reference oracle does: `folded_contains` /`simple_fold`
(`crates/holon-integration-tests/src/pbt/transitions/search.rs:95-105`) uses
Unicode **simple** lowercase — a character with a multi-character lowercase
maps to itself. So the oracle says `straße` must NOT find `STRASSE` and `ss`
must NOT find `ß`. The SUT agrees exactly: each of the six characters above is
emitted as itself or as its simple-lower class only, and `"ss"` compiles to
`[Ss][Ss]` with no `ß` in it. **SUT ≡ oracle on every multi-character-fold
case.** See DEFECT 2 for the disclosure gap this creates.

## Claim 2 — length refusal — CONFIRMED

Ceiling constant: `MAX_GLOB_PATTERN_BYTES = 50_000`
(`crates/holon/src/api/query_engine.rs:426`).

Checked against the engine, not taken on trust:
`/Users/martin/Workspaces/bigdata/turso/core/vdbe/value.rs:1311-1317`
    const MAX_GLOB_PATTERN_LENGTH: usize = 50000;
    if pattern.len() > MAX_GLOB_PATTERN_LENGTH { ... "GLOB pattern too complex" }
Same value, same byte unit (`&str::len()`), and **the same strict `>`** as the
lane's `if pattern_bytes > MAX_GLOB_PATTERN_BYTES`. Not looser than the
engine's; no off-by-one at the equality point.

Boundary measured for four per-character costs — `probe_length_boundary_ascii_and_widest`,
`fold-probe2.log:149-157`:

    "a" per_char=4 fits=12499 under=49998 accepted; +1 char -> refused (50002)
    "а" per_char=6 fits=8333  under=50000 accepted; +1 char -> refused (50006)
    "k" per_char=7 fits=7142  under=49996 accepted; +1 char -> refused (50003)
    "ǆ" per_char=8 fits=6249  under=49994 accepted; +1 char -> refused (50002)

The Cyrillic row lands **exactly on 50000** and is accepted — matching the
engine, which rejects only above it. Error text (line 150): `search query is
too long: case folding expands it to a 50002-byte GLOB pattern, over the
50000-byte limit the storage engine accepts`.

Surfaced as a search FAILURE, not "no matches", on both UI paths:
- `SearchMatch::new(...)?` at `query_engine.rs:81` and `:106` — both call sites
  propagate, so no over-long pattern can reach the engine.
- quick-open: `frontends/gpui/src/search_ui.rs:186-189` maps the `Err` to a
  string, `:231-234` stores it in `state.error`, and `:375` renders that error
  in place of the result body.
- `[[` link popup: `crates/holon-frontend/src/link_provider.rs:80-85` returns a
  popup item `format!("Search failed: {e}")`.
No other `SearchMatch::new` call site exists (grep over `crates` + `frontends`).

## Claim 3 — budget test honesty — CONFIRMED (it has teeth)

Baseline, unmodified tree — `vaultscale-base.log:72-83`:
`PASS [24.575s] quick_open_search_at_vault_scale`, keystroke "S" 1.150 s,
"Su" 1.191 s, "Sup"/"Supp"/"Suppe" ~150 ms. Reproduces the lane's numbers.

Teeth probe: I reverted the IN-subquery back to the JOIN shape in `pages_sql`
(`FROM block b JOIN block_tags bt ON bt.block_id = b.id AND bt.tag = 'Page'`).
`vaultscale-join.log:70-90`:

    FAIL [39.015s] quick_open_search_at_vault_scale
    keystroke "S": pages=20 content=30 in 9.190597916s
    panicked at crates/holon-app/tests/quick_open_search_at_vault_scale.rs:162:13:
    quick_open_search("S") took 9.190597916s, over the 6s join-regression bound

9.19 s vs a 6 s bound — the JOIN regression the test exists to pin is caught
with 3.2 s of margin, and the pass margin in the other direction is 4.8 s.
The 6 s bound is genuinely between the two populations. File restored, sha256
`cc5ef4...`/`e4a749...` verified.

## Claim 4 — hand-authored case — CONFIRMED both directions

Green, run by me: `just hand-authored`, `handauthored-full.log`
- line 6762: `[hand-authored regression] PASSED case "search-many-to-one-case-folds-reach-every-spelling"`
- line 6765: `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 643.25s`
- line 6767: `HAND-AUTHORED-EXIT=0`
- 80 `PASSED case` lines; no `SKIPPED`/quarantine lines.

Red-for-the-right-reason: I broke the fold by dropping U+1E9E (capital sharp S)
from every class (`members.retain(|c| *c != '\u{1E9E}')` inside the
`FOLD_CLASSES` builder) and re-ran the one case via
`HOLON_HAND_AUTHORED_CASE`. `handauthored-broken.log:356-358, 411, 415`:

    panicked at crates/holon-integration-tests/src/pbt/transitions/search.rs:254:13:
    quick_open_search("straße") missed block:searchstrasse in the In content
    section: its content "STRAẞE capital" contains the query and the section
    returned only 1 of its 30 slots, so nothing was truncated
    test result: FAILED. 8 passed; 1 failed ... finished in 8.65s
    HAND-AUTHORED-BROKEN-EXIT=101

Red for exactly the many-to-one fold reason, not a truncation or LIMIT
artifact — the message rules out truncation explicitly. Restored, sha256 verified.

## Claim 5 — entries honest — CONFIRMED (with DEFECT 2 and 3)

- `bugfunnel-check.log`: `638 entries, 0 problems`.
  `bugfunnel.py counts`: 635 escapes, FIXED 322 / OPEN 263.
- `search-does-not-fold-case-for-non-ascii-letters` — status `FIXED`. Rests on
  gate (a), which I re-ran and which passed (claim 4). Its Resolution names the
  two-element-class defect and the seven rev-2 divergences honestly, and its
  Covering-tests section states plainly that the `ADVERSARIAL_QUERIES`
  additions are vacuous against the `[a-z]` generated alphabet — I confirmed
  that: `search.rs:47-59` adds ß/ẞ/ǅ/ǆ/Ǆ/U+212A/U+2126/U+212B as queries only,
  and generated content never contains them. That disclosure is accurate.
- `a-broad-search-query-costs-a-second-at-vault-scale` — status `OPEN`, summary
  now says 0.8-5.3 s, Missing-piece section names `JOIN_REGRESSION_BOUND`/6 s
  and discloses the load range, Remedy references the renamed constant. My own
  quiet measurement (1.150 s / 1.191 s) falls inside the disclosed range.
- Stale "1500 ms": **zero** hits under `docs/Testing/bugfunnel/entries/*search*`.
  Seven hits remain in `lane-report-search-fix.md`, all benign: line 413 is a
  log-file timestamp, lines 349/365/371/374/384/455 are the rev-3 checkpoint
  narrative *describing* the staleness it then fixed. §1a itself (line 133) now
  reads `JOIN_REGRESSION_BOUND`, 6 s. Claim holds.

## Claim 6 — gates — CONFIRMED

`gate-nextest.log`
- line 2: `FMT-CHECK-CLEAN` (`cargo fmt --check`, whole tree). Independently
  reproduced a second time in `vaultscale-base.log:3`.
- line 975: `Summary [ 218.271s] 776 tests run: 771 passed (7 slow), 5 failed, 10 skipped`

The five failures (lines 976-980):

    holon::e2e_backend_engine_test test_basic_query_execution
    holon::e2e_backend_engine_test test_create_and_delete_workflow
    holon::e2e_backend_engine_test test_multiple_operations_sequence
    holon::e2e_backend_engine_test test_operation_triggers_stream_update
    holon::e2e_backend_engine_test test_query_and_watch_stream

A strict **subset** of the allowlist — `e2e_backend_engine_test ×5` only.
`test_multi_peer_sync_iroh` and the other five allowlisted names passed on my
run. **No failure outside the allowed set. Zero novel failures.**

---

## DEFECTS (none refute a claim; all documentation / disclosure)

**DEFECT 1 — the `MAX_GLOB_PATTERN_BYTES` doc comment understates the per-character cost.**
`crates/holon/src/api/query_engine.rs:422-425` says "A class costs up to six
bytes per query character". Measured maximum is **eight** bytes: the largest
class has 3 members (`probe_class_wellformedness`: `biggest=3 for U+01C6`), and
`ǆ` compiles to an 8-byte class (`fold-probe2.log:155`, `per_char=8`). No
correctness impact — the guard measures the real `body.len()`, not the estimate
— but "up to six" is stated as a bound and is not one.

**DEFECT 2 — the entry's "WHOLE Unicode fold-equivalence set" wording overclaims, and the ß↔ss disclosure was deleted.**
`docs/Testing/bugfunnel/entries/2026-09-03-search-does-not-fold-case-for-non-ascii-letters.md`
Resolution now says each class holds "its WHOLE Unicode fold-equivalence set".
The classes are whole *simple-lowercase* equivalence sets, which is strictly
coarser than Unicode simple case folding. Concrete instance produced this
session (`fold-probe2.log:158-159`):

    PROBE long-s emitted = "ſ"
    PROBE s emitted      = "[Ss]"

U+017F LATIN SMALL LETTER LONG S is emitted bare and is absent from `s`'s
class, so no query finds stored `ſ` except `ſ` itself. Unicode `CaseFolding.txt`
maps 017F→0073; Rust's `to_lowercase` does not. This is **consistent with the
declared oracle** (hence not a claim-1 divergence) but contradicts the entry's
own wording. Compounding it, the rev-3 rewrite **deleted** the previous
version's sentence explaining that folding is simple, not full, and that `ß`
therefore does not reach `SS` — see the diff hunk removing "Simple, not full,
folding: `ß` uppercases to `SS` ...". So an entry marked FIXED no longer
discloses that all-caps ASCII German `STRASSE` is unreachable from `straße`,
which is the same user-facing family the entry exists for. Confirmed
unreachable: `PROBE ss body = "[Ss][Ss]"` (no `ß`), and the hand-authored case
only stores `STRAẞE`, never `STRASSE`.

**DEFECT 3 — the length-refusal unit test is not listed among the covering tests.**
The same entry's Covering-tests section lists
`glob_class_is_the_oracles_whole_equivalence_class_across_the_bmp` and
`the_many_to_one_folds_reach_every_spelling` but omits
`an_over_long_query_is_refused_at_the_exact_threshold`, which is the only test
covering the `SearchQueryTooLong` behaviour the same Resolution section claims.

**DEFECT 4 — `lane-report-search-fix.md` contradicts itself top-down.**
The "Rev 3 — IN PROGRESS" section (lines ~340-395) is left verbatim and states
"status NOT YET corrected", "Not yet run", "not yet corrected" for four items
that the "Rev 3 — DONE" section immediately below reports as closed (and which
I independently confirmed closed). A reader stopping at the first section gets
the opposite of the truth.

## Notes on coverage the lane's own tests do not reach (not defects)

- The BMP sweep in `mod fold_class_tests` stops at 0xFFFF; the astral planes
  are covered only by construction. My full-plane sweep found no divergence, so
  the construction argument holds today, but nothing in the tree pins it.
- `FOLD_CLASSES` scans 1.1 M scalars on first use inside a `LazyLock`. Measured
  cost is inside the 1.44 s of a test that also does 4632 pattern builds, so it
  is not visible at the SLO scale, but it is paid on the first keystroke of a
  session and is not budgeted anywhere.

## Commands run

    cargo nextest run -p holon --lib -E 'test(verifier_probe_r3) + test(fold_class_tests)'   -> fold-probe.log, fold-probe2.log
    cargo fmt --check                                                                        -> vaultscale-base.log:3, gate-nextest.log:2
    cargo nextest run -p holon-app --test quick_open_search_at_vault_scale                   -> vaultscale-base.log, vaultscale-join.log
    just hand-authored                                                                       -> handauthored-full.log
    HOLON_HAND_AUTHORED_CASE=... just hand-authored (fold broken)                            -> handauthored-broken.log
    cargo nextest run --no-fail-fast -p holon -p holon-app -p holon-pbt-core                  -> gate-nextest.log
    /usr/bin/python3 scripts/bugfunnel.py check                                               -> bugfunnel-check.log

All builds via `sem --id holon-build -j6 --fg` with `RUSTC_WRAPPER=` and
`CARGO_BUILD_JOBS=6`; toolchain `nightly-2026-08-16-aarch64-apple-darwin`
(overridden by the workspace's own `rust-toolchain.toml`).
