---
id: 2026-09-12-the-not-switched-on-toast-truncates-the-command-it-tells-the-user-to-run
date: 2026-09-12
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  The "integration is not switched on" toast is a remedy command the user is
  meant to run, and it is cut off mid-path with an ellipsis in unselectable
  text, so the one actionable thing in it cannot be used.
---

## Bug

Found by the `dogfood-explorer` gate for `user-connections` (main
`f134df9ece6c`).

Dropping one connection file and launching produces a toast whose entire content
is a shell command:

```
⚠ Integration is not switched on — fixturebox: run
`HOLON_MCP_INTEGRATIONS_DIR='/private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon
/bc7b1e67-1603-4c68-8742-84215e1a79e3/scratchpad/dogfood-uc/state1/config/integrations' scripts
/holon-integration-enable.sh fixturebox` to write /private/tmp/claude-501/-Users-martin-Workspaces-pkm-holon
/bc7b1e67-1603…
```

The toast ends in an ellipsis. Screenshots
`scratchpad/dogfood-uc/shots/01-boot.png` and `06-masked-secret.png`.

The message is well written — it names the connection, the reason, the remedy
and the file the remedy writes. It is also unusable as delivered: nine of its
lines are one absolute path, the command it asks the user to run is cut short,
and toast text cannot be selected or copied. The correct action for a user here
is to retype a path they can only partly see.

This is the FIRST thing a user meets after introducing a connection, so it is
the feature's front door.

## Root cause

Not a code defect in the message. The disclosure is composed with full paths by
design — Increment 4's `IgnoredReason` arms deliberately name the file so the
user knows what to fix — and the toast renders what it is given.

The mismatch is between a disclosure whose payload is a COMMAND and a surface
built for a sentence. Nothing sizes, wraps or offers the payload as something
copyable.

## Missing piece

PERCEPTION. No assertion can say "this text is usable"; only a person or a
windowed layout test looking at overflow can. The windowed harness already
reasons about painted text (`painted_texts` in
`settings_integrations_ops_windowed.rs`) and about narrow windows
(`integrations_row_narrow_window_windowed.rs`), so a test asserting that a
disclosure carrying a path is not truncated is expressible — it has not been
written.

Related to, but distinct from,
`2026-09-12-refusal-toasts-push-each-other-off-screen-so-some-refusals-are-never-seen`:
that one is about disclosures that never render, this one is about a disclosure
that renders and still cannot be acted on. Same surface, different remedies.

## Remedy

FIXED along the line the entry recommended: the toast stops carrying the command.

The `IntegrationNotEnabled` disclosure now reads "<name> is installed but
switched off, so it runs nothing. Switch it on in Settings › Integrations
(<path>)". At roughly 200 characters it is under `toast_message`'s 320-character
cap, so no ellipsis appears and nothing is cut. The enable command and the state
path stay in the log, which can be read and copied; the Settings toggle is the
supported path and is now what the toast names.

One correction to the entry's headline, from reading the painted string: the
remedy command itself survived the cap (it ended at character 237 of 601). What
the ellipsis cut was the `state_path` clause that followed it. The entry's
substance is unaffected — the toast ended mid-path in unselectable text — and
the fix addresses the real shape: a disclosure whose payload was three absolute
paths on a surface built for a sentence.

The PERCEPTION gap is closed by
`frontends/gpui/tests/refusal_toasts_reach_the_user_windowed.rs`, which asserts
no painted toast line contains an ellipsis and that every refusal on screen says
where to act on it. Before the fix every one of the five painted lines ended in
"…".

CORRECTION, found by a verifier on the first version of this fix: shortening the
message until it happened to fit is not a fix. `MAX_DETAIL_CHARS = 320` was
untouched, so a deeper directory would truncate again. The cap now applies only
to a detail's FIRST LINE — its prose sentence — and every later line is painted
verbatim, which is the mechanism `PairingReimported` already used for its query.
The `IntegrationNotEnabled` disclosure puts the installed path on its own line,
so it is exempt by construction rather than by being short enough. The windowed
test's fixture path is 362 characters, deliberately longer than the cap, with a
precondition asserting that — otherwise the test could not tell a real fix from
a message that merely got shorter.

Consequence, and it is a real cost: a verbatim path wraps to ~175px inside the
toast box, so five such toasts are taller than the window and the top two paint
at zero height. The visible stack is therefore capped at THREE, with the rest
counted. Showing fewer refusals in full, and saying how many are not shown,
beats showing five of which two are silently clipped — but it is a budget, not a
guarantee: a path much longer than this fixture's would overflow again. The
durable fix is the Settings list of refused files, which does not exist yet.

Not done: making toast text selectable. That is a property of the whole toast
surface, not of this disclosure, and no other toast today needs it.

## Attribution

PRE-EXISTING, not a `user-connections` regression. Verified by reading the tree
at `a5e161c0` (the commit before that lane): every line named above is already
there — `git show a5e161c0:<path>`. What the lane changed is reachability: it
made the files user-supplied, so a shape that had only ever been authored
in-tree became one a user can write.
