-- automations_journal: the user-facing automation journal (ADR 0024 P8) —
-- effects grouped by (origin, transition_id, day). IVM-maintained over the
-- `block_history` base TABLE (not a matview-on-matview), so the grouped counts
-- update O(delta) as history rows are appended. Proven by the R2 spike
-- (group_by_matview_spike.rs) and the CDC test (automations_journal_cdc.rs).
--
-- `effect_count` is the number of recorded op/field deltas in the group; group
-- by day makes "Daily journal — created 2026-07-10 ⚙" a single read. Kept to
-- COUNT only (no MAX/MIN) to stay inside the aggregate class the spike proved
-- IVM maintains under retraction.
SELECT
    origin,
    transition_id,
    day,
    COUNT(*) AS effect_count
FROM block_history
GROUP BY origin, transition_id, day
