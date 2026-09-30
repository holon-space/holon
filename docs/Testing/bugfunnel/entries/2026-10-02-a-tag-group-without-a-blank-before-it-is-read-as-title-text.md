---
id: 2026-10-02-a-tag-group-without-a-blank-before-it-is-read-as-title-text
date: 2026-10-02
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Holon and the orgize fork read `* ab:cd:` as the title `ab:cd:` with no
  tags; org reads the title `ab` and the tag `cd`.
---

## Bug
Found by the org-faithful group B lane (round 18) when it compared Holon's
headline split with `emacs -Q` 30.2 / org 9.7.11. The fixture
`crates/holon-org-format/tests/emacs/headline_tags_org_9_7_11.{el,txt}` holds
9819 headline lines and org's reading of each. On the base code 1712 rows
differed (`lane-logs/groupB-r18-red-holon-base.log`), and the fork's parser
differed on 342 rows (`lane-logs/groupB-r18b-fork-red-headline.log`).
Other shapes of the same rule: `x :a: :b:` (org: title `x :a:`, tag `b`),
`x :::` (org: title `x`, an empty tag group).

## Root cause
Org's headline parser (`org-element--headline-parse-title`, org-element.el)
reads the tags with `\(:[[:alnum:]_@#%:]+:\)[ \t]*$`, searched forward from
the title start. The leftmost match needs no blank before it and holds no
blank inside. Holon (`Tags::split_org_headline`) and the fork
(`src/syntax/headline.rs` `headline_tags_node`) needed a blank before the
group and accepted blank items in it. Two Holon tests stated the wrong rule
(`properties_prefix_headline_repro.rs` `a:b:`,
`org_text_reads_back_as_written.rs` `:::`).

## Missing piece
An oracle for org syntax that is independent of Holon's own parser: the
keystone's reference model uses the same split, so no invariant could go red.

## Remedy
- Holon: `Tags::org_tag_group_start` (crates/holon-api/src/types.rs:1024) is
  org's rule; `split_org_headline` and `linkable_title` use it. The two tests
  now expect org's reading (`a_tag_group_that_names_no_tag_ends_the_title`).
- Fork: `lane-logs/r18-orgize.patch` (`headline_tags_node`, test
  `a_headline_title_and_tags_are_read_as_org_reads_them` against the fixture).
- FIXED: the fork fix is landed (orgize `holon` branch `ee3807a`, then
  `df58aef` with the r18c rules: only `[ \t]` may follow a tag group) and
  Holon's `Cargo.lock` pins `df58aef`. Fork logs
  `lane-logs/groupB-r18b-fork-red-headline.log` (red),
  `lane-logs/groupB-r18b-fork-green-all.log` and
  `lane-logs/groupB-r18c-fork-red-tagend.log` -> `groupB-r18c-fork-green-full.log`
  (green), `lane-logs/groupB-r18b-fork-teeth.log`,
  `lane-logs/groupB-r18c-fork-teeth.log` (teeth). Holon on the pinned tree:
  `lane-logs/groupB-r18d-orgformat.log`, `lane-logs/groupB-r18d-orgmode-di.log`,
  `lane-logs/groupB-r18d-app.log`.
