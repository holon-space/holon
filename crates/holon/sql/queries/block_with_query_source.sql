-- Loads a block together with its query source child and optional render source sibling.
-- The {query_langs} placeholder is filled at compile time with QueryLanguage::sql_in_list().
SELECT
    b.id,
    b.parent_id,
    b.content,
    b.properties,
    query_src.content AS query_source,
    query_src.source_language AS query_language,
    render_src.content AS render_source
FROM block b
INNER JOIN block query_src ON query_src.parent_id = b.id
    AND query_src.content_type = 'source'
    AND query_src.source_language IN {query_langs}
LEFT JOIN block render_src ON render_src.parent_id = b.id
    AND render_src.content_type = 'source'
    AND render_src.source_language = 'render'
WHERE b.id = $block_id
LIMIT 1
