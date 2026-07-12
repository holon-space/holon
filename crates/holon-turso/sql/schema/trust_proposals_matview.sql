SELECT
    id,
    json_extract(properties, '$._proposal.status') AS status,
    json_extract(properties, '$._proposal.entity') AS entity,
    json_extract(properties, '$._proposal.op') AS op,
    json_extract(properties, '$._provenance.origin') AS origin,
    json_extract(properties, '$._provenance.transition_id') AS transition_id,
    json_extract(properties, '$._provenance.session_id') AS session_id,
    json_extract(properties, '$._provenance.tool_call_id') AS tool_call_id,
    json_extract(properties, '$._provenance.at_millis') AS proposed_at_millis,
    content
FROM block_raw
WHERE json_extract(properties, '$._proposal') IS NOT NULL
