-- Q2 transitions-by-transition: op fire counts grouped by the firing
-- transition id (VisionGapAnalysis C2b, ADR 0024 P8). One row per
-- transition_id: `ops` = distinct op groups that transition fired, `events` =
-- field-delta rows. NULL transition_id = non-rule-origin ops (user/agent).
-- The "which rules fired, how often" view — the counting primitive behind
-- "postponed N times" generalized across every transition.
--
-- Raw SQL over block_history is sanctioned (Martin's ruling 2026-07-11).
SELECT
    transition_id,
    COUNT(DISTINCT op_group) AS ops,
    COUNT(*) AS events
FROM block_history
GROUP BY transition_id
ORDER BY transition_id;
