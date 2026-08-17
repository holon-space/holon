---
id: 2026-07-30-sidebar-disclosure-collapsed-parent-halo-perceptually
date: 2026-07-30
gap: PERCEPTION
secondary: null
status: FIXED
summary: >-
  Sidebar disclosure: the collapsed-parent halo is perceptually absent.
  Measured 1.05:1 luminance contrast against the sidebar background in BOTH
  shipped themes (holonLight: halo rgb(245,245,245) on bg rgb(241,240,235);
  holonDark: rgb(38,38,38) on rgb(41,41,38)) — the WCAG floor for a meaningful
  non-text state indicator is 3:1, and the halo is only findable at 3–5× zoom.
  Its stated purpose ("hidden content is scannable at a glance" down the whole
  sidebar) is not achieved. Compounding it, the affordance's visual weight is
  INVERTED: a leaf's `bullet_dot` (solid, `muted_foreground`) reads heavier
  than a parent's chevron, so leaves dominate the scan and parents recede —
  the opposite of "a parent must be identifiable at a glance". Found by the
  dogfood gate driving the real GPUI app over MCP against a 71-page scratch
  vault (feature rev f55d7172).
source_line: 1120
---

## Bug

Sidebar disclosure: the collapsed-parent halo is perceptually absent.
Measured 1.05:1 luminance contrast against the sidebar background in BOTH
shipped themes (holonLight: halo rgb(245,245,245) on bg rgb(241,240,235);
holonDark: rgb(38,38,38) on rgb(41,41,38)) — the WCAG floor for a meaningful
non-text state indicator is 3:1, and the halo is only findable at 3–5× zoom.
Its stated purpose ("hidden content is scannable at a glance" down the whole
sidebar) is not achieved. Compounding it, the affordance's visual weight is
INVERTED: a leaf's `bullet_dot` (solid, `muted_foreground`) reads heavier
than a parent's chevron, so leaves dominate the scan and parents recede —
the opposite of "a parent must be identifiable at a glance". Found by the
dogfood gate driving the real GPUI app over MCP against a 71-page scratch
vault (feature rev f55d7172).

## Root cause

the sidebar disclosure feature's "collapsed halo" is invisible in practice —
measured 1.05:1 luminance contrast against the sidebar background in BOTH
themes (holonLight: halo rgb(245,245,245) on rgb(241,240,235); holonDark:
rgb(38,38,38) on rgb(41,41,38)); the WCAG floor for a meaningful non-text
state indicator is 3:1. The halo's stated job — "hidden content is scannable
at a glance down the whole sidebar" — is therefore not done: it is only
findable at 3–5× zoom. Compounding it, the affordance's visual weight is
INVERTED: leaf rows draw `bullet_dot` (solid, `muted_foreground`) which
reads heavier than a parent's chevron, so leaves shout and parents whisper.
Found driving the real GPUI app over MCP on a 71-page scratch vault.
PERCEPTION and irreducibly so: the covering PBT asserts halo PRESENCE (a
registry entry), which cannot express "is it perceptible" — tint and size
are Martin's taste call, but presence-only is the wrong observable for the
design intent.)

## Missing piece

The covering PBT's halo observable is PRESENCE (a bounds-registry entry),
which cannot express perceptibility. A contrast-ratio or paint-colour
assertion against the row background would be formalizable and is the
missing piece; size and tint remain Martin's taste call.

## Remedy

FIXED-in-same-land 2026-07-30 — the halo's tint and weight are corrected in
the SAME change that introduced the affordance, so the feature never lands
with the imperceptible halo. Evidence: light-theme and dark-theme close-ups
plus the contrast probe in the session scratchpad.
