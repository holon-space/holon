SELECT
    br.block_id    AS block_id,
    br.required_id AS required_id,
    b.content      AS required_content
FROM block_requires br
JOIN block b ON b.id = br.required_id
