# ADR 0035 — A condition is one vocabulary, and its remedies are declared data

**Status:** Accepted (directive 2026-09-12 in D120.a; D121.a ratified by Martin
2026-09-14)
**Date:** 2026-09-14
**Deciders:** Martin (directive on generic error handling; decision-inbox
ruling D121)
**Relates to:**
[ADR 0024](0024-unified-action-execution.md) — the Petri net is the one action
language, so a remedy dispatches an operation and never a bespoke closure.
`docs/Architecture/Model.md` — the condition bus and invariant 14's disclosure
rule.
`docs/Architecture/Reactivity.md` — the derived-data contract the condition
holder obeys.
`~/.claude/plans/error-remedy-design.md` — the increment plan this ADR opens.

## Problem

Holon ranks a silent degradation last among outcomes: a fallback is legal only
when it is disclosed. The machinery for that existed and was good. A sticky
condition bus carried 22 kinds, each with a stable kind string, a subject, and
replay so a window opened after boot still saw a condition raised during boot.

What the bus did **not** carry was everything needed to draw a condition. How
bad it is, where it belongs on screen, when it goes away and what the user may
do about it were all re-invented in the GPUI frontend: a 22-arm colour match, a
300-line placement match, and a 26-variant enum mirroring the backend's kinds.
A second frontend would have re-invented all three, and each of them could drift
from the backend without anything failing.

The cost showed in the product. An inventory of every disclosure surface found
ten of them and exactly **one** button: the deferred-reimport banner's Retry.
Nine conditions told the user something was wrong and offered nothing to do
about it. Two surfaces — the "Sync failing" glyph and the sidebar dot — were not
on the bus at all; they read a SQL column, so the same fact had two authorities
that could disagree.

The all-clear was the sharper problem. Every kind's doc comment named the moment
that ended it, and that rule was right, but it was prose. Nothing observed it,
nothing acted on it, and nothing could check that a new kind named one.

## Decision

**One vocabulary, in `holon-api`.** The bus moves out of `holon-loro` and is
renamed from `ShareDegraded*` to `Condition*`; most of its kinds were never
about shares. `holon-api` already holds the closed boundary vocabularies (the
block-write fields of Model.md invariant 3), and conditions are the same kind of
thing: something every layer above and below has to agree on. An arch-lint rule
(`api-storage-backend`, `api-frontend-dep`) keeps the crate free of both a
storage backend and a frontend toolkit, so the vocabulary stays usable in a
configuration that has neither.

**The profile is declared per kind, as data.** `ConditionKind::profile()` is a
total match returning severity, label, placement, the all-clear and the legal
remedies. Four consequences were ratified individually in D121.a:

- **Severity is per kind, not per instance.** Per-instance severity is exactly
  what turned a colour lookup into a 22-arm match.
- **Placement is condition data, not frontend policy.** A frontend-side
  placement map is the mirror this ADR deletes. A frontend with no surface for a
  declared placement falls back to a toast and logs the substitution; it never
  drops the disclosure.
- **`Dismiss` is legal only where the all-clear is `RemedyApplied` or
  `Elapsed`.** Dismissing a condition that is still true hides it rather than
  resolving it, which is the failure this project ranks last.
- **`Elapsed` is legal only for `Severity::Info`.** Info is feedback, not
  degradation. A warning or error that vanishes on a timer is the silent
  vanishing the bus was built to stop.

The last two are enforced by construction. `ConditionProfile::new` is the only
constructor, it is a `const fn`, and every profile is a `const`, so a violating
profile is a compile error rather than a panic the first time that condition is
raised.

**Remedies reference a closed operation vocabulary.** `RemedySlot::Retry` takes
an `OpId`, not an op-name string, so an unknown operation cannot be written into
a profile. A remedy resolves to an `OperationIntent` and travels the path every
other click takes (ADR 0024), which is also what lets the keystone drive a
remedy with the existing driver rather than a new one.

**The all-clear is a typed moment.** `AllClear` names it: a `ClearingEvent`, a
clean ingest, a remedy applied, an elapsed duration, or `UntilRestart`. The last
is legal and disclosed as such — a condition true for the whole session should
say so rather than disappear and look resolved.

`ClearingEvent` is deliberately **not** `OpId`. A snapshot save, a SQL
projection and a rehydration are internal moments with no dispatcher operation
behind them; typing them as operations would claim a dispatch path that does not
exist.

## Consequences

Adding an error type becomes one table row plus one `emit` call, instead of
about forty lines across four files. Every frontend gets severity, placement and
remedies without asking, and the GPUI mirror is deleted rather than maintained.

The declared all-clear is the foundation for a later increment in which the
holder observes operation outcomes and clears on a subject match, so no author
can forget to call `clear`. That stream does not exist yet: failures and
successes travel separate channels today, and unifying them is its own step.

A rename of this size is a one-time cost paid across 31 files. It is mechanical,
and it buys the property that made it worth doing: the name no longer says
"share" for a vocabulary that is about the whole system.

`holon-api` is no longer purely about writes. That was the explicit trade-off in
D121.a question 2, taken because conditions and intents are read together and
one crate is easier to keep honest than two.

## Alternatives considered

**A frontend-side remedy table beside the colour match.** The smallest diff, and
it makes the mirror permanent: a second frontend re-implements it, and the
keystone cannot read remedies at all, so no headless test can pin them.

**A new disclosure type published beside the bus, migrating surfaces one at a
time.** Avoids the rename, at the price of two live disclosure channels for
several waves. The repository's standing rule is that old paths are not kept
"just in case", because the next agent copies whichever it reads first.

**Projecting conditions into a SQL table so templates could read them.** Rejected
by Martin on 2026-09-13: it hard-codes a storage backend into the layer every
configuration shares. The generic holder plus a source-agnostic row seam
replaces it.
