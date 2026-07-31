# Org File ID Conventions

## Bare IDs in Org Files

Org files store IDs **without** scheme prefixes (`block:`, `sentinel:`).
The parser adds the correct `EntityUri` scheme when reading; the renderer strips it when writing.

### Link targets are the documented exception: they DO carry schemes

Link targets (`[[target]]` / `[[target][text]]`) are the one place the bare-ID
rule does not apply — a target is written and read with its full scheme, and the
renderer emits it verbatim. Targets are classified in three states:

| Target | Classified as | Example |
|---|---|---|
| Web/mail URL | external link, unchanged | `[[https://example.com][site]]` |
| Scheme-shaped, scheme **registered** as an entity | resolved entity URI | `[[block:abc]]`, `[[tag:rust]]`, `[[person:alice]]`, `[[cc-session:0f3a][the refactor]]` |
| Scheme-shaped, scheme **not registered** | unknown-scheme link — disclosed, bytes preserved, never a page | `[[Areas:Work]]`, `[[doc:x]]` (retired H7, 2026-07-02) |
| Not scheme-shaped | page-creation intent, hashed to a deterministic `block:` UUID | `[[Projects/New thing]]`, `[[Ketosis: How to lose weight]]` |

"Scheme-shaped" is the RFC 3986 shape — `letter (letter|digit|+|-|.)* ':'` with
**no space after the colon**. The no-space rule is what keeps ordinary titles
(`Ketosis: How to lose weight`) on the page side without capitalization
heuristics, and Windows forbids `:` in filenames anyway, so a scheme-shaped page
file was never portable.

The shape is **reserved**: page creation rejects a scheme-shaped page name
(use `/` for hierarchy). That reservation is what makes installing or removing an
integration safe — its links move between resolved and unknown-scheme, never
across the page/entity boundary, so no page can have been silently minted under a
scheme that later becomes real.

The registered set is the entity registry (`TypeRegistry`): built-ins plus every
entity a YAML sidecar declares. Pages are ordinary blocks tagged `Page`.

### Heading blocks

```org
* My Heading
:PROPERTIES:
:ID: abc-123
:END:
```

- `:ID: abc-123` — bare string, no `block:` prefix
- Parser wraps with `EntityUri::from_raw("abc-123")` → `block:abc-123`
- Renderer writes `block.id.id()` (path part only) or `block.get_block_id()` (the stored "ID" property)

### Source blocks

```org
#+BEGIN_SRC holon_sql :id abc-123::src::0
SELECT * FROM blocks
#+END_SRC
```

- `:id abc-123::src::0` — bare string in header args
- Parser wraps with `EntityUri::block(src_id)` → `block:abc-123::src::0`
- Renderer writes `block.id.id()` (path part only)
- Fallback ID (when no `:id` header arg): `{parent_id}::src::{index}` (e.g., `abc-123::src::0`)

### Rule blocks (`holon_rule`)

A `holon_rule` source block is a self-contained reactive rule (ADR 0024 §7.2):
its **body is YAML** carrying both the guard and the effect — no separate trigger
block. This supersedes the legacy query+action *pair* (a `holon_sql` trigger next
to a Rhai `block.create(...)` action). The default journal-auto-create rule:

```org
#+BEGIN_SRC holon_rule :id journals::action::0
name: daily_journal
when: 'not block_exists("Journals/{today}")'
emit:
  place: page(journals)
  name: "{today}"
#+END_SRC
```

- `when:` — a guard string parsed by the dual-evaluated `Pattern` AST
  (`block_exists`, `has_tag`, `and`/`or`/`not`; `{today}` interpolates the clock).
- `emit:` — a ratcheted create: `place` (the placement kind) + `name`
  (`{today}`-interpolated leaf content). `place: <root>` places an **inline child**
  of `block:<root>` (`journals` → `block:journals`); `place: page(<root>)` places a
  `Page`-tagged child that materializes into its own `<name-chain>.org` file. The
  journal rule ships `place: page(journals)` (LogSeq-parity daily-note ruling
  2026-07-19): each day is minted as a `Page`-tagged child of the journals shell
  that owns its own `Journals/{today}.org` file, so the day's bullets nest UNDER
  the date page (not as flat siblings of the shell) and the date is a first-class
  `[[{today}]]` link target. Companion de-inline (a rule-created child page would
  otherwise stay inlined in the `Journals.org` companion) is handled by the Fork B
  B1 writeback sweep.
- The block is **program-marked** (`is_program`) so it renders as a rule card, not
  as query content. A malformed body surfaces a loud `RuleStatus::ParseError` on
  the card. The parser is `holon_advice::holon_rule::parse_holon_rule`.

### Image blocks

An image child block renders as a single `[[file:…]]` link line inside its
parent heading's section:

```org
* Heading
:PROPERTIES:
:ID: abc-123
:END:
[[file:attachments/photo.png]]
```

- Canonical form: `[[file:<relative-path>]]` on its own line, where the path
  ends in a known image extension (`png`, `jpg`, `jpeg`, `gif`, `webp`, `svg`,
  `bmp`, `ico`, `tiff`, `tif`). The extension is what classifies the block as
  `ContentType::Image` at the **parse boundary** (`is_image_path`) — a
  `[[file:…]]` link to a non-image target (e.g. `.pdf`) stays inline text with a
  `Link` mark, it does NOT become an image block.
- `block.content` stores the bare path (no `file:` scheme, no brackets); the
  renderer re-adds `[[file:…]]` on write (`Block::to_org`).
- Fallback ID (images carry no `:id`): `{parent_id}::img::{index}` (e.g.
  `abc-123::img::0`), assigned by the parser in document order.
- Image blocks carry `marks = None` and `source_language = None`; the
  image-ness lives solely in `content_type = Image`, which every storage layer
  (org ⇄ SQL ⇄ Loro) must preserve. In Loro this is a first-class
  `BlockContent::Image { path }` variant so the create/read round-trip cannot
  silently collapse it to `Text`.

### Why bare IDs?

1. **Human readability** — org files are edited in Emacs/vim, scheme prefixes are noise
2. **URI parsing ambiguity** — bare IDs like `j-09-::src::0` can be mis-parsed as scheme `j-09-` with path `::src::0` by RFC 3986 parsers. By convention, org files always store bare IDs and the parser always wraps them.
3. **Single source of truth** — the `EntityUri` type enforces the scheme internally; the org file just stores the identity

## Edge-field drawers: dependency edge (`:REQUIRES:` / `:BLOCKED-BY:`)

A block's dependency edge — "this block is blocked by / depends on these
blocks" — is a single edge field (`Block.requires`, projected to the
`block_requires` junction; see `crates/holon-turso/sql/schema/block_requires.sql`,
whose own comment names both spellings for this one edge). It has **two accepted
org-drawer spellings on read** and **one canonical spelling on write**:

| Spelling | On read (parse) | On write (render) |
|----------|-----------------|-------------------|
| `:REQUIRES: a b`   | lifted into `Block.requires` | **canonical** — always emitted |
| `:BLOCKED-BY: a b` | lifted into `Block.requires` (accepted alias) | never emitted (converges to `:REQUIRES:`) |

- Values are bare IDs (whitespace- or comma-separated), promoted to `block:`
  URIs at the parse boundary and stripped back to bare on render, exactly like
  IDs above.
- **Canonical form is `:REQUIRES:`** (owner ruling 2026-07-16). A `:BLOCKED-BY:`
  drawer therefore converges to `:REQUIRES:` on the first write-back — a
  *convergent canonical form* (the org analogue of the foreign-vault O4 ruling,
  `docs/Proposals/ForeignVaultCompat-2026-07-12.md` §6a): the edge is preserved
  losslessly; only the interchangeable keyword normalizes, and it reaches a
  fixed point in one pass.
- Rendered targets are **sorted** (the edge is a set of blockers; order is not
  semantic), which also makes the round-trip deterministic through the
  `json_group_array` junction hydration (no `ORDER BY`).
- There is **no distinct `BlockedBy` edge field**: `EdgeField` enumerates only
  `Tags`, `Requires`, `AdviceSuppressed` (`crates/holon-api/src/edge_field.rs`).
  `:BLOCKED-BY:` is an accepted input surface spelling of the `Requires` edge,
  not a second junction.

| Role | File | Key function/line |
|------|------|-------------------|
| Parse `:REQUIRES:`/`:BLOCKED-BY:` drawer | `parser.rs` | headline loop unions both keys into `block.requires` |
| Parse `:REQUIRES`/`:BLOCKED-BY` src header-arg | `parser.rs` | source-block header-arg loop unions both keys |
| Render canonical `:REQUIRES:` | `models.rs` | `drawer_properties()` inserts sorted `REQUIRES` |

### Code locations

| Role | File | Key function/line |
|------|------|-------------------|
| Parse heading ID | `parser.rs` | `EntityUri::from_raw(&id)` at block creation |
| Parse source ID | `parser.rs` | `EntityUri::block(&src_id)` at source block creation |
| Render heading ID | `models.rs` | `format_properties_drawer()` writes `ID` property (already bare) |
| Render source ID | `models.rs` | `source_block_to_org()` writes `block.id.id()` |
| Sync controller | `file_sync_controller.rs` | `build_block_params()` uses `get_block_id()` with `block.id.id()` fallback |
| Test serializer | `org_utils.rs` | `serialize_block_recursive()` writes `block.id.id()` |
