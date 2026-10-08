# Architecture Decision Records

An ADR records a decision and the reasons for it.

The files in `docs/Architecture/` describe how things work today. ADRs say
why we chose it. Link between the two. Do not copy text from one to the other.

## Amend in place

When a decision changes, edit the ADR itself.

- Change the decision text. Never leave a dead rule with only a
  "superseded" flag.
- Decision IDs stay stable. A removed decision is marked removed. Its number
  is never used again.
- Write one or two present-tense sentences that say why the old rule no
  longer applies. Keep rules that still hold visible.
- Add a status line with the date and a pointer.
- Do not write a change log or tell the history in the text. Version control
  holds the past.

Example status line:

    Amended 2026-10-08 — guard 1; see ../Architecture/Vision-PetriNetCoordination.md
