-- Q5 forensic per-block timeline: the complete, ordered op/effect history of
-- ONE block (bound to the block_id parameter), oldest to newest by append seq
-- (VisionGapAnalysis C2b, ADR 0024 P8). The forensic "why does this
-- block have this state, and who did it" view — every field delta with its
-- provenance (origin, firing transition, driving agent session/tool-call,
-- effect id) in causal order.
--
-- Raw SQL over block_history is sanctioned (Martin's ruling 2026-07-11).
SELECT
    seq,
    at_millis,
    day,
    op_group,
    op_name,
    field,
    old_value,
    new_value,
    origin,
    transition_id,
    session_id,
    tool_call_id,
    effect_id
FROM block_history
WHERE block_id = $block_id
ORDER BY seq ASC;
