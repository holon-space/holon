-- Resolves focus targets to the navigated-to block itself (not its children).
-- Each open `navigation_history` row WITH a non-null `block_id` produces one
-- matview row. Main-panel region has at most one such row (focus_replace
-- closes the prior); right sidebar can have many (one per pinned page).
-- The GQL/SQL query then uses CHILD_OF*0..N from `root_id` to include the
-- focus block + its descendants.
--
-- `closed_at IS NULL` means "open"; UPDATE flipping closed_at to a timestamp
-- removes the row from this matview (Turso IVM CDC fires a Delete-shaped
-- event downstream).
--
-- `block_id IS NOT NULL` excludes home rows (`NavigateHome` inserts a
-- `block_id = NULL` row to record "I'm at the region's home"). Home rows
-- live only in `navigation_history` — consumers of `focus_roots` (the
-- main panel's GQL, the right sidebar's tree, the PBT LiveData mirror)
-- want no row when focus is home, so the panel falls through to its
-- default render. Filtering at the matview level here is correct as of
-- nightscape@holon `aff40a84` (the IVM fix that makes alias+compound
-- WHERE projections drop NULL rows).
--
-- `history_id` is exposed so the sidebar X button has a targetable handle
-- — `close(history_id)` flips closed_at on exactly that row.
SELECT
    region,
    block_id AS root_id,
    timestamp AS added_ts,
    id AS history_id
FROM navigation_history
WHERE closed_at IS NULL AND block_id IS NOT NULL
