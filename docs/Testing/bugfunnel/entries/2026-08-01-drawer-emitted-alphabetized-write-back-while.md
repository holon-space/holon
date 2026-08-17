---
id: 2026-08-01-drawer-emitted-alphabetized-write-back-while
date: 2026-08-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  Every `:PROPERTIES:` drawer is re-emitted ALPHABETIZED on write-back while
  the file on disk holds the author's insertion order, so ingest→write-back
  churns lines for no semantic gain. Concretely on
  `Agents/citrix/citrix-STX.BROWSER_AGENT.org`, authored `:STATE: :PROJECT:
  :SOURCE: :UPDATED-AT: :NOTE:` comes back as `:NOTE: :PROJECT: :SOURCE:
  :STATE: :UPDATED-AT:` — 3 changed lines. This was the SINGLE remaining
  offender in task #67's vault byte-stability acceptance, which allowlisted
  the file (`KNOWN_PRE_EXISTING_CHURN` in
  `crates/holon-org-format/tests/vault_writeback_stability.rs`) after
  A/B-proving it pre-existing. Key order dies in four places on the
  parse→render path, all in `holon-org-format`: `extract_properties`
  (`parser.rs`) returned a `HashMap`; `OrgBlockExt::drawer_properties`
  (`models.rs`) returns a `HashMap`; and BOTH `prepare_block_for_org`
  (`org_renderer.rs`) and
  `format_properties_drawer`/`format_properties_drawer_without_id`
  (`models.rs`) then sorted explicitly. NOT a storage-representation defect:
  the drawer is rendered exclusively from the `org_properties` JSON STRING,
  which is one opaque value in the existing `properties` bucket (a
  `serde_json::Map` with `preserve_order`, i.e. an IndexMap), and that string
  is persisted verbatim as one key of the SQL `properties` JSON blob and one
  key of the Loro `PROPERTIES_MAP` — so order can be carried end-to-end with
  NO schema change.
source_line: 787
---

## Bug

(task #88) Every `:PROPERTIES:` drawer is re-emitted ALPHABETIZED on
write-back while the file on disk holds the author's insertion order, so
ingest→write-back churns lines for no semantic gain. Concretely on
`Agents/citrix/citrix-STX.BROWSER_AGENT.org`, authored `:STATE: :PROJECT:
:SOURCE: :UPDATED-AT: :NOTE:` comes back as `:NOTE: :PROJECT: :SOURCE:
:STATE: :UPDATED-AT:` — 3 changed lines. This was the SINGLE remaining
offender in task #67's vault byte-stability acceptance, which allowlisted
the file (`KNOWN_PRE_EXISTING_CHURN` in
`crates/holon-org-format/tests/vault_writeback_stability.rs`) after
A/B-proving it pre-existing. Key order dies in four places on the
parse→render path, all in `holon-org-format`: `extract_properties`
(`parser.rs`) returned a `HashMap`; `OrgBlockExt::drawer_properties`
(`models.rs`) returns a `HashMap`; and BOTH `prepare_block_for_org`
(`org_renderer.rs`) and
`format_properties_drawer`/`format_properties_drawer_without_id`
(`models.rs`) then sorted explicitly. NOT a storage-representation defect:
the drawer is rendered exclusively from the `org_properties` JSON STRING,
which is one opaque value in the existing `properties` bucket (a
`serde_json::Map` with `preserve_order`, i.e. an IndexMap), and that string
is persisted verbatim as one key of the SQL `properties` JSON blob and one
key of the Loro `PROPERTIES_MAP` — so order can be carried end-to-end with
NO schema change.

## Missing piece

The START STATE of every org round-trip PBT, not its alphabet.
`round_trip_pbt.rs`'s `BlockMutation::SetDrawerProperty` DOES generate
multi-key drawers, so drawer coverage exists — but the properties all begin
from a SYNTHESIZED `Block` and the assertion is that render→parse→render is
a fixed point. A renderer that NORMALIZES satisfies that by construction:
its own first render establishes exactly the order the second render
reproduces, so no amount of generated drawer content can expose a
normalization the author never chose. Only a DISK-FIRST property (authored
bytes → parse → render → compare bytes) can see it, and the sole disk-first
lane in the tree was `vault_writeback_stability.rs`, which is `#[ignore]`d
and needs a real vault to run.

## Remedy

FIXED 2026-08-01 — the authored key order is now recorded at the parse
boundary (`_drawer_order`, a JSON array of the drawer keys in file order,
underscore-prefixed so it is already excluded from the drawer it describes
by `drawer_properties`' existing `!k.starts_with('_')` filter) and replayed
by `prepare_block_for_org`, which ranks `drawer_properties()` by that
sequence and appends never-authored keys alphabetically; the two explicit
sorts in `format_properties_drawer*` are deleted so the `org_properties`
IndexMap's own order reaches the drawer. `:BLOCKED-BY:` folds onto the
canonical `:REQUIRES:` spelling when the order is recorded, so a lifted edge
key keeps the slot it was authored in. New disk-first characterization suite
`crates/holon-org-format/tests/drawer_key_order_stability.rs` (6 cases: the
citrix shape, strictly-descending keys, `:ID:` hoisted first, two-pass
idempotence, lifted `:REQUIRES:`/`:COLLAPSED:` keeping their authored slot,
and case-distinct keys). Red-first proof: 4/5 red on the unmodified tree
with the exact alphabetization diff (`:STATE: :PROJECT: :SOURCE:
:UPDATED-AT: :NOTE:` written back as `:NOTE: :PROJECT: :SOURCE: :STATE:
:UPDATED-AT:`); the 5th (idempotence) passes on base because sorting twice
is stable, and is kept as a drift guard. ACCEPTANCE:
`vault_writeback_stability` over all 102 real-vault files with
`KNOWN_PRE_EXISTING_CHURN` now EMPTY — 0 unstable, 0 changed lines. The
first attempt at the fix regressed a SECOND file (`Projects/Holon/Now.org`,
4 lines): that headline carries BOTH `:Effort:` and `:effort:` straddling a
`:REQUIRES:`, and an `eq_ignore_ascii_case` dedupe of the recorded order
collapsed them so the lowercase key inherited the uppercase key's slot and
displaced `:REQUIRES:`. Order recording now dedupes on EXACT spelling and
the renderer's rank probes exact-first, case-insensitive only after — which
is what lets a lifted `:collapsed:` still find the slot it was authored in
while two case-distinct custom keys keep their own.
