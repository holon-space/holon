---
id: 2026-07-12-advice-feature-coupled-holon-frontend-holon
date: 2026-07-12
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  advice feature coupled holon-frontend→holon-advice→holon-turso/turso — page
  target invariant "no engine crates" broken, dioxus-web uncompilable since
  ~07-07
source_line: 982
---

## Bug

advice feature coupled holon-frontend→holon-advice→holon-turso/turso — page
target invariant "no engine crates" broken, dioxus-web uncompilable since
~07-07

## Missing piece

no CI builds wasm32-unknown-unknown for the page; invariant undeclared in
code

## Remedy

FIXED (holon-advice `engine` feature default-on; frontend takes
default-features=false; 872dbf4a)
