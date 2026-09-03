---
id: 2026-09-03-rule-card-says-enabled-for-a-rule-that-never-parsed
date: 2026-09-03
gap: ORACLE
secondary: PERCEPTION
status: OPEN
summary: >-
  A holon_rule whose YAML fails to parse renders as "Automation rule · Enabled"
  with no error; the parse failure only reaches a log WARN.
---

## Bug

Found by the `dogfood-search` lane, exercising the rules surface on the live
app.

Authored one `holon_rule` source block under a page, then viewed the page. The
card renders:

    ◉ Automation rule   ◉ Enabled
    last fired: –
    when:
      entity: block
      event: created
    emit:
      - op: noop

The engine had already rejected that body. At the same second:

    WARN holon::api::holon_rule_watcher: [holon_rule_watcher]
    block:dogfood-rule-1 failed to parse: holon_rule YAML error:
    when: invalid type: map, expected a string at line 2 column 3

Then the body was replaced with deliberately broken YAML (`when: [unclosed`,
`entity: : block`, `emit ???`). The watcher logged the same parse failure
again — and the card was UNCHANGED: still "Automation rule", still "Enabled",
still "last fired: –", now with the malformed text underneath. Screenshots
`26-rule.png` (first body) and `27-rule-broken.png` (broken body).

So the only signal that a rule is dead is a WARN in a log the user never sees,
and the badge that is supposed to carry the status actively asserts the
opposite.

## Root cause

`assets/queries/holon_rule_discovery.sql` states the intended contract in its
own header: "a body that is not valid rule YAML surfaces a LOUD
`RuleStatus::ParseError` on the rule card". The parse does run and does fail —
`holon::api::holon_rule_watcher` logs it — but the resulting status is not what
the card renders. The card's badge is showing the block's enabled FLAG, not the
rule's parse status, so a rule can only ever read Enabled or Disabled and never
Broken.

Not diagnosed further; the two observations (WARN emitted, card unchanged)
bound the defect to the status→card leg.

## Missing piece

No invariant relates a rule block's parse outcome to what its card renders.
The keystone has no transition that authors a `holon_rule` at all, so neither
the happy path nor the malformed path is generated — and this vault contains
zero rule blocks, which is why the previous dogfood run could not reach the
surface either.

## Remedy

OPEN. Render `RuleStatus::ParseError` on the card with the parser's own message
(it is already precise — line and column), and treat "cannot parse" as a state
distinct from enabled/disabled. Add a keystone transition that authors a
`holon_rule` body drawn from valid and invalid YAML, with the invariant that a
card's status matches the watcher's parse outcome.

Note for whoever fixes this: the FIRST body above looks valid to a reader and
is not — `when:` wants a string, not a map. That the card said "Enabled" for it
is how the discrepancy was found at all.
