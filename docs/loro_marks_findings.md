# Loro marks spike findings (Phase 0.1)

**Loro version**: `1.11.1` (pinned exactly via `loro = "= 1.11.1"` in `Cargo.toml`).
**Spike source**: `crates/holon/examples/loro_marks_spike.rs`
**Reproduce**: `cargo run -p holon --example loro_marks_spike 2>&1 | tee /tmp/loro-marks-spike.log`

## Verdict

All Phase 0.1 hypotheses (H1, H2, H8, H12) are confirmed favorably for the
plan's design. H13 produced one important caveat (silent latch, see S3
below) — the plan already requires single-call init, so this is consistent.

## API surface (1.11.1)

```rust
// Range-based, Unicode-scalar offsets:
text.mark(start..end, "bold", true)         // value: impl Into<LoroValue>
text.unmark(start..end, "bold")
text.to_delta() -> Vec<TextDelta>           // [Insert { insert, attributes }]

// Doc-level config (call ONCE before any mark):
let mut cfg = StyleConfigMap::new();
cfg.insert("bold".into(), StyleConfig { expand: ExpandType::After });
doc.config_text_style(cfg);

// Stable cursors:
let cursor = text.get_cursor(pos, loro::cursor::Side::Middle).expect(..);
let pos    = doc.get_cursor_pos(&cursor).expect(..).current.pos;
```

## Findings

### S1 — Basic mark/unmark round-trip

`mark(0..5, "bold", true)` then `unmark(0..5, "bold")` returns a clean delta
with no residual attributes. Delta format:

```
[ Insert { insert: "Hello, ", attributes: None },
  Insert { insert: "world", attributes: Some({"italic": Bool(true)}) },
  Insert { insert: "!",     attributes: None } ]
```

This is exactly the Quill/Peritext shape we render in the GPUI builder.

### S2 (H2) — Late-created LoroText sees the configured expand ✅

A `LoroText` obtained via `doc.get_text("name")` AFTER
`config_text_style` honors the configured `ExpandType`. Specifically:
inserting at the end of an `ExpandType::None` link does NOT extend the link.
Confirms the plan's "configure once at LoroDoc creation" approach works for
all per-block LoroTexts that are accessed lazily via `get_text`.

### S3 (H13) — Reconfigure with a conflicting expand: silent latch ⚠️

Calling `config_text_style` twice with a conflicting `ExpandType` for the
same key does NOT panic, but the new config has **no effect** — the first
config wins. Test: configure `"bold"` as `After`, then re-call with
`None`; subsequent insert at end of bold range is still bold.

**Implication**: There is no runtime "fix" for a wrong initial config. The
plan's directive to call `config_text_style` exactly once at LoroDoc
creation, with the full mark vocabulary, must be enforced.

### S4 (H8) — Concurrent bold + insert: B's text inherits Bold ✅

Peer A bolds `[5..10]`; peer B (offline) inserts `"INSERT"` at position 7.
After both peers exchange updates, both converge to the same delta where
`"INSERT"` is bold (it lands inside A's bold range). Confirms Peritext's
intended UX semantics for `ExpandType::After`.

### S5 — Concurrent unmark inside bold splits the mark ✅

Peer A bolds `[5..10]`; peer B unmarks `[7..8]`. Merged result splits the
range: `[5..7]` bold, `[7..8]` plain, `[8..10]` bold. Standard Peritext
behavior.

### S6 — Concurrent link conflict: LWW, both peers converge ✅

Peer A links `[5..10]` to `URL_X`; peer B links the same range to `URL_Y`.
Both peers converge to the **same** value (`URL_Y` won, deterministic by
operation ID). No divergence — safe for production.

### S7 — `LoroValue::Map` as link value ✅

```rust
let mut map: HashMap<String, LoroValue> = HashMap::new();
map.insert("kind".into(),  "internal".into());
map.insert("id".into(),    "uuid-123".into());
map.insert("label".into(), "here".into());
text.mark(6..10, "link", LoroValue::from(map))?;
```

The structured map survives `to_delta` round-trip with all keys intact.
Confirms our `EntityRef::Internal { id, label }` shape is viable.

**Note**: `LoroValue::Map` wraps a `LoroMapValue` (not `Arc<HashMap>` as in
older versions). Use `HashMap::into()` rather than constructing the variant
directly — the From impl handles the wrapping.

### S8 (H12a) — Cursor stable across remote insert ✅

Cursor anchored at position 5; remote peer inserts 3 chars at position 0;
cursor resolves to position 8. As expected.

### S9 (H12b) — Cursor stable across mark-only change ✅

Cursor at position 5; a mark is applied (no text change); cursor still
resolves to position 5. Marks don't move characters.

### S10 — Indexing decision: use `mark` (Unicode scalar) ✅

For `"héllo"`: `len_unicode=5`, `len_utf8=6`, `len_utf16=5`.

`mark(1..2, "bold", true)` on this text correctly bolds the single `é`
character. **Decision**: use `mark` (Unicode scalar offsets) throughout.
Orgize emits byte ranges from rowan; we convert byte→scalar at the parser
boundary (`crates/holon-orgmode/src/parser.rs`).

### S11 — Cross-peer unmark works ✅

Peer A applies a mark; peer B unmarks the same range; after merge, the
mark is gone on A. No "unmergeable annotations" error encountered. The
caveat in Loro docs about unmark applies to mark types configured as
"unmergeable comments"; we don't use that pattern.

## Decisions locked in

| Decision | Choice | Rationale |
|---|---|---|
| Loro version | `= 1.11.1` (exact pin) | Latest stable; spike clean against this |
| Indexing | Unicode scalar (`mark`/`unmark`) | Matches orgize → scalar conversion at boundary |
| Config lifecycle | Once at `LoroDoc::new` | S3 shows reconfigure is a silent no-op |
| Link value shape | `LoroValue::Map { kind, id, label }` | S7 confirms round-trip stability |
| Cursor model | `loro::cursor::Side::Middle` | Stable across both text edits and mark-only changes |

## Follow-ups (deferred to Phase 1)

- **`apply_delta` symmetry**: not exercised end-to-end in this spike; spot-check during Phase 1 unit tests when wiring the Org parser → mark emission path.
- **Performance**: 100+ marks on a paragraph not measured here. Belongs in Phase 2 perf smoke (`<16ms` target).
- **CDC behavior on mark-only changes (H22)**: this spike doesn't cover the broader Holon event bus — that's a Phase 1 reconcile test.
