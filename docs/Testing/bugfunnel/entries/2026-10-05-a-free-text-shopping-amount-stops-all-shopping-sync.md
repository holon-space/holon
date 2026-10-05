---
id: 2026-10-05-a-free-text-shopping-amount-stops-all-shopping-sync
date: 2026-10-05
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  The shopping pull refuses any `count` that is not a JSON number, but That
  Shopping List stores amounts as free text ("2 kg", "500g"), so the first item
  with an amount stops every shopping sync round.
---

## Bug
Found by an agent reading code on 2026-10-05, while planning declared
operations (`~/.claude/plans/declared-ops-plan.md`, finding F1). The app's own
client code (`www/js/utils.js` `numberRangeOfString`, `adjustCountByDelta`;
`app.js` `finalizeSheetItemCount`) writes `count` as a free-text string, and a
picked item carries its amount as `pickedCount`
(`docs/Plans/ThatShoppingList-API-2026-09-01.md`). Not yet seen in Martin's
live list; the first item he gives an amount would trigger it.

## Root cause
`assets/integrations/shopping.yaml` `pull_list.response`:
`def count_of: if . == null then null elif type == "number" then . else
error("`count` must be a number") end;` — the whole response is refused, so the
round pulls nothing and pushes nothing. The column was `shopping_item.count
REAL`, and the duplicate fold summed counts as numbers. A picked item's
`pickedCount` was never read.

Red: `crates/holon-app/tests/shopping_pull_mock.rs`
`free_text_amounts_are_mirrored_verbatim` — the pull fails with
"`count` must be a number".

## Missing piece
COVERAGE: the keystone's remote-list fixture
(`crates/holon-integration-tests/src/pbt/remote_list_fixture.rs`) never sent
a free-text amount; its only merge column was a whole number. ORACLE: the
mapping's differential test (`crates/holon-kitchen/tests/shopping_mapping_differential.rs`)
generated `count: "2"` but its model refused it too, so the wrong refusal was
pinned as correct.

## Remedy
The mapping keeps `count` as opaque text, byte for byte (a JSON number becomes
its decimal text), reads `pickedCount` for a picked item, and no longer adds
amounts on the fold: an active entry's amount wins over a picked one, and a
second active entry for one `(name, cat)` is skipped and reported by name (the
peer names an item by its name); the first by list position is kept and the
rest of the list still syncs. `shopping_item.count` is `TEXT`. The push leg sends `count` on
add when the block has one (`commit.request` in `shopping.yaml` builds
`good: {name, cat, new}` plus `count`); see
`2026-10-05-an-amount-authored-in-holon-is-nulled-by-the-next-sync.md`. The remote-list fixture now carries a free-text
`amount` merge column that the keystone generator fills with list-app
strings, and the differential model accepts text amounts and `pickedCount`.

Open: a database created before this change keeps `count REAL`, because a
declared type's table is created with `CREATE TABLE IF NOT EXISTS`
(`crates/holon-turso/src/dynamic_schema_module.rs`) and nothing changes an
existing column's type.
