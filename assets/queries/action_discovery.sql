-- Turso IVM doesn't support IN(...) filters, so use OR instead.
SELECT
    action_src.id AS action_id,
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
    AND action_src.source_language = 'action'
