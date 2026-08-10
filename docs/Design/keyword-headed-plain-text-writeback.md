# Representing "plain text that begins with a task keyword"

**Status: OPTIONS ONLY — awaiting Martin's ruling. Nothing here is implemented.**
Scope: dogfood findings **F2** (task #78) and **F5**, which this analysis shows are
**one question asked at two layers**, not two bugs.

---

## 1. The question

A user types `TODO ` into a block, Holon promotes it (`task_state=TODO`, keyword stripped
from `content`), and the user presses Cmd-Z. Undo means *"treat that keystroke as ordinary
text instead"* — so the intended post-undo state is:

> a block with **no** task state whose **content literally begins with a keyword of this
> document's vocabulary**.

That state is **not representable** on the way back out. Two independent narrowings bite:

| | Layer that refuses | What it does | Finding |
|---|---|---|---|
| **Q1** | the **org file** | a headline `** TODO alpha four` re-ingests as a TASK, not as plain text beginning with "TODO" | F2 |
| **Q2** | the **store** | `content` cannot hold trailing whitespace at all, so `"TODO "` becomes `"TODO"` before it ever reaches a file | F5 |

Q2 is strictly narrower and fires first. A ruling on Q1 alone does **not** fix F5.

---

## 2. What is already proven (not hypothesis)

### F5 is NOT a defect in the promotion inverse

The brief assumed the inverse "drops the consumed trailing space". It does not. The engine
already restores the verbatim typed text — `crates/holon/src/api/operation_engine.rs:1229-1231`
overwrites the content inverse's value with `typed.clone()`.

Red test `undo_of_an_empty_remainder_promotion_restores_the_consumed_space`
(`crates/holon/tests/promote_task_keyword_compound.rs`, red log `lane-logs/f5-red.txt`) asserts at
**two layers** precisely to localize this, and the split is decisive:

```
undo_entry_values("inverse_ops", "content") == "TODO "   PASSED  <- the engine is correct
col("content")                              == "TODO "   FAILED
        left: Some("TODO")   right: Some("TODO ")                <- the store dropped it
```

The byte dies in `holon_api::content_canonical::canonicalize_stored_content`
(`crates/holon-api/src/content_canonical.rs:26-35`), reached via
`SqlOperationProvider::trimmed_content` (`crates/holon/src/core/sql_operation_provider.rs:311-336`):

```rust
let trimmed_end = content.trim_end();
```

**This is deliberate and load-bearing.** Its own doc comment states the reason: the first line
of a text block becomes the org headline, which the parser `.trim()`s on re-parse, so storing
trailing whitespace would guarantee a store/disk divergence on every round trip. The comment at
`sql_operation_provider.rs:325-330` names a second consumer: the transform is the **single source
of truth** shared with the GPUI editor's echo-suppression discriminator, and warns that a drift
"would let the store canonicalize typed whitespace the editor then fails to recognize, deleting it
from the buffer".

So "just keep the trailing space" is not a small fix — it is a change to a shared canonical form
with a documented data-loss failure mode on the editor side.

### Why F5 is not cosmetic

The dogfood report rated F5 minor. The guard analysis says otherwise. `detect_keyword_promotion`
(`crates/holon-org-format/src/task_keyword.rs:168-187`) has three guards; guard 3 — *prior content
was not itself keyword-headed* — is what makes an undone promotion **durable** (test G6,
`plain_block_that_merely_starts_with_a_keyword_never_promotes`).

* Non-empty arm: undo restores `TODO alpha one`, which **is** keyword-headed → guard 3 engages →
  re-promotion blocked. Durable.
* Empty-remainder arm: undo restores `TODO` (canonicalized), which is **not** keyword-headed
  (`keyword_headed` requires keyword + whitespace + rest, `task_keyword.rs:93-107`) → guard 3 does
  **not** engage.

`TODO` + the next space is exactly the primary promotion path (test G1,
`char_by_char_promotes_on_the_space`). The undo lands the block in the one state from which the
gesture re-fires. The lost byte is not a cosmetic byte; it is the byte that was holding the guard
open.

### The writeback shape

Unescaped and undisclosed: `** TODO alpha four` (dogfood §F2 table). Nothing on disk distinguishes
it from a genuine task, which is why the cold-boot re-ingest re-promotes it. In the keyword-only
case the disk form is `** TODO` — an empty-title task headline — and re-ingest yields
`content=""`, `task_state=TODO`: **the typed word is gone**.

---

## 3. Ecosystem survey (done first, per the brief — do not invent syntax)

**Org has no dedicated escape for a keyword-headed headline.** Emacs matches
`org-todo-regexp` against the first token after the stars; if the token is in
`org-todo-keywords` it is a keyword, unconditionally. There is no `\TODO`, no quoting form.

What org *does* document is a **general** escape: the **zero-width space U+200B**
([Escape Character, The Org Manual](https://orgmode.org/manual/Escape-Character.html)). It is the
sanctioned way to stop Org interpreting something as markup, and the community applies it to
headline-level ambiguity too. Mechanically it works here: a leading ZWSP makes the first token
`​TODO`, which no longer matches the keyword regexp.

Interop cost is real and measurable: **other org readers do not honor it.** Pandoc's org reader
does not treat ZWSP as an escape character
([jgm/pandoc#8716](https://github.com/jgm/pandoc/issues/8716)), so a Holon-written file round-tripped
through pandoc surfaces a stray invisible character in the text. LogSeq has no keyword-escape
concept at all; its parser is keyword-position-based like Emacs's.

Net: option (i) below is an **existing ecosystem convention rather than invented syntax**, but it
is a convention with known-incomplete support.

---

## 4. Options

Common evaluation axes: org-ecosystem interop, data-loss risk, parser symmetry, blast radius.

### (i) ZWSP escape on writeback, stripped symmetrically on ingest

Renderer emits `** ​TODO alpha four`; parser strips a leading ZWSP before keyword matching
and drops it from `content`.

* **Interop** — Emacs reads it correctly as plain text (the desired semantics). Pandoc leaks the
  character into the text. A user editing the headline in Emacs sees an invisible char they may
  delete, silently re-promoting the block on the next ingest.
* **Data loss** — none in Holon's own round trip. The failure mode is *cosmetic leakage* outward.
* **Symmetry** — must be exact and must live in ONE place, or the asymmetry becomes a new
  divergence class. Ingest side `crates/holon-org-format/src/parser.rs:774,887`; render side
  `crates/holon-org-format/src/org_renderer.rs:274`.
* **Does it fix F5?** **No.** ZWSP addresses Q1 only. `"TODO "` still cannot be stored, because
  `canonicalize_stored_content` trims before any renderer runs. F5 needs a Q2 answer too.
* **Breaks** — the org round-trip PBT and any golden-file test asserting exact headline bytes;
  every `keyword_headed` call site must agree on whether it sees the ZWSP or the stripped form.

### (ii) Disclose-and-accept: emit the ambiguous form, record the truth in a drawer

Writeback emits `** TODO alpha four` plus a property (e.g. `:HOLON_PLAIN_PREFIX: t`) that
re-ingest consults to keep the block plain.

* **Interop** — the headline stays byte-clean for every reader; Emacs and LogSeq still *show* it as
  a TODO, so what the user sees in Emacs disagrees with what Holon means. The drawer is inert
  noise elsewhere.
* **Data loss** — the drawer and the headline can desynchronize. If a user removes the keyword in
  Emacs the drawer still claims "plain", and the reverse is worse: a user who *adds* a genuine
  keyword gets it suppressed by a stale drawer. This is a new silent-divergence surface.
* **Symmetry** — good: both sides read one explicit field, no character-level trickery.
* **Does it fix F5?** No — same Q2 gap as (i).
* **Breaks** — property round-trip fixtures; any test asserting the drawer set of a plain block.

### (iii) Refuse to persist the state: undo works in-session, writeback normalizes with disclosure

Accept that the state is unrepresentable. Undo restores it in the store and the UI; the org
writeback normalizes (re-promotes) and **says so** — a WARN naming the block and the consequence,
per the project's fail-loud rule.

* **Interop** — perfect; Holon writes only forms org natively means.
* **Data loss** — the user's demotion is lost at the next writeback, but *loudly*. Contrast with
  today, which loses it **silently** — that is the actual violation the dogfood report cites.
* **Symmetry** — trivially preserved; nothing new to parse.
* **Does it fix F5?** Partially and honestly: it reframes both F2 and F5 as "this state is
  session-only", which is at least *true* and disclosed.
* **Breaks** — the F2 round-trip invariant as the brief phrased it ("promote → undo → render →
  re-ingest ⇒ identical block") could never pass; it would have to be restated as
  "⇒ identical block **or** a disclosed normalization".

### (iv) NEW — make the undo target representable: restore to a form that is both plain and stable

Rather than escaping an unrepresentable state, choose a post-undo content org can natively hold as
plain text. Concretely: keep the keyword out of the *headline-initial* position, or accept the
canonicalized `TODO` and **fix the guard instead** — widen guard 3 so that content which *equals*
a keyword (not just keyword-headed content) also blocks re-promotion.

* This does not make `** TODO alpha four` safe, so it is **not a complete F2 answer**.
* It *is* a complete and cheap **F5** answer for the re-promotion half: `TODO` would stop
  re-firing on the next space. The trailing byte stays lost, but the byte only mattered because it
  held the guard open (§2). Cost: it also blocks the legitimate G1 primary path
  (`TODO` + space → promote), which is the main authoring gesture — so it is a real
  behaviour trade, not free.
* **Breaks** — test G1 `char_by_char_promotes_on_the_space` **by construction**. That is the
  decisive objection and the reason I do not recommend it alone.

---

## 5. Recommendation

**Split the ruling, and do not treat F5 as small.**

1. **Q2 / F5 first, and treat it as the more urgent of the two.** It is reachable in two
   keystrokes, and its consequence is not a missing space but a *silently re-firing promotion*
   (§2). My recommendation is **(iii) for the store**: accept that `content` is canonicalized,
   and make the promotion undo **disclose** that it restored `TODO` rather than `TODO ` instead of
   pretending byte-fidelity. Pair it with the narrow half of (iv) *only if* Martin accepts losing
   the G1 gesture on an already-`TODO` block — I lean **against**, because G1 is the primary
   authoring path.
2. **Q1 / F2**: recommend **(iii)** over (i) and (ii). Both (i) and (ii) buy Holon-internal
   fidelity at the cost of writing files whose meaning depends on a Holon-specific convention that
   the wider ecosystem provably does not honor (pandoc for ZWSP; nothing at all for the drawer).
   Holon's stated priority order is "works correctly with real data" > "falls back **visibly**" >
   "fails clearly" > "silently degrades". Today we are at the forbidden level 4. Option (iii)
   moves us to level 2 immediately, with no new file-format surface and no new divergence class.
   Options (i)/(ii) *aim* at level 1 but introduce a fresh silent-divergence surface (a
   user-deletable invisible char; a desynchronizable drawer) — i.e. they risk re-entering level 4
   by a different door.
3. If Martin wants true durability rather than disclosure, **(i) ZWSP is the better of the two
   durable options** — it is at least a documented org convention, and its failure mode (a visible
   stray char in foreign tools) is *loud*, whereas (ii)'s failure mode (a stale drawer silently
   suppressing a real keyword) is *silent*, which is the property we are trying to eliminate.

**What I need ruled:** (a) Q2 — may the promotion undo disclose-and-normalize, or must `content`
learn to hold trailing whitespace (a change to a shared canonical form with a documented editor
data-loss mode)? (b) Q1 — disclosure (iii), ZWSP (i), or drawer (ii)? (c) whether the F2 invariant
should be restated to admit a disclosed normalization.

---

## 6. Anchors

| What | Where |
|---|---|
| Canonicalizer (Q2 root cause) | `crates/holon-api/src/content_canonical.rs:26-35` |
| Its store call site + rationale | `crates/holon/src/core/sql_operation_provider.rs:311-336` |
| Promotion inverse (already verbatim — NOT the bug) | `crates/holon/src/api/operation_engine.rs:1229-1231` |
| Promotion guards; guard 3 is the durability guard | `crates/holon-org-format/src/task_keyword.rs:168-187` |
| `keyword_headed` (why bare `TODO` fails guard 3) | `crates/holon-org-format/src/task_keyword.rs:93-107` |
| G1 primary path (blocked by option iv) | `task_keyword.rs` test `char_by_char_promotes_on_the_space` |
| G6 durability lock | `task_keyword.rs` test `plain_block_that_merely_starts_with_a_keyword_never_promotes` |
| F5 red log | `lane-logs/f5-red.txt` |
| Org escape-character convention | <https://orgmode.org/manual/Escape-Character.html> |
| Pandoc does not honor ZWSP | <https://github.com/jgm/pandoc/issues/8716> |
