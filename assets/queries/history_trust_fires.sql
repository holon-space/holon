-- Q4 trust stats x history fires: the C5 acceptance stats per proposer
-- (origin, transition_id) LEFT JOINed with how many ops that same
-- (origin, transition_id) actually fired in block_history. "Proposed vs did":
-- the supervision payoff of C2b (history) crossed with C5 (trust proposals).
--
-- `stats` mirrors TRUST_PROPOSAL_STATS_SQL (schema_modules.rs) but keyed by
-- (origin, transition_id) only. `fires` counts distinct op groups per proposer
-- in block_history. The join handles NULL transition_id explicitly (SQL `=`
-- never matches NULL), so non-rule proposers line up with their non-rule ops.
--
-- Raw SQL over block_history is sanctioned (Martin's ruling 2026-07-11).
WITH stats AS (
    SELECT
        origin,
        transition_id,
        COUNT(*) AS proposals,
        SUM(CASE WHEN status = 'accepted' THEN 1 ELSE 0 END) AS accepted,
        SUM(CASE WHEN status = 'rejected' THEN 1 ELSE 0 END) AS rejected,
        SUM(CASE WHEN status = 'pending' THEN 1 ELSE 0 END) AS pending
    FROM trust_proposals
    GROUP BY origin, transition_id
),
fires AS (
    SELECT
        origin,
        transition_id,
        COUNT(DISTINCT op_group) AS fired_ops
    FROM block_history
    GROUP BY origin, transition_id
)
SELECT
    stats.origin,
    stats.transition_id,
    stats.proposals,
    stats.accepted,
    stats.rejected,
    stats.pending,
    COALESCE(fires.fired_ops, 0) AS fired_ops
FROM stats
LEFT JOIN fires
    ON fires.origin = stats.origin
    AND (fires.transition_id = stats.transition_id
         OR (fires.transition_id IS NULL AND stats.transition_id IS NULL))
ORDER BY stats.origin, stats.transition_id;
