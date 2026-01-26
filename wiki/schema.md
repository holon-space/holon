---
title: Wiki Schema & Conventions
type: concept
tags: [meta, wiki]
created: 2026-04-13
updated: 2026-04-13
related_files: []
---

# Wiki Schema & Conventions

## Purpose

This wiki documents the `holon` codebase using the Karpathy-wiki pattern: LLM-authored, source-code-driven, interlinked pages. Source code is the source of truth — this wiki is a navigational layer, not a substitute.

## Frontmatter

Every page carries YAML frontmatter:

```yaml
---
title: Page Title
type: concept | entity | query | overview
tags: [tag1, tag2]
created: YYYY-MM-DD
updated: YYYY-MM-DD
related_files: [crates/foo/src/bar.rs]
---
```

## Cross-References

- Use `[[wikilinks]]` for linking to other wiki pages by their filename (no `.md` extension).
- Use `crates/foo/src/bar.rs:42` style for source file references.
- Use code fences with language tags for all code snippets.

## File Naming

| Directory | Contents |
|-----------|----------|
| `wiki/entities/` | One page per major crate or frontend module |
| `wiki/concepts/` | One page per architectural concept or design pattern |
| `wiki/index.md` | Catalog of all pages |
| `wiki/log.md` | Append-only operation log |
| `wiki/overview.md` | Architecture overview |
| `wiki/schema.md` | This file |

## Page Types

- **overview** — architecture, tech stack, directory layout
- **entity** — a crate, module, or runtime object (tracks `related_files`)
- **concept** — an architectural pattern, design decision, or cross-cutting concern
- **query** — a reference for querying data in the system (PRQL, SQL, GQL snippets)

## Maintenance

When source code changes significantly, update the relevant wiki page and append an entry to `[[log]]`. Add the date and a short description.

During every INGEST operation, also review `[[tech-debt]]`:
- Mark items as resolved if the underlying code changed
- Add new items if new structural problems are identified
- Update severity if circumstances changed
