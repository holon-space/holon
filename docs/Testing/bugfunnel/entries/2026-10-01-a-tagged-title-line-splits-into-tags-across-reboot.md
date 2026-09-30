---
id: 2026-10-01-a-tagged-title-line-splits-into-tags-across-reboot
date: 2026-10-01
gap: ORACLE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  A block whose stored title line ends in a tag group shows the title without
  the tags, and holds the tags as tags, after a Reboot; the reference keeps
  the raw text.
---

## Bug
Found while fixing the BlockToPage tag group (org-faithful group B r17) with
hand-authored probes: create `"Tagged title :work:urgent:\nbody line"`, then
Reboot, with no BlockToPage. Red on loro and sqlonly:
`inv-displayed-text/viewmodel` shows `"Tagged title\nbody line"`, expected
`"Tagged title :work:urgent:\nbody line"`, and `inv-loro-ui-rows-match-ref`
`tags {urgent, work} != ref {}`. Not A/B-run on main.

## Root cause
Not found. The second boot reads the org file, where the tag group is tag
syntax, while the reference model keeps the stored blocks across a Reboot
(`ref_caps/boot.rs`, `reboot_drops_in_memory_state`).

## Missing piece
The keystone draws no tagged title line before a Reboot.

## Remedy
Open: decide whether the second boot may re-read a stored title into tags
(then the reference applies `apply_org_headline_tag_split` on Reboot) or must
keep the stored text.
