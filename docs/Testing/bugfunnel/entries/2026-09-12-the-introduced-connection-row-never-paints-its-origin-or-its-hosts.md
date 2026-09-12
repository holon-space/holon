---
id: 2026-09-12-the-introduced-connection-row-never-paints-its-origin-or-its-hosts
date: 2026-09-12
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  IntegrationRow carries origin and hosts for an introduced connection and no
  rendering code reads either field, so the Settings row a user enables looks
  exactly like a bundled one — the disclosure the threat model rests on never
  reaches the screen.
---

## Bug

Found by the `dogfood-explorer` gate for `user-connections` (main
`f134df9ece6c`), which is the gate the lane's own report names as the one
remaining step.

The lane's Increment 5 calls this the LANDING CONDITION, in its own words: "The
threat model rests on enabling being a deliberate act WITH a disclosure, and a
row showing only a name and an icon carries none — both are the file's choice,
so a hostile connection can call itself 'Calendar', wear a calendar glyph and
ask for the same click as a bundled one."

Driven live: an introduced connection `fixturebox`, whose manual calls
`127.0.0.1:8791`, was dropped into the sandbox integrations directory and the
Settings modal was opened. Its row reads

```
fixturebox | unconfigured | Pending | [toggle] | Switch integration  Open
```

— name, config status, status, toggle, operations. No origin path. No hosts.
Character for character the same columns as the bundled `claude-history` and
`jsonplaceholder` rows directly above and below it. Screenshot
`scratchpad/dogfood-uc/shots/03-settings-scrolled.png`.

So the row still shows only a name and an icon the file chose. The condition the
increment was declared against is not met in the product, even though the
increment's tests are green.

## Root cause

`crates/holon-app/src/integrations_settings.rs:85-92` declares `origin:
Option<String>` and `hosts: Vec<String>` on `IntegrationRow`, and lines 310-311
and 378-379 populate them correctly — that half works, and its test proves it.

The fields have **no consumer**. Searching every `.origin` / `.hosts` use across
`crates/holon-app`, `crates/holon-gpui` and `frontends/gpui` returns the
populating lines plus `crates/holon-app/tests/introduced_connection_row_disclosure.rs`
and nothing else. No view model reads them, no shadow builder emits them, no
widget paints them.

This is precisely the hazard the same increment documented for a different
field. Its report says of `secret_stored`: "Three separate places had to learn
the field ... A field added to one and not the others reaches the row data and
silently never reaches the screen — exactly what happened twice while building
this, and only the windowed test caught it." `origin` and `hosts` were added to
the first place only, and no windowed test was written for them, so nothing
caught it.

## Missing piece

COVERAGE. `crates/holon-app/tests/introduced_connection_row_disclosure.rs`
asserts on the `IntegrationRow` struct, which is the data, not the disclosure. A
disclosure is a claim about what a user SEES, and the only harness that can
answer that is the windowed one.

None of the eight windowed integration tests under `frontends/gpui/tests/`
(`settings_integrations_table_windowed.rs`,
`settings_integrations_ops_windowed.rs`,
`integrations_row_narrow_window_windowed.rs` and the rest) puts an INSTALLED
connection in its fixture — every one uses bundled providers. So the windowed
harness has never been able to generate the state, which is what makes this
COVERAGE rather than ORACLE: the assertion could not have been written against a
row that never existed in the fixture.

Secondary ORACLE: once such a fixture exists, the missing assertion is the
ordinary one — the painted texts of an introduced row must contain its origin
path and each of its hosts, and those of a bundled row must contain neither.

The keystone PBT cannot reproduce this; it does not paint.

## Remedy

FIXED in lane `uc-fixes` (wave 12).

The remedy sketched above named the wrong three files, and that is worth
recording: `origin`/`hosts` are not preference-row fields, so
`preferences_to_rows`, the `reactive_view_model.rs` allowlist and
`shadow_builders/pref_field.rs` are not on this path at all. The Settings
integrations list is a `live_query` over the `integration_state` mirror, so the
fields had to travel a different route:

- `crates/holon-turso/sql/schema/integration_state.sql` — two columns, `origin`
  and `hosts`, both `NOT NULL DEFAULT ''`; the existing additive migration in
  `schema_modules.rs` gained them (and is renamed
  `add_missing_integration_state_columns`, since it is no longer only about
  presentation).
- `crates/holon-app/src/integration_projection.rs` — `TABLE_COLUMNS` and the
  upsert write both, `hosts` as the `", "`-joined list.
- `crates/holon-app/src/integrations_section.rs` — `SETTINGS_SQL` selects them
  and the Integration cell paints them under the name, behind an
  `if_col("origin", "")` so a bundled row is unchanged.

Two rungs, because the row-data test stayed green throughout and that is why the
defect escaped. Headless (the mirror):
`crates/holon-app/tests/introduced_connection_disclosure_reaches_the_mirror.rs`
— red `lane-logs/item2a-RED-1789178795.log` (`no such column: origin`), green
`lane-logs/item2a-GREEN-1789178950.log`. Windowed (the paint):
`frontends/gpui/tests/settings_introduced_connection_disclosure_windowed.rs` —
red `lane-logs/item2b-RED-1789179292.log`, where the modal demonstrably opened
(it painted "Status", "Setup" and all six provider names) and no origin text
appeared; green `lane-logs/item2b-GREEN-1789179506.log`.

## Attribution

REGRESSION of `user-connections`: the fields, the row and the landing condition
are all this lane's, and the disclosure never worked. Nothing about it predates
`a5e161c0`.

## Note for the layout lane

The disclosure adds two muted lines inside the Integration cell of an introduced
row, and long paths will wrap. That interacts with
`2026-09-12-the-settings-integrations-table-wraps-mid-word-and-overflows-its-modal`,
which lane `sidecar-sync-fixes` owns. Truncating the path was rejected here: a
disclosure the user cannot read in full is not one.
