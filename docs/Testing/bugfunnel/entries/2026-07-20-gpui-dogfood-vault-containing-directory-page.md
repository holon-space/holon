---
id: 2026-07-20-gpui-dogfood-vault-containing-directory-page
date: 2026-07-20
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  GPUI dogfood: a vault containing a `Journals/` directory (or a page
  literally named "Journals") produces a SECOND, empty "Journals" `Page`
  (`block:24e2a8f4…`, no children) sitting beside the built-in journals shell
  (`block:journals`) — two identically-named entries in the sidebar. The
  `Journals/2026-07-20.org` date content was routed under the built-in shell,
  leaving the fixture-derived shell empty. A Logseq vault (which uses a
  `journals/` dir) hits this on import.
source_line: 1037
---

## Bug

GPUI dogfood: a vault containing a `Journals/` directory (or a page
literally named "Journals") produces a SECOND, empty "Journals" `Page`
(`block:24e2a8f4…`, no children) sitting beside the built-in journals shell
(`block:journals`) — two identically-named entries in the sidebar. The
`Journals/2026-07-20.org` date content was routed under the built-in shell,
leaving the fixture-derived shell empty. A Logseq vault (which uses a
`journals/` dir) hits this on import.

## Missing piece

Keystone seeds only the built-in journals shell; it never ingests a user
vault whose folder/page name collides with the reserved journals identity.
Needs a real-vault-import parity case + a journals-identity dedup/merge rule
(or reserved-name handling at ingest).

## Remedy

OPEN
