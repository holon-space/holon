-- Turso IVM doesn't support IN(...) filters, so use OR instead.
-- Rule heads use the `holon_rule` language (ADR 0024). The retired `action`
-- language is still matched so the watcher can surface a LOUD deprecation status
-- (RuleStatus::DeprecatedLanguage) rather than let the block go silently inert —
-- it is NOT executed. `action_language` lets the watcher tell the two apart.
SELECT
    action_src.id AS action_id,
    action_src.source_language AS action_language,
    query_src.content AS query_source,
    query_src.source_language AS query_language,
    action_src.content AS action_source
FROM block action_src
INNER JOIN block query_src ON query_src.parent_id = action_src.parent_id
    AND query_src.content_type = 'source'
    AND (query_src.source_language = 'holon_prql'
         OR query_src.source_language = 'holon_gql'
         OR query_src.source_language = 'holon_sql')
WHERE action_src.content_type = 'source'
    AND (action_src.source_language = 'holon_rule'
         OR action_src.source_language = 'action')
