-- Q1 supervision: per-session / per-tool-call op counts over block_history
-- (VisionGapAnalysis C2b, ADR 0024 P8). One row per driving agent
-- (session_id, tool_call_id): `ops` = distinct op groups (one op = one group),
-- `events` = field-delta rows. User/rule-origin ops (NULL session) fold into
-- their own NULL-keyed row. The "what did each agent session actually do"
-- supervision view — one query, no per-tool bookkeeping.
--
-- Raw SQL over block_history is sanctioned (Martin's ruling 2026-07-11): the
-- relation is a disclosed ephemeral cache exposed as a plain SQL table.
SELECT
    session_id,
    tool_call_id,
    COUNT(DISTINCT op_group) AS ops,
    COUNT(*) AS events
FROM block_history
GROUP BY session_id, tool_call_id
ORDER BY session_id, tool_call_id;
