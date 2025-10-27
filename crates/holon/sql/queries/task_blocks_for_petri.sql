SELECT
    id, parent_id, content, content_type, source_language,
    source_name, properties, created_at, updated_at
FROM block
WHERE json_extract(properties, '$.task_state') IS NOT NULL
   OR json_extract(properties, '$.prototype_for') IS NOT NULL
   OR json_extract(properties, '$.is_self') = true
ORDER BY parent_id, sort_key
