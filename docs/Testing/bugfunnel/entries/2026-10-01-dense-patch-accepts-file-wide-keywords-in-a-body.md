---
id: 2026-10-01-dense-patch-accepts-file-wide-keywords-in-a-body
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  dense_patch accepted `#+FILETAGS:`, `#+CATEGORY:`, `#+PROPERTY:` and other
  buffer settings in a row body, which org reads for the whole file.
---

## Bug
Found by the Inc 6 rebase verifier (`lane-logs/inc6rb2v-verify.md`, item 2),
lane decision Inc 6. A body line such as `#+FILETAGS: :x:` applied silently.

## Root cause
`declares_page_keyword` (`crates/holon-org-format/src/page_keywords.rs:30`)
listed only TITLE, the task keyword settings and the page id. Measured with
`emacs -Q --batch` (30.2): a `#+FILETAGS:`, `#+CATEGORY:` or `#+PROPERTY:`
line under a headline sets the tags, category and property of the first
headline of the file too, so org reads them file-wide, not only in the
preamble.

## Missing piece
The generator's page keyword shape drew only TITLE and TODO lines.

## Remedy
`declares_page_keyword` lists the buffer settings `org-set-regexps-and-options`
collects (plus SETUPFILE); such a line in a body is refused by row name.
`#+INCLUDE:` stays accepted: org reads it only at export. Pinned by
`a_text_org_writes_otherwise_is_refused_before_any_write`.
