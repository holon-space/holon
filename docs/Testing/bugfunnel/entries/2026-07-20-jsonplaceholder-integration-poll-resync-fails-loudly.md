---
id: 2026-07-20-jsonplaceholder-integration-poll-resync-fails-loudly
date: 2026-07-20
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  jsonplaceholder integration poll-resync fails loudly & repeatedly: `WARN
  holon_mcp_client::mcp_integration: poll resync failed error=Batch
  transaction failed: Database error: Failed to execute statement: datatype
  mismatch` for entity `jp_posts`, on every integration poll loop. Disclosed
  loudly (good) but the sync is broken.
source_line: 1028
---

## Bug

jsonplaceholder integration poll-resync fails loudly & repeatedly: `WARN
holon_mcp_client::mcp_integration: poll resync failed error=Batch
transaction failed: Database error: Failed to execute statement: datatype
mismatch` for entity `jp_posts`, on every integration poll loop. Disclosed
loudly (good) but the sync is broken.

## Missing piece

integration resync path (`docs/integrations/jsonplaceholder.yaml` → batch
upsert) is not exercised by any automated test with a real
datatype-heterogeneous payload; the mismatch (JSON number vs declared column
type) escapes to the live poll loop.

## Remedy

OPEN — GPUI dogfood 2026-07-20 (P3)
