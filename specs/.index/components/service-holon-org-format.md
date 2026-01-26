---
name: holon-org-format
description: Pure org-mode parsing, rendering, and diffing — no I/O, no dependencies on storage
type: reference
source_type: component
source_id: crates/holon-org-format/src/
category: service
fetch_timestamp: 2026-04-23
---

## holon-org-format (crates/holon-org-format)

**Purpose**: Pure library for org-mode text ↔ block data structure round-tripping. No disk I/O. Used by `holon-orgmode` and `holon-integration-tests`.

### Key Modules & Types

| Module | Key Types / Traits |
|--------|-------------------|
| `parser` | `parse(text) -> ParseResult` |
| `org_renderer` | `OrgRenderer` — serializes blocks to org text |
| `block_diff` | `BlockDiff` — diffs two block sets |
| `models` | `Block`, `OrgBlockExt`, `OrgDocumentExt`, `ToOrg` |
| `link_parser` | Org `[[link]]` and `[[link][description]]` parsing |

### Bare ID Convention

Org files store IDs WITHOUT scheme prefixes (no `block:`, no `doc:`):
- **Parser**: adds scheme via `EntityUri::block(id)` for source blocks, `EntityUri::from_raw(id)` for headings
- **Renderer**: strips scheme by writing `.id()` (path part only)
- Bug history: source block fallback IDs like `j-09-::src::0` were parsed as scheme `j-09-` (valid RFC 3986). Fixed by always using `EntityUri::block()` explicitly.

### Related

- **holon-orgmode**: wraps this for disk I/O
- **holon-api**: `EntityUri` defined there, used here at parse boundary
