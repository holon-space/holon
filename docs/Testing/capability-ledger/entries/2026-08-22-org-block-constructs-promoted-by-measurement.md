---
id: 2026-08-22-org-block-constructs-promoted-by-measurement
date: 2026-08-22
profile: org
axis: content
leg: content
construct: table,logbook,quote,list,underline,strikethrough,code
status: PROMOTED
summary: >-
  seven constructs the org profile omitted round-trip intact and were promoted;
  this discharges the UNKNOWN-resolution obligation that blocks 2b.5
---

## What the certifier saw

Increment 2b.2 drove EVERY construct in the closed content vocabulary against
the real org write-back, declared or not. Seven came back intact while the
profile did not list them:

- block: `table`, `logbook`, `quote`, `list`
- inline: `underline`, `strikethrough`, `code`

## Why it matters

`table` and `logbook` were the draft's named UNKNOWNs
(`~/.claude/plans/capability-vocabulary-draft-2026-08-22.md` §2.1: "table and
logbook are ABSENT: recon found no parser or renderer support. UNKNOWN rather
than proven-absent"). The draft was WRONG — both survive. Recon had looked for
a parser that MODELS them and found none, which is true and is a different
question from whether the bytes round-trip.

That distinction is the whole reason the obligation existed. 2b.5 refuses
content on the strength of profile clauses, and refusing a table because nobody
looked would have rejected ordinary Emacs-authored org.

`quote` and `list` were never mentioned in the draft at all — the certifier
found them because it drives the whole vocabulary rather than only the declared
part.

## Remedy

PROMOTED into `crates/holon-org-format/profile.yaml`. The obligation named in
the plan's 2b.2 section is discharged BY MEASUREMENT for these seven.

One UNKNOWN deliberately NOT resolved: `escape_sequence` stays absent and stays
marked. The draft's claim is that backslash escapes are not HONOURED — a
semantic question — while the content probe only observes whether the bytes
return. A `\*` that survives verbatim proves nothing about escaping, so
promoting it on this evidence would have been a false promotion. Certifying it
needs an oracle that asks whether the marked-up region was suppressed.

## Scope of the claim

Survival here is byte-level CARRIAGE: org keeps the lines. It is NOT a claim
that Holon models a table or a logbook — nothing parses them into structure. A
consumer reading `table` in this profile learns that a table put into an
org-homed block comes back, and nothing more.
