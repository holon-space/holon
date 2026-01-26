# Org File ID Conventions

## Bare IDs in Org Files

Org files store IDs **without** scheme prefixes (`block:`, `doc:`, `sentinel:`).
The parser adds the correct `EntityUri` scheme when reading; the renderer strips it when writing.

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

### Why bare IDs?

1. **Human readability** — org files are edited in Emacs/vim, scheme prefixes are noise
2. **URI parsing ambiguity** — bare IDs like `j-09-::src::0` can be mis-parsed as scheme `j-09-` with path `::src::0` by RFC 3986 parsers. By convention, org files always store bare IDs and the parser always wraps them.
3. **Single source of truth** — the `EntityUri` type enforces the scheme internally; the org file just stores the identity

### Code locations

| Role | File | Key function/line |
|------|------|-------------------|
| Parse heading ID | `parser.rs` | `EntityUri::from_raw(&id)` at block creation |
| Parse source ID | `parser.rs` | `EntityUri::block(&src_id)` at source block creation |
| Render heading ID | `models.rs` | `format_properties_drawer()` writes `ID` property (already bare) |
| Render source ID | `models.rs` | `source_block_to_org()` writes `block.id.id()` |
| Sync controller | `org_sync_controller.rs` | `build_block_params()` uses `get_block_id()` with `block.id.id()` fallback |
| Test serializer | `org_utils.rs` | `serialize_block_recursive()` writes `block.id.id()` |
