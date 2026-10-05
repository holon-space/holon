---
id: 2026-10-05-shopping-app-add-item-matches-by-name-alone
date: 2026-10-05
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  That Shopping List's `addItem` finds an existing entry by normalized name
  alone, so a Holon push of (X, catA) replaces the app's (X, catB), including
  its amount, while Holon identifies an item by (name, cat).
---

## Bug
Found by an agent reading the app's own client code (read-only) on 2026-10-05,
in the `shopping-count-pull` lane, while deciding what a duplicate active
`(name, cat)` entry means. Not seen in Martin's live list. Record only: no fix
in this round.

## Root cause
`/Applications/That Shopping List.app/Wrapper/That Shopping List.app/www/js/list.js`
`addItem` (line 1290) calls `thatlist.util.indexOfItem(listItem.name, this.items)`
(line 1325). `indexOfItem` (`www/js/utils.js:2163`) compares `curItem.name`
alone, raw or through `normalizeStr`; the category plays no part. A match is
removed (`splice`) or overwritten (`this.items[targetIdx] = listItem`), and the
new entry is the one the add carried, so its `cat` and `count` replace the old
entry's.

Holon's identity is `(name, cat)` (`assets/integrations/shopping.yaml`
`list_sync.key`). The write leg sends `good: {name, cat, new: true}` with no
amount. So a Holon-created (X, catA) item that the app already holds as
(X, catB) with an amount replaces that entry: the category changes and the
amount is lost. The next pull then shows (X, catA) with no amount, and Holon's
row for (X, catB), if any, reads as deleted.

Related: the same name-only rule is why two active entries of one `(name, cat)`
cannot be told apart by the peer; the pull keeps the first and reports the rest
(`shopping_pull_mock.rs` `a_duplicate_active_entry_is_refused_by_name_and_the_rest_syncs`).

## Missing piece
ENVIRONMENT: the remote-list fixture peer the keystone uses
(`crates/holon-integration-tests/src/pbt/remote_list_fixture.rs`) keys items by
a `[.label, .bucket]` pair (line 70), the analogue of Holon's `(name, cat)`. The real peer keys them by name, so the fixture
cannot produce this collision.

## Remedy
Open. Candidates, not decided: make the fixture peer replace by normalized name
alone (so the keystone can generate the collision), and make Holon refuse or
rename a local (X, catA) create when the pulled list holds X under another
category.
