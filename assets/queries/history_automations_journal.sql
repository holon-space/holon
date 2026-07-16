-- Q3 automations journal: the user-facing automation journal (ADR 0024 P8),
-- read from the IVM-maintained `automations_journal` matview (grouped by
-- origin, transition_id, day over the block_history base table), not raw
-- block_history. "Daily journal — created 2026-07-10 ⚙" is this query, ordered
-- newest day first.
--
-- The matview itself is boot-owned by `AutomationsJournalSchemaModule`
-- (holon-turso); this file is the thin read over it, kept as data alongside
-- the rest of the C2b query pack.
SELECT
    origin,
    transition_id,
    day,
    effect_count
FROM automations_journal
ORDER BY day DESC, origin, transition_id;
