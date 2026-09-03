# Verifier verdict — lane `org-bold-link`: **REFUTED**

WS `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/org-bold-link`, `@ = kttmwyno`, `@- = 89e2efea`.
All evidence produced in this session. Primary log:
`lane-logs/verify-all2.log` (926 KB, one detached run, all steps).

## Verdict

The claimed defect IS fixed for the shapes the lane tested, and the teeth are
genuine. But the fix **introduces a new silent corruption of the same class**
on doubled-emphasis nodes whose inner delimiter differs from the outer one.
Five shapes that round-tripped byte-identically on `@-` no longer do, and one
of them loses a styling mark outright.

## What is confirmed

| Check | Result | Evidence |
|---|---|---|
| Tree identity | `pwd` = WS; `emit_with_literal_inner_delimiters` at inline_marks.rs:896, `doubled_by` at :513; sha256 `6eb05603…` | this report, top |
| holon-org-format, PROPTEST_CASES=256, CI=true | 262 run: **261 passed, 1 failed** — the only red is my scratch probe | verify-all2.log:349 |
| fixed-point PBT @256 | PASS | verify-all2.log:346 |
| holon-app `org_store_org_round_trip` | 7 passed (1 leaky) | verify-all2.log:465-466 |
| `cargo check -p holon-gpui` | clean | verify-all2.log:621 (`STEP3_EXIT=0`) |
| Teeth (inline_marks.rs ← `@-`, sha `e3057a0b…`) | 23 run, **2 failed**: `render_lossless_shapes doubled_emphasis_round_trips_byte_identically` + probe | verify-all2.log:749-753 |
| Restore | `cmp` clean, `RESTORED_OK`, sha256 back to `6eb05603…` | verify-all2.log:754-755 |
| `just keystone-smoke` (PROPTEST_CASES=1) | ok, 4 passed | verify-all2.log:958-960 |
| `just hand-authored` | ok, 9 passed (648s) | verify-all2.log:7274-7276 |
| Bugfunnel entry | file exists, names all four covering tests; `counts` ok (610 escapes), `check` = 613 entries, 0 problems | run in-session |

No `jj`/`git` write command was run. The revert was `cp` only.

## The refutation — lane-introduced regression

Probe: a throwaway `crates/holon-org-format/tests/zz_verifier_scratch_shapes.rs`
(deleted afterwards) that asserts, for 41 shapes, duplicate-free marks AND
`render_lossless(extract_inline_marks(s)) == s`. Run once against the lane and
once against `@-` in the same session.

Five shapes are **NEQ on the lane and NOT NEQ on `@-`**:

```
NEQ "*/*x*/*"   -> content "*x*"   marks [Bold 0..3, Bold 1..2]           -> "***x***"
NEQ "/*/x/*/"   -> content "/x/"   marks [Italic 0..3, Italic 1..2]       -> "///x///"
NEQ "_*_x_*_"   -> content "_x_"   marks [Underline 0..3, Underline 1..2] -> "___x___"
NEQ "*_*x*_*"   -> content "*x*"   marks [Bold 0..3, Bold 1..2]           -> "***x***"
NEQ "*/**x**/*" -> content "**x**" marks [Bold 0..5, Bold 1..4]           -> "****x****"
```
verify-all2.log:326-330 (lane, STEP1) vs verify-all2.log:~690 (base, STEP4).

Worked counterexample, `*/*x*/*` (bold ▸ italic ▸ bold):

- expected (and what `@-` emits): `*/*x*/*`, marks `[Bold 0..1, Italic 0..1, Bold 0..1]`
  (base line: `DUP "*/*x*/*" -> ... all [Bold, Italic, Bold]` — a duplicate, but
  the bytes survive)
- actual on the lane: content `*x*`, marks `[Bold 0..3, Bold 1..2]`, rendered
  `***x***`

The **Italic mark is gone** and the authored `/` delimiters are replaced by `*`.
This is a one-way, unlogged loss on a page the app only re-rendered — the exact
failure mode the lane exists to remove.

Where it breaks: `emit_with_literal_inner_delimiters`
(`crates/holon-org-format/src/inline_marks.rs:896-940`) assumes the node is
`DDxDD`: it takes `delim = raw.chars().next()` and blindly
`strip_prefix_suffix(raw, 2, 2)`. The trigger at :870 only requires that
recursion yielded *a mark equal to `outer_mark` spanning the whole inner text* —
which is also true when a different delimiter sits between the two same-kind
ones. The middle delimiter chars are then discarded and re-emitted as the OUTER
delimiter. The run tiling inherits the same error: `lead_src = node_src.start +
kept_src.start - delim_len` points at the `/`, not a `*`.

The lane's new oracle does not see it: the two `Bold` spans now have *different*
ranges (0..3 and 1..2), so the duplicate-free assertion passes while the content
is corrupt. `render_marks_fixed_point_pbt` at 256 cases did not generate the
mixed-delimiter nesting.

## Not this lane's (pre-existing, both sides identical)

```
BAIL "*a **b** c*" -> no quote delimiter in ['=','~'] renders content "a *b c*"
                      (marks [Bold 0..4, Bold 2..4]) back to "a *b c*"
```
Present on `@-` too (base STEP4 list). Reportable separately: `render_lossless`
bails on a plausible authored shape, and the extracted content is already
mangled (`"a *b c*"` from `*a **b** c*`).

## Environment notes (not lane defects)

- The `holon-build` parallel semaphore was saturated by other lanes; two runs
  spent >40 min queued and were then killed by the harness's background-task
  timeout, producing 0-byte logs (`parallel --fg` buffers job output until exit).
  The successful run writes its own log via `exec >` inside the script.
- The machine's sccache server is **wedged**: `sccache --start-server` returns
  "Address in use" and `sccache -s` hangs, while every cargo invocation failed
  with `sccache: caused by: Failed to read response header` (first run,
  `lane-logs/verify-all.log`, all six steps `*_EXIT=101`). The reported run
  bypasses it with `RUSTC_WRAPPER=` — nothing was killed. This will false-red
  every other lane on this machine until an operator clears it.
- Report residual 3 (stale semaphore slots) is stale: all four holders were live
  `perl` processes when checked.

## What the lane must do before landing

Not my call to fix. The failing input set above is the reproducer; it should
become a red-for-the-right-reason case in
`render_lossless_shapes::doubled_emphasis_round_trips_byte_identically` and a
generator shape in the fixed-point PBT (mixed-delimiter nesting), since neither
currently reaches it.

---

# Rev 2 — **CONFIRMED**

`@ = kttmwyno f5ea45d2`, `inline_marks.rs` sha256 `24ae84b6…`. Log:
`lane-logs/v2-all-b.log` (a first run, `lane-logs/v2-all.log`, aborted on a
harness error of mine: nextest `-j6` IS `--test-threads`, so passing both is
rejected). Scripts used `RUSTC_WRAPPER=`, `CARGO_BUILD_JOBS=6`, `-j6`; sccache
is still wedged machine-wide.

| Check | Result | Evidence |
|---|---|---|
| Scratch file gone | `zz_verifier_scratch_shapes.rs` absent; `jj status` = the lane's 9 entries only | run in-session |
| Rev-1 regressions | **all five gone**: `*/*x*/*`, `/*/x/*/`, `_*_x_*_`, `*_*x*_*`, `*/**x**/*` round-trip byte-identically | v2-all-b.log:2-346 |
| 48-shape adversarial sweep (rev-1 list + `*/_+x+_/*`, `**_//y//_**`, `*x* **y** /z/`, unicode, newline, link nestings) | no NEQ, no DUP anywhere | same |
| org-format, PROPTEST_CASES=512, CI=true, cold (`find -name '*.proptest-regressions' -delete` matched nothing — the PBT sets `failure_persistence: None`) | 264 run: **263 passed**, 1 failed = my probe only | v2-all-b.log:343 |
| `org_store_org_round_trip` | 7 passed | v2-all-b.log:441 |
| `cargo check -p holon-gpui` | clean | v2-all-b.log:532 |
| Teeth (only the `full_span_active` guard at :891 disabled via `false &&`, sha `03d8289c…`) | 25 run, **4 failed**: `doubled_emphasis_…`, `nested_mixed_delimiter_emphasis_…`, PBT `nested_emphasis_text_…`, probe | v2-all-b.log:769-775 |
| Restore | `RESTORED_OK`, sha back to `24ae84b6…` | v2-all-b.log:776-777 |
| `just keystone-smoke` | ok, 4 passed | v2-all-b.log:978-980 |
| `just hand-authored` | ok, 9 passed (415s) | v2-all-b.log:7339-7341 |

## The two remaining probe failures are pre-existing, not rev 2's

Both are `render_lossless` BAILs of the family already filed OPEN as
`2026-09-03-emphasis-around-a-doubled-run-loses-the-inner-delimiters`, and both
BAIL **identically with the guard disabled** (teeth run):

```
"*a **b** c*"    -> content "a *b c*"     (the entry's own shape)
"***a *b* c***"  -> content "**a *b c***" marks [Bold 0..6]
```

`***a *b* c***` is not named in the entry; it is the entry's `*a *b* c*`
geometry wrapped in a doubled pair. Worth adding to that entry's shape list —
it is a parse defect upstream of the lane, present on both sides.

Rev 2 also *improves* one shape the guard-disabled build fails on:
`**[[https://e.test/1][a **b** c]]**` BAILs without the fix and round-trips
with it.

No `jj`/`git` write command was run; the teeth revert and restore were `cp`
only.
