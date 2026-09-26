---
id: 2026-10-01-dense-patch-applies-special-drawer-keys-and-export-keywords
date: 2026-10-01
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  dense_patch applied a drawer key org computes (ITEM, CLOSED, FILE, blocked,
  ALLTAGS, ...) and a body line org reads for the whole file (#+AUTHOR,
  #+MACRO, #+EXCLUDE_TAGS, #+BIND, #+LANGUAGE, ...).
---

## Bug
Found by the rb4 verifier of the decision Inc 6 lane. The store held the
property or body line; org did not read it back as written.

## Root cause
Org ignores a drawer line whose key is one of the 14 org-special-properties,
in any case (scratch probes `sp.el`, `sp2.el`). Org reads TITLE, DATE, AUTHOR,
EMAIL, LANGUAGE, SELECT_TAGS, EXCLUDE_TAGS, CREATOR, CITE_EXPORT, MACRO and
BIND from a body line for the whole file (`ex.el`, `ex2.el`).
`declares_page_keyword` knew none of the export keywords, and no check knew
the special keys.

## Missing piece
The engine oracle `org_reads_otherwise` knew only TITLE, FILETAGS, CATEGORY
and PROPERTY.

## Remedy
`ORG_SPECIAL_PROPERTIES` refusal in `refuse_inexact`
(`frontends/mcp/src/dense_patch.rs:247`); the export keywords in
`declares_page_keyword` (`crates/holon-org-format/src/page_keywords.rs:31`);
the oracle mirrors both. Backend-only keywords (DESCRIPTION, KEYWORDS,
SUBTITLE, HTML_HEAD, LATEX_HEADER, ...) change only their export backend and
stay applied.
