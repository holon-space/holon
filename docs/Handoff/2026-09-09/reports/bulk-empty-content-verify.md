# Adversarial verification — lane `bulk-empty-content`

Verifier session; every result below was produced by me in this session.

- Workspace: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/bulk-empty-content`
- `pwd` for every lane command: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/bulk-empty-content`
- `pwd` for every attribution command: `/Users/martin/Workspaces/pkm/holon` (primary repo)
- `pwd` for every sandbox command: `/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/probetree`
- Identity assert: `jj log -r @` = `0a78e6c91db5`, `@-` = `fcae936b1539` (`sw/ingest-dupslug`). Matches the brief.
- Tree-identity assert (before any gate): `crates/holon-org-format/src/inline_marks.rs:817` is
  `push_text(state, &raw, node_src.clone(), true);` inside the `if text.is_empty()` arm — the
  empty-label literal is emitted as plain text. sha256 of the lane file `3488026513d1bc65…`.
- I made **no** edit to the lane tree. `jj status` before and after shows the same four modified
  files and the same sha256. All my writes went to `lane-logs/` (gitignored: `.gitignore:36`) and to
  the scratchpad.

## Method: an out-of-tree sandbox

Rather than probe-editing the lane tree, I extracted the parent commit into the scratchpad with
`git -C /Users/martin/Workspaces/pkm/holon archive fcae936b1539 | tar -x` (run from the PRIMARY
repo, per the `git-archive-jj-workspace-empty` hazard; sentinel: 4464 files extracted, non-empty),
then copied the lane's three changed source/test files in. That gives me a tree I can flip between
OLD and NEW `inline_marks.rs` at will, with sha256 proof each way:

- OLD (`fcae936b:crates/holon-org-format/src/inline_marks.rs`) = `24ae84b6a9c522f0…`
- NEW (lane working copy) = `3488026513d1bc65…`

All sandbox builds used `CARGO_TARGET_DIR=<workspace>/lane-logs/target-verify-probe`, all lane
builds `…/target-verify`, both cold, all semaphore-wrapped. The corrupt shared `target/` was never
touched.

---

## C1 — attribution "pre-existing on main" — **CONFIRMED**

Blob-hash comparison across `main 830d794f878f`, chain tip `aaa5a817bd55`, and the actual parent
`fcae936b1539` (the lane compared only against the tip; I added the parent):

| File | verdict |
|---|---|
| `crates/holon-org-format/src/inline_marks.rs` | IDENTICAL across all three |
| `crates/holon-org-format/src/models.rs` | IDENTICAL across all three |
| `crates/holon-pbt-core/src/content_generators.rs` | IDENTICAL across all three |
| `crates/holon-integration-tests/src/pbt/types.rs` | IDENTICAL across all three |
| `crates/holon-integration-tests/src/pbt/generators.rs` | IDENTICAL across all three |
| `crates/holon-integration-tests/src/pbt/transitions/bulk_external_add.rs` | IDENTICAL across all three |
| `crates/holon-org-format/tests/degraded_render_severity.rs` | IDENTICAL across all three |
| `crates/holon-org-format/tests/render_marks_fixed_point_pbt.rs` | IDENTICAL across all three |

Pickaxe on the two implicated constructs returns exactly one commit each, and it is the one the lane
named:

- `git log -S 'text.is_empty()' -- crates/holon-org-format/src/inline_marks.rs` → `2b2cd69536` only
- `git log -S '[[{tail}]]' -- crates/holon-pbt-core/src/content_generators.rs` → `2b2cd69536` only
- `git merge-base --is-ancestor 2b2cd69536 830d794f878f` → YES. Commit `2b2cd6953670…`, "WIP",
  Sun Jul 5 00:02:45 2026 +0200.

**One imprecision in the lane report, not a refutation.** The report says "`git log
830d794f878f..aaa5a817` touches none of them". At *file* level that is true and is what matters. At
*directory* level four chain commits do touch the causal crates — `7e00a965f0`, `6452ac2dc0`,
`0d28c42396`, `5644b6d3de` — but only `pbt/loro_sync/stub_sut.rs`,
`pbt/window_slice/seed.rs`, `pbt/composed/two_instance_transport.rs`, and two `lib.rs` files
(architecture doc-comment sync). None is on the causal path. The attribution stands.

## C2 — root cause — **CONFIRMED**, and the reference's convergence rule is an oracle assumption

Both halves read and confirmed:

- Extract half, pre-fix (`inline_marks.rs`, OLD blob, the `MarkKindHint::Link` arm): `if
  text.is_empty() { return; }` — emits no mark **and no content**. `strip_link` trims the label
  (`inline_marks.rs:1071,1073`), so `[[   ]]`, `[[\t]]`, `[[\u{a0}]]`, `[[a][ ]]` and `[[][]]` all
  reach that arm.
- Render half: `DegradeReason::Unrepresentable.content_survives()` is `false`
  (`models.rs:1274`), and `disclose_degraded_render` routes a non-surviving rung to
  `tracing::error!` at **`models.rs:1309`** — exactly the line named. So the ERROR is a consequence
  of the rung, as claimed.

I then ported the reference model's convergence rule verbatim from
`crates/holon-integration-tests/src/pbt/types.rs:194-228` (`normalize_seeded`: up to 32 iterations of
`render_inline_marks` → headline trim → `extract_inline_marks`, stopping when text and marks stop
moving) into the sandbox and ran it on both code versions:

| input | ref converges to (OLD) | ref converges to (NEW) |
|---|---|---|
| `[[]]` | `""` | `"[[]]"` |
| `[[ ]]` | `""` | `"[[ ]]"` |
| `[[   ]]` | `""` | `"[[   ]]"` |
| `[[]][[]]` | `""` | `"[[]][[]]"` |

That is the `content sut="[[]]" ref=""` signature reproduced from first principles and then removed
by the fix. Evidence: `lane-logs/verify-ref-agreement-NEW.txt` vs `lane-logs/verify-ref-agreement.txt`.

**Is the convergence rule documented?** No — it is an oracle assumption encoded in code only. Its
sole statement is the three-word doc comment `/// The render→re-ingest fixed point, started from
(content, seed_marks).` at `types.rs:193`. It appears in no ADR or architecture doc I could find.
It also carries an asymmetry the lane did not mention: the reference converges through
`render_inline_marks` (the pure quoting function), while the SUT emits through
`render_block_content` (the degradation ladder). For the empty-link class both are the identity, so
it does not affect this fix, but it is the reason the ref and a naive SUT replica disagree on
ordinary emphasis (see the residual note below).

## C3 — the fix, and the `]][[` protection — **CONFIRMED**

I ran the lane's `render_marks_fixed_point_pbt` myself plus my own adversarial probe
(`crates/holon-org-format/tests/zz_verifier_adversarial.rs` in the sandbox, 23 inputs, 4 cycles
each, recording emitted bytes / fidelity / extracted content / extracted marks per cycle). Log:
`lane-logs/verify-adv-table.txt`; run `lane-logs/verify-adversarial-new2.log`,
`28 tests run: 28 passed`.

Every input the brief named, and 17 more, reaches a fixed point with **byte-stable disk output from
cycle 0** and fidelity `Exact`:

`[[]]`, `[[ ]]`, `[[   ]]`, `[[][]]`, `[[]][[]]`, `[[]]]`, `[[]] text [[x]]`, `a[[]]b`, `[[a][ ]]`,
`[[a][]]`, `[[][b]]`, `[[[[]]]]`, `*[[]]*`, `=[[]]=`, `~[[]]~`, `[[]]\n[[]]`, `[[\t]]`,
`[[\u{a0}]]`, `[[]]*bold*`, `[[x][]] tail`, `- [[]]`, `[[]] [[]] [[]]`, `[[ ]][[x][y]]`.

Four rows report `authored_preserved=false` — `[[]] text [[x]]`, `[[][b]]`, `[[[[]]]]`,
`[[ ]][[x][y]]`. In all four the difference is ordinary **link adoption** (`[[x]]` becomes content
`x` plus a `Link` mark), the documented contract, and the emitted bytes are identical on every
cycle. No silent byte change anywhere.

Zero-width mark protection: my second probe asserts `m.start < m.end` for every mark minted over 4
cycles of all 23 inputs — passes. No `]][[` appears in any emitted string. The render-side safety
nets are untouched by the diff and still present: the zero-length filter at `inline_marks.rs:1132`
and the tests at `inline_marks.rs:1484` and `:1528`.

Old-vs-new contrast, same probe, OLD blob (`lane-logs/verify-adv-table-OLD.txt`): **20 of 23 inputs
were erased or partly erased**, e.g. `[[]]` → `""`, `a[[]]b` → `"ab"`, `- [[]]` → `"- "`,
`[[]] [[]] [[]]` → `"  "`, `[[x][]] tail` → `" tail"`. Under the fix that is 0 of 23.

## C4 — red for the right reason — **CONFIRMED**

I put the lane's three changed test files onto the OLD `inline_marks.rs` in the sandbox and ran
them. `lane-logs/verify-adversarial-OLD.log`, `Summary 27 tests run: 24 passed, 3 failed`:

```
thread 'a_link_that_adopts_to_nothing_survives_re_ingest' panicked at
crates/holon-org-format/tests/render_marks_fixed_point_pbt.rs:343:9:
assertion `left == right` failed: "[[   ]]": re-ingest erased bytes the renderer had preserved
  left: ""
```

Same file, same line, same message, same `left: ""` as the lane's `RED-extract-empty-link.log`.
The failure is the erasure, not a harness artifact: the assertion compares `extract_inline_marks(c)`
against `c` directly. The two sibling tests fail on the OLD code for the *fidelity* half
(`left: ContentUnpreserved right: Exact`), also correct. Every other test in those three binaries
passes on the OLD code, so nothing else is entangled.

## C5 — the three changed tests — **CONFIRMED, no historic protection weakened**

| Test | OLD pin | NEW pin | assessment |
|---|---|---|---|
| `inline_marks::tests::empty_link_is_dropped_at_extraction_no_zero_width_mark` → `…_mints_no_zero_width_mark_and_keeps_its_bytes` | `extract("[[]]") == ""`, `extract("a[[]]b") == "ab"`, no marks | `extract(input) == input` for `[[]]`, `[[][]]`, `[[   ]]`; `extract("a[[]]b") == "a[[]]b"`; no marks | The no-mark half — the actual `]][[` root — is **kept verbatim and widened** by a third input. Only the erasure half flipped. Strengthened. |
| `degraded_render_severity::a_link_that_adopts_to_nothing_still_errors` → `…_is_not_a_degradation_at_all` | `ContentUnpreserved` + one `Level::ERROR` `Unrepresentable` event for `[[   ]]` | `Exact` + no events | This is the one to scrutinise, and it is **not** vacuous. The ERROR arm of the severity mapping is independently pinned by `the_content_unpreserved_rung_still_errors_and_names_itself` (same file, `[[a][b]]` under a `Link` mark), whose own comment says "Without this row the severity mapping could be satisfied by demoting everything". I ran it: green on both OLD and NEW code, so it still has teeth. |
| `render_marks_fixed_point_pbt::a_link_that_adopts_to_nothing_keeps_its_bytes_and_stays_loud` → `…_and_settles` | `ContentUnpreserved` for 5 inputs | `Exact` for the same 5 inputs | Loosens the fidelity claim, but the accompanying **new** test `a_link_that_adopts_to_nothing_survives_re_ingest` adds a strictly stronger obligation on the same 5-input class: byte identity through re-ingest, no marks, plus a 2-cycle `assert_fixed_point`. Net stronger. |

Nothing else in the tree pinned the erasure. I searched every `[[]]` / `[[ ]]` site outside the
diff. Two look load-bearing and both are unaffected:
`crates/holon-logseq-db/src/project.rs:556` asserts only `link_marks("[[]]").is_empty()` (still
true), and `crates/holon/src/api/operation_dispatcher.rs:1033` guards on `!marks.is_empty()` so the
create path leaves the literal alone — its comment ("keeps a link with no representable label as the
author's bytes instead of erasing them") is now *more* accurate than before.

**My answer to the D110 question — product decision or oversight? A deliberate product decision
resting on an unexamined premise.** The evidence is the deleted doc comment itself: "Emitting it
would settle silently at `Exact` while deleting the user's bytes — the 'degrades to look fine'
outcome. The loud ERROR is the required answer here." That is a reasoned application of the repo's
error-handling priority order, and it was *correct given an erasing parser*: if the bytes really are
lost, disclosing loudly beats settling silently. The oversight is one level up — nobody asked
whether the erasure itself was necessary. `crates/holon-integration-tests/src/pbt/types.rs:387-402`
shows the erasure was equally deliberate at the time (the dogfood #4 fix chose "drop the zero-width
link" to stop the `]][[` corruption), and the 2026-08-08 bugfunnel entry names the residue precisely:
"Honest disclosure, wrong consequence." Dropping the *mark* was the necessary part; dropping the
*bytes* was collateral nobody separated out. So the old pin was not sloppy, and the flip is not a
weakening — it is the disclosure becoming unnecessary because the thing it disclosed is gone.

## C6 — gates re-run by me — all green

`pwd` for every row: `/Users/martin/Workspaces/pkm/holon/.claude/worktrees/bulk-empty-content`.

| Gate | Result | Log |
|---|---|---|
| `cargo check -p holon-org-format --all-targets` | PASS, `Finished dev profile in 4m 34s` (2 pre-existing warnings, both present on the parent) | `lane-logs/verify-check-orgformat.log` |
| `cargo nextest run -p holon-org-format` | PASS `264 tests run: 264 passed, 1 skipped` | `lane-logs/verify-nextest-orgformat.log` |
| `cargo nextest run -p holon-architecture-tests` | PASS `7 tests run: 7 passed` | `lane-logs/verify-arch.log` |
| `just keystone-smoke` #1 | exit 0, `4 passed; 0 failed` | `lane-logs/verify-smoke-1.log` |
| `just keystone-smoke` #2 | exit 0, `4 passed; 0 failed` | `lane-logs/verify-smoke-2.log` |
| `just keystone-smoke` #3 | exit 101 → classified **PASS-WITH-NOTE** | `lane-logs/verify-smoke-3.log` |
| `/usr/bin/python3 scripts/bugfunnel.py check` | `666 entries, 0 problems` | `lane-logs/verify-bugfunnel.log` |

Smoke #3 classification, `bash scripts/keystone-known-reds.sh lane-logs/verify-smoke-3.log`:

```
[known-reds] PASS-WITH-NOTE: 6 known-red panic(s), 0 novel, 0 collateral (ignored).
             Registry: docs/Testing/KeystoneKnownReds.md
```

All 6 are `drawer-open-matches-ref` (`inv-drawer-open-matches-ref`, drawer rendered open=true but
reference says open=false) — a registered known red, unrelated to this change. Note this is a
*different* known red from the `editor-text-mirror` one the lane's `keystone-full 16` hit; both are
registered, so neither is novel.

**`sut="[[]]"` occurrences across all three smoke runs: zero** (`grep -c 'sut="\[\['` returns 0 for
each log). Per the brief I therefore skipped `keystone-full` and `hand-authored`.

I did not re-run `just analyze-arch`, `check-worker-wasm`, `check-frontend-wasm` or
`hand-authored`; the brief scoped them out and `holon-org-format` is not a wasm-compiled crate.

## C7 — entry flip — **CONFIRMED**, with one small asymmetry

`docs/Testing/bugfunnel/entries/2026-08-08-blank-empty-link-puts-vault-into.md` flips `OPEN` →
`FIXED` and names four covering tests. The entry's symptom is "the renderer classifies them
`rung="Unrepresentable"` and logs `org render is DEGRADED …` at ERROR on every render, including a
clean cold boot". `degraded_render_severity::a_link_that_adopts_to_nothing_is_not_a_degradation_at_all`
captures the tracing events for `[[   ]]` and asserts the set is empty — that is the symptom,
directly. All four named tests exist and pass; I ran each.

Asymmetry (not a defect): the *event-capture* test covers only the whitespace variant `[[   ]]`. The
empty variant `[[]]`, which the entry title names first, is covered for `Exact` fidelity by
`a_link_that_adopts_to_nothing_keeps_its_bytes_and_settles` (which iterates it) but not by an
event-capture assertion. `Exact` cannot reach `disclose_degraded_render` at all, so the coverage is
sound; it is one hop of inference rather than a direct pin.

Triage decision checked against `.claude/skills/bug-gap-triage/SKILL.md`: the skill applies "whenever
a bug is found OUTSIDE an automated test". This one was found by the keystone, so creating no new
entry is correct. I also confirmed `docs/Testing/KeystoneKnownReds.md` holds **no** row for the
empty-link / `sut="[[]]"` signature, so the lane's "never registered" claim is right and there is no
stale row left behind to delete.

## C8 — comments, secrets, scope

**Scope: clean.** `jj diff --name-only` is exactly the four files, all under `crates/` and `docs/`.
No `scripts-lane/` helper, no stray file. `frontends/holon-worker/Cargo.lock` (which the lane
reported restoring) is absent from the diff — the restore held.

**Secrets: clean.** Grep of the full diff for key/token/password/private-key/`sk-`/vendor-name
shapes returns nothing. No real vault content; every fixture string is synthetic.

**Comments: one defect.** See D1 below. Otherwise the diff conforms to
`~/.claude/skills/commenting/SKILL.md`: no history ("was previously", dates, bug numbers as
narrative), no comment explaining what 1-5 lines do, and the rewritten doc comments describe the
current contract rather than the change. The new block at `inline_marks.rs:804-815` carries two
reasons (zero-width mark is illegal; the bytes must survive because the render half keeps them),
which is at but not over the two-reason budget.

---

## Defects found (described, not remedied)

**D1 — the fix's own comment states a fact its own test refutes.**
`crates/holon-org-format/src/inline_marks.rs:811-815`:

```rust
// The literal's BYTES still survive, as plain text. The render half
// refuses to normalize this shape away (`render_block_content`
// keeps the raw bytes on the `ContentUnpreserved` rung), so erasing
// them here would leave the two halves disagreeing about what the
// block holds and the store↔disk cycle without a fixed point.
```

After this fix `render_block_content_checked` returns `RenderFidelity::Exact` for this shape, not
`ContentUnpreserved`. The lane's own changed tests assert exactly that — `degraded_render_severity.rs:196`
(`assert_eq!(fidelity, RenderFidelity::Exact)`) and `render_marks_fixed_point_pbt.rs:322` — and my
probe table records `fid=Exact` for `[[   ]]`, `[[]]`, `[[ ]]` and every sibling. The comment names
a rung the code no longer takes, which is the misinformation-rot the commenting standard exists to
prevent. Severity: low (comment only), but it sits on the single most-read line of the diff and the
same commit is what made it false. Suggested shape of the correction is for the implementer; the
factual claim to keep is "the render half refuses to normalize this shape away and keeps the raw
bytes", without the rung name.

**D2 — a neighbouring comment is now ambiguous.**
`crates/holon-integration-tests/src/pbt/types.rs:392`: "The renderer/extractor now drop the
zero-width link, so the ref's expected on-disk form is clean." Read as "drop the zero-width *mark*"
it stays true; read as "drop the *literal*" it is now false. The test's assertions
(`!content.contains("]][")`, `marks.is_none()`) still pass and are in fact less vacuous than before
(content is now `"[[]]"` rather than `""`), so this is comment ambiguity only, outside the diff.
Severity: very low. Worth a one-word fix if the lane reopens the file; not worth reopening it for.

**Residual risk, not a defect.** My port of the reference's convergence rule disagrees with a naive
SUT replica on ordinary emphasis — `*bold*` → ref `"bold"` vs `"*bold*"`, `~c~` → ref `"c"` vs
`"~c~"`, `a *b* c` → ref `"a b c"` vs `"a *b* c"` — because the reference converges through
`render_inline_marks` while the ladder path verbatim-wraps markup literals. The three empty-link
cases that appear in that divergence list (`[[]]*bold*`, `*[[]]*`, `[[]]~c~`) diverge **only** by
that pre-existing emphasis asymmetry; the control cases with no link at all diverge identically. So
nothing empty-link-specific remains. I flag it because my replica is not the real keystone SUT and
cannot adjudicate the real pairing — the authority there is the keystone itself, which ran clean.

## Weave safety

Safe to weave. The change is confined to one crate's parse boundary, the diff is four files, the
attribution to pre-existing `main` code is proven at blob level against `main`, the chain tip **and**
the actual parent, and every gate I ran independently is green with zero novel keystone reds and zero
occurrences of the target signature. D1 is a comment correction that can ride on the same commit or
follow it; it blocks nothing.

## Caveat I am obliged to state

Neither the lane nor I reproduced the *original stochastic keystone red* before the fix. My proof
that the fix removes it is mechanistic and deterministic: the reference's own convergence rule,
ported verbatim, lands on `""` under the old parser and on the authored bytes under the new one. That
is the signature, but it is a port, not the keystone. Post-fix absence over my 3 smoke runs plus the
lane's 23 cases is supporting evidence only, for a shape the lane estimates at ~4% per draw.
