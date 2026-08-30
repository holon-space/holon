---
id: 2026-08-30-tag-with-separator-char-loses-whole-set-on-reboot
date: 2026-08-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A tag containing a comma is accepted and stored, then takes the block's ENTIRE
  tag set with it at the next boot — the org tag group stops parsing, so the
  re-ingest drops every tag while the write had reported success.
---

## Bug

Found by adversarial verification of the edge-array multiset detector
(task #10 lane), not by any test.

The reported observation — `block_tags` EMPTY for a block whose tag list
contained `"a,b"`, with the write having returned `Ok` — is **real and
reproduces**. What was wrong was only the mechanism first attributed to it
("the write lands zero tags"): measured immediately after settling, in both a
standalone harness and the reboot harness, the call lands all four tags
**including `"a,b"`**
(`[{"tag":"M"},{"tag":"a,b"},{"tag":"proj"},{"tag":"zzz"}]`). So the write does
not fail and is not rejected.

The set is destroyed one BOOT later — which is where the reported empty read
landed. Same write, same block, after a clean restart:

* comma-carrying set `["zzz","a,b","M"]` — boot-1 `block_tags` holds all three;
  boot-2 holds **nothing**.
* comma-free control `["proj"]` — boot-1 holds it; boot-2 **still** holds it.

So the separator is the variable, the loss is the whole set rather than the
offending tag, and it is silent: the write returned `Ok` and read back correct.

## Root cause

Tags are serialized into the org tag group `:M:a,b:proj:zzz:` (`Tags::to_org`,
crates/holon-api/src/types.rs). Org has **no escape** for that syntax — stated
outright at `split_headline_tags`
(crates/holon-org-format/src/parser.rs:556-559: "a trailing `:tag1:tag2:` group
is org TAG syntax, not title text (org has no escape for it)"). A comma is not
in the org tag grammar, so on re-ingest the trailing group stops being
recognized as a tag group **at all** and every tag in it is dropped — which is
why one bad tag costs the whole set.

`Tags::to_csv` / `Tags::from_csv` (same file) carry an independent instance of
the same hazard for the comma specifically: the CSV storage form would re-split
`"a,b"` into two tags.

The write path had no boundary check, so an unrepresentable tag was accepted,
stored, and lost later — priority 4 ("silently degrades to look fine") in the
Error Handling Philosophy.

## Missing piece

**COVERAGE.** The keystone's tag alphabet never generates a separator character
inside a tag, so no draw can reach the state. The oracle was NOT the weakness:
`block_compare` and the reference model would have flagged a tag-set difference
had one been generated. Nor would the sibling multiset detector have caught it —
after the loss the matview and the junction agree on *nothing*, so a
matview-vs-junction comparison is satisfied by both sides being empty. Only a
before/after-reboot comparison sees it.

## Remedy

Fail loud at the write boundary. `Tags::unrepresentable_char`
(crates/holon-api/src/types.rs) names the first character no tag serialization
can represent, and `reject_unrepresentable_tags`
(crates/holon-loro/src/loro_backend.rs) refuses the write with an error naming
the offending tag, the character, and the consequence.

Each rejected character fails DIFFERENTLY, and the doc claims only what a test
demonstrates:

* `,` — outside the org tag grammar: the whole group stops parsing and the block
  loses EVERY tag (the measured reboot loss above). Also the CSV storage
  separator, which splits it independently
  (`a_comma_in_a_tag_splits_the_csv_storage_form`).
* `:` — the org tag-group separator itself, so the tag SPLITS rather than
  vanishing: `:a:b:proj:` reads back as three tags where two were written
  (`a_colon_in_a_tag_splits_the_org_tag_group`). Silent corruption of a
  different shape; rejection stands on its own evidence, not by analogy to `,`.
* whitespace — terminates the trailing tag group in org headline syntax.

Wired into **all three** Loro tag write paths:

1. `set_block_tags`,
2. the generic `set_block_edge_field` when the key is the `tags` column,
3. `write_new_node` — the CREATE path. This one was missed on the first pass and
   is the reason the fix needed a second round: creates reach `tags` through
   `BlockEdges`, never through the setters, so `create_block_with_properties`
   (and `add_subtask`'s create params, which is user-reachable) could still
   store an unrepresentable tag. The guard sits in `write_new_node` rather than
   at its two call sites because it is the sole writer of a new node's meta, so
   a future third caller inherits it.

Deliberately narrow: only characters with a demonstrated failure are rejected.
The org tag spec is stricter (`[[:alnum:]_@#%]`), but tags like `some-tag` round
trip today, so enforcing the full spec would refuse writes that currently work.

Pinned by `crates/holon-integration-tests/tests/store_suite/tag_with_comma_drops_whole_set.rs`:

* `tags_without_a_comma_all_land` — the control, so a failure cannot be read as
  "tags never land in this harness".
* `a_comma_carrying_tag_must_not_silently_drop_the_whole_set` — requires the
  REFUSAL (rejection is the contract now, so an accept-or-store disjunction
  would pass with and without the fix), and requires the refused write to leave
  the block's prior tags untouched rather than half-applying.
* `creating_a_block_with_a_separator_carrying_tag_is_refused` — the create path,
  which bypasses the setters entirely.
* `a_separator_carrying_tag_must_not_lose_the_set_across_a_reboot` — the actual
  defect: tags must not change across a restart that writes nothing, and the
  block must still carry a NON-empty set (without that second assertion,
  "accepted then lost" and "refused then empty" would both satisfy an equality
  between two empties).

Plus, in `crates/holon-api/src/types.rs`,
`unrepresentable_char_names_every_separator_and_nothing_else` (every rejected
char, and the accepted ones — `some-tag`, `@home`, `tag#1` — that must keep
working) and the two split demonstrations named above.

Red for the right reason with the guard disabled at its single point
(`scratchpad/hunt/store_d4red.log`, generated against the landed file): all
THREE guard legs fail — the setter refusal, the create refusal, and the reboot
loss — while the control and the matview reboot test stay green.

```
the block's tags changed across a reboot with nothing writing to them …
(before the reboot the block held ["M", "a,b", "zzz"])
  left: []
 right: ["M", "a,b", "zzz"]
```

Green with it restored (`scratchpad/hunt/store_d4v3.log`, 5/5).

### Residual

Only the three Loro write paths are guarded, and that a whole write path was
missed on the first pass is itself the argument for the durable fix: checking at
each write site is exactly the "a new call site defaults the check away" hazard
that `EdgeField::ALL` exists to prevent elsewhere. The durable answer is to make
the illegal state unrepresentable — parse tags into a validated newtype at
construction, so no write path can hold an invalid `Tags` at all. That is a
`Tags` API change with call-site fallout across crates and is queued, not done.

Until then, a tag reaching `block_tags` by a non-Loro route — a direct SQL
insert, or an org file authored with a tag the parser accepts but the renderer
re-emits differently — is still uncovered.
