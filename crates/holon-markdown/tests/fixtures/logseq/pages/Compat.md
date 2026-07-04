title:: Compat
tags:: compat, holon
type:: test page

- This page exercises the LogSeq markdown flavor for Holon ingest.
  id:: 11111111-0000-4000-8000-000000000001
- A block with [[Page Link]] and a tag #compat and #[[multi word tag]].
- TODO a task block with a scheduled date
  SCHEDULED: <2026-07-15 Wed>
- DONE finished task
  :LOGBOOK:
  CLOCK: [2026-07-11 Sat 10:00:00]--[2026-07-11 Sat 10:30:00] =>  00:30:00
  :END:
- A parent block
  - a child referencing ((11111111-0000-4000-8000-000000000001))
    - a grandchild with a property
      priority:: high
  - another child, collapsed
    collapsed:: true
- A **bold** and *italic* and `code` block.
- {{query (todo TODO)}}
