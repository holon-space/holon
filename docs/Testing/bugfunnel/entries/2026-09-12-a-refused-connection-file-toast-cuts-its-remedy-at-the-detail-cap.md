---
id: 2026-09-12-a-refused-connection-file-toast-cuts-its-remedy-at-the-detail-cap
date: 2026-09-12
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  The toast for a refused connection file is one sentence, so the 320-character
  detail cap ends it in an ellipsis and the remedy — the clause telling the user
  what to change — is never painted, even when the window is nearly empty.
---

## Bug

Found by the `user-connections` dogfood RE-RUN of 2026-09-12, driving the real
GPUI app at `main` `e50f042ba5b3`.

Every connection file the loader refuses raises a toast reading
`Connection file cannot be used — <name>: <path> cannot be used — <reason and
remedy>`. The painted text ends in an ellipsis before the remedy. Two captures:

- `foreignsecret` ends at `(case-insensitively, with '-' and '_' alike) —
  otherwise a …`. The cut clause is `Rename the variable, or put this
  connection's own secret under 'FOREIGNSECRET_…' with holon-secret set.`
- `cleartexthost` ends at `(http:// to a …`. The cut clause is the loopback
  exception and `Fix the file and restart; the rest of the app is unaffected.`

The path survives, because it sits early in the sentence. What is lost is the
end: the instruction. Toast text cannot be selected, so the cut clause is
reachable nowhere on screen — only in the log.

This is not a layout problem. The `cleartexthost` capture has two toasts in a
1400x860 window with the entire upper two thirds of the page empty; the text is
cut by a character count, not by the space available.

Evidence under `/tmp/holon-dogfood-uc/`:

- `s1/shots/02-boot-860.png` — eight refused files, 1400x860.
- `s1b/shots/03-boot-720.png` — the same set at 1400x720.
- `s1c/shots/04-cleartext.png` — one refusal, nearly empty window, still cut.
- `s1-app-860.log`, `s1c-app-cleartext.log` — the full, uncut messages as logged.

## Root cause

`frontends/gpui/src/share_ui.rs:2094` caps a toast's first detail SENTENCE at
`MAX_DETAIL_CHARS = 320`. Lines after the first `\n` are exempt and painted
verbatim — that exemption is the mechanism by which a path or a command
survives whole.

`frontends/gpui/src/share_ui.rs:569` builds the refusal's detail as
`format!("{provider}: {installed_path} cannot be used — {why}")`. It has no
newline, so the whole disclosure — path, reason and remedy together — is one
sentence and the cap falls inside it. The `{why}` strings this feature added run
to roughly 500 characters.

The neighbouring `IntegrationNotEnabled` disclosure does the opposite and is
painted in full in the same runs: it puts the enable command and the state path
on their own lines, so both reach the user complete. That row is the working
example, in the same window, on the same screen.

## Missing piece

`frontends/gpui/tests/refusal_toasts_reach_the_user_windowed.rs` asserts that no
painted line contains an ellipsis, which is the right assertion. It never goes
red because it raises only `ShareDegradedReason::IntegrationNotEnabled` — the
one reason whose payloads are already on exempt lines. The
`IntegrationSidecarUnusable` reason, which is what every refusal in this feature
produces, is not in its fixture set.

So the rung pins the reason that was fixed and not the reason the refusals use.

## Remedy

FIXED by lane `uc-fixes-2`, generally rather than for this one kind.

**The pin, red first.** `frontends/gpui/tests/refusal_toasts_reach_the_user_windowed.rs`
now also raises `IntegrationSidecarUnusable`, and its `why` comes from the REAL
loader run over a real bad file in a temp directory — a hand-written sentence
would be as long as the test author chose, which is how the earlier fix came to
be measured against a message that merely got shorter. It is raised FIRST, so it
is inside the painted stack rather than in the overflow. Red for the right
reason: the painted line ended `declare it \`sql_type: TEX…`, cutting the remedy
mid-word.

**The fix.** `DegradedToast::detail` is no longer a `String` with a `\n`
convention. It is a `ToastDetail { headline, body }`: the headline is prose and
is capped, the body lines are payloads painted verbatim and never capped. The
split is in the type because the convention only held for the kinds whose author
remembered it. Every arm that carries a path, a command, a URL or a remedy was
moved onto body lines: `IntegrationSidecarUnusable`, `IntegrationNotEnabled`,
`IntegrationSidecarNotBundled`, `IntegrationSidecarSuperseded`,
`IntegrationNeedsAuth` and `PairingReimportedLocalContent`. Prose-only notices
build through `ToastDetail::prose`.

Files: `frontends/gpui/src/share_ui.rs` (the type, the arms, `toast_message`,
`toast_lines`; `split_detail` deleted), plus the construction sites in
`render/builders/pref_field.rs`, `render/builders/input_box.rs` and
`views/editor_view.rs`.

## Attribution

Regression of lane `sidecar-sync-fixes`, whose cap-exemption fix covered
`IntegrationNotEnabled` — the reason it was reported against — while every
refusal the loader itself raises is `IntegrationSidecarUnusable`. The rung
landed with it pinned the reason that had been fixed, not the reason the
refusals use.
