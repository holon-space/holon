# Parse, Don't Validate — Codebase Audit

Audit of stringly-typed data flowing through Holon, where types could encode invariants
instead of re-validating at every call site.

Last updated: 2026-02-26

## Priority 1: `parent_id` + `id` — Stringly-Typed Sum Type — DONE

**Implemented.** `EntityUri` newtype around `fluent_uri::Uri<String>` in `holon-api/src/entity_uri.rs`.

Both `Block.id` and `Block.parent_id` are `EntityUri`. `Document.id` and `Document.parent_id` are `EntityUri`.
The old `ParentRef` enum is deleted. Legacy URI formats (`holon-doc://`, `__no_parent__`) are eliminated.

URI schemes: `doc:path`, `block:uuid`, `sentinel:no_parent`.
API: `.is_doc()`, `.is_block()`, `.is_sentinel()`, `.is_no_parent()`, `.as_str()`, `.id()` (path component).
No `AsRef<str>`, no `PartialEq<str>`, no `Borrow<str>` — prevents silent string comparisons that bypass the type.

All Rust-level `starts_with("doc:")` / `starts_with("holon-doc://")` checks replaced with `EntityUri` methods.
Only remaining string-level check is the PRQL stdlib SQL filter: `filter (parent_id | text.starts_with "doc:")` (unavoidable — SQL operates on stored text).

Remaining (separate domain):
- `crates/holon-todoist/src/models.rs:20` — `TodoistTask.parent_id: Option<String>` (Todoist API model, different domain)

## Priority 2: `content_type` — Always "text" or "source" — DONE

**Implemented.** `ContentType` enum in `holon-api/src/types.rs` with `Display`, `FromStr`, `Ord`, `Value` conversion.
`Block.content_type: ContentType` (not `String`). Dead constants `CONTENT_TYPE_TEXT`/`CONTENT_TYPE_SOURCE` removed.
All bypasses eliminated:
- `loro_backend.rs` — parses CRDT string to `ContentType` at read boundary
- `pbt_infrastructure.rs` — `ComparableBlock.content_type: ContentType`
- `round_trip_pbt.rs` — `NormalizedBlock.content_type: ContentType`, enum comparisons
- `test_environment.rs` — parses SQL row to `ContentType`, uses `ContentType::Text.into()` / `ContentType::Source.into()`
- `general_e2e_pbt.rs` — uses `ContentType.into()` for Value creation
- `cucumber.rs` — parses SQL row to `ContentType`
- `loro_block_operations.rs` — parses fields map to `ContentType` at boundary
- `org_sync_controller.rs`, `org_renderer.rs` — already used enum (were fixed before this pass)

## Priority 3: `task_state` — 4 Different Done-Checks (LATENT BUG)

Done-ness is checked in 4 places with **inconsistent keyword lists**:
1. `crates/holon-orgmode/src/models.rs:132-134` — `["DONE", "CANCELLED", "CLOSED"]`
2. `crates/holon-orgmode/src/models.rs:390-393` — per-document TODO config
3. `crates/holon/src/petri.rs:622-626` — `["DONE", "CANCELLED", "CLOSED"]`
4. `crates/holon-core/src/traits.rs:1011-1012` — **only checks `"DONE"`** (BUG: misses CANCELLED, CLOSED)

**Status: IMPLEMENTED.** `TaskState` struct with `keyword: String` + `category: StateCategory` in `holon-api/src/types.rs`.
- `TaskState::from_keyword()` uses default done-keyword list (DONE/CANCELLED/CLOSED)
- `TaskState::from_keyword_with_done_list()` accepts per-document `#+TODO:` config
- `TaskState::active()` / `TaskState::done()` for explicit construction at provider boundaries

Migrated (clean):
- `OrgBlockExt::task_state()` → `Option<TaskState>` (parsed at boundary)
- `OrgBlockExt::is_completed()` → delegates to `TaskState::is_done()`
- `petri.rs::TaskInfo.task_state: Option<TaskState>` — inline `done_keywords` array removed, uses `TaskState::is_done()`
- `holon-core/traits.rs::TaskEntity::completed()` — **BUG FIXED**: was reading wrong property key `"TODO"` instead of `"task_state"`, now uses `TaskState::from_keyword().is_done()`
- `parser.rs` — constructs `TaskState` with document-level done keywords at parse boundary
- `org_renderer.rs` — constructs `TaskState::from_keyword()` when transferring legacy `TODO` property
- Duplicate `DEFAULT_DONE_KEYWORDS` / `DEFAULT_ACTIVE_KEYWORDS` / `is_done_keyword()` removed from `models.rs` — single source of truth in `TaskState`
- All integration tests + PBT updated

Still legacy (trait boundary — kept for macro/API compat):
- `holon-core/traits.rs:929` — `set_state(task_state: String)` (macro-generated dispatch extracts String from params)
- `holon-core/traits.rs:25-26` — `CompletionStateInfo.state: String` (serialized to frontend JSON)
- `holon-todoist/todoist_datasource.rs:197` — compares against `"completed"` (Todoist-specific vocabulary at trait boundary)

## Priority 4: `source_language` / Query Language (8+ files) — DONE

**Implemented.** `compile_to_sql()` now takes `QueryLanguage` (enum) instead of `&str`.
All internal function signatures (`E2ETestContext::query`, `TestEnvironment::query`, `setup_watch`, etc.) use `QueryLanguage`.
`TestEnvironment::create_source_block` takes `SourceLanguage`.

Boundaries parse at entry:
- MCP `tools.rs` — `params.language.parse::<QueryLanguage>()` with `invalid_params` error on bad input
- FFI `ffi_bridge.rs` — `language.parse::<QueryLanguage>()` at boundary
- Blinc `live_query.rs` — dispatches `QueryLanguage::Gql/Sql/Prql` from DSL args
- `render_block` in `backend_engine.rs` — parses `query_language` from block info at boundary
- `test_environment.rs` — parses `source_language` from SQL rows via `SourceLanguage::as_query()`
- `cucumber.rs` — parses step string to `SourceLanguage` at BDD boundary
- `general_e2e_pbt.rs` — removed duplicate local `QueryLanguage` enum, uses `holon_api::QueryLanguage`

`SourceLanguage` enum: `Query(QueryLanguage)`, `Render`, `Other(String)`.
`SourceLanguage::as_query()` projects to `Option<QueryLanguage>`.
String matching (`"prql" | "gql" | "sql"`) replaced with `sl.as_query().is_some()` throughout.

## Priority 5: Petri Net Prototype Properties — DONE

**Implemented.** `PrototypeValue` enum with `Literal(f64)` and `Computed(String)` variants.
`BTreeMap<String, PrototypeValue>` replaces `BTreeMap<String, String>` throughout:
- `DEFAULT_TASK_PROTOTYPE` is now `&[(&str, PrototypeValue)]` (literals) + `default_computed_props()` (expressions)
- `block_to_prototype_props()` parses at the boundary, panics on invalid values
- `resolve_prototype()` uses typed match instead of string parsing
- `PrototypeValue::parse()` for string→typed conversion at boundaries
- Silent drops of invalid values eliminated

## Priority 6: `deadline` / `scheduled` — Raw Strings — DONE

**Implemented.** `Timestamp` struct in `holon-api/src/types.rs` with `raw: String` + `date: NaiveDate`.
`Timestamp::parse()` handles org-mode format (`<2026-02-21 Fri>`, `<2026-02-21 Fri 10:00>`) and plain ISO dates (`2026-02-21`).

All `.ok()` silent drops eliminated:
- `OrgBlockExt::scheduled()` / `deadline()` — `.expect()` panics on invalid stored data (bug, not user error)
- `petri.rs::TaskInfo::from_block()` — `.expect()` on stored deadline property
- `holon-core/traits.rs::due_date()` — `.expect()` on stored DEADLINE property
- `parser.rs` — `tracing::warn!()` on unparseable org timestamps (user-authored content, crash too aggressive)
- `org_renderer.rs` — `tracing::warn!()` on unparseable property transfer

Parse boundaries (parser, renderer) log warnings for invalid timestamps from org files.
Internal reads (getters, petri) panic — stored data must be valid.

Still legacy (separate domain):
- `holon-todoist/models.rs:27-35` — `due_date`, `created_at`, `updated_at`, `completed_at` as `Option<String>` (Todoist API model)

## Priority 7: `tags` — Comma-Separated String — DONE

**Implemented.** `Tags` newtype wrapping `Vec<String>` in `holon-api/src/types.rs`.
`OrgBlockExt::tags()` returns `Tags` (not `Option` — empty is the zero value).
`OrgBlockExt::set_tags(Tags)` removes the property key when empty.

API: `Tags::from_csv()`, `Tags::from_iter()`, `Tags::to_csv()`, `Tags::to_org()`, `Tags::to_set()`, `Tags::as_slice()`, `Tags::is_empty()`.
`Display` → csv, `FromStr` → csv, `From<Vec<String>>`, `From<Tags> for Value`.
No `Option` wrapper — eliminates `None` vs `Some(empty)` ambiguity.

All split/join call sites eliminated:
- `models.rs` — `format_tags()` delegates to `Tags::to_org()`; `get_tags()` method deleted (redundant)
- `parser.rs` — constructs `Tags::from_iter()` at parse boundary
- `org_renderer.rs` — constructs `Tags::from_csv()` at legacy property transfer
- `org_sync_controller.rs` — uses `Tags::to_csv()` for params; `blocks_differ()` compares `Tags` directly
- `org_utils.rs` — uses `Tags::to_org()` for org headline rendering
- `round_trip_pbt.rs` — uses `Tags::to_set()` for normalization
- `general_e2e_pbt.rs` — uses `Tags::from_csv()` at SQL row boundary

## Priority 8: `event_type: String` — Stringly-Typed Event Dispatch — DONE

**Implemented.** `EventKind` enum (`Created`, `Updated`, `Deleted`, `FieldsChanged`) and `AggregateType` enum (`Block`, `Task`, `Project`, `Directory`, `File`, `Custom(String)`) in `holon/src/sync/event_bus.rs`.

`Event.event_type: String` + `Event.aggregate_type: String` replaced with `Event.event_kind: EventKind` + `Event.aggregate_type: AggregateType`.

Migrated (clean):
- `Event::new()` takes typed `EventKind` + `AggregateType` instead of `impl Into<String>`
- `CacheEventSubscriber` — exhaustive `match event.event_kind` instead of `match event.event_type.as_str()`
- `CacheEventSubscriber::subscribe_entity()` takes `AggregateType` instead of `&str`
- `EventFilter.aggregate_types: Vec<AggregateType>` (not `Vec<String>`)
- `LoroEventAdapter` — constructs `EventKind::Created/Updated/Deleted/FieldsChanged` + `AggregateType::Block`
- `OrgModeEventAdapter` — generic `change_to_event<T>()` helper with `AggregateType::Directory`/`File`
- `TodoistEventAdapter` — generic `publish_change<T>()` helper with `AggregateType::Task`/`Project`
- `SqlOperationProvider` — `publish_event()` takes `EventKind`, derives `AggregateType` from entity_short_name at boundary
- `extract_doc_ids_from_event()` in `di.rs` — exhaustive `match event.event_kind`

SQL boundary (TursoEventBus):
- `event_to_params()` serializes via `event.event_type_string()` → `"block.created"` for SQL `event_type TEXT` column
- `parse_row_change_to_event()` parses via `Event::parse_event_type_string()` at the read boundary
- `EventKind` stays closed — new kinds require an explicit code change
- `AggregateType::Custom(String)` handles third-party integrations

## Priority 9: `string_properties: Option<String>` — JSON-in-String with Silent Error Drop — DONE

**Implemented.** `StringProperties` newtype wrapping `HashMap<String, String>` in `holon-api/src/types.rs`.
`OrgBlockExt.string_properties()` returns `StringProperties` (not `Option` — empty is the zero value, like `Tags`).
`OrgBlockExt.set_string_properties(StringProperties)` removes the property key when empty.

API: `StringProperties::from_json()` (panics on stored data), `StringProperties::from_json_lenient()` (logs warning at parse boundaries), `StringProperties::from_iter()`, `StringProperties::to_json()`, `get()`, `insert()`, `remove()`, `iter()`, `into_inner()`, `is_empty()`.

All JSON parsing `.ok()` / `Err(_) => return` eliminated:
- `models.rs` — `format_properties_drawer()` takes `&StringProperties`, no JSON parsing
- `models.rs` — `get_block_id()` uses `string_properties().get("ID")` directly
- `parser.rs` — `extract_properties()` returns `StringProperties` (parsed from orgize drawer, never JSON)
- `org_sync_controller.rs` — `build_update_params()` iterates typed map
- `org_renderer.rs` — builds `StringProperties` directly, no JSON serialization fallback
- `block_diff.rs` — `parse_block_properties()` delegates to `into_inner()`
- `round_trip_pbt.rs` — builds `StringProperties` directly

## Priority 10: Priority A/B/C ↔ Integer — Three Independent Converters with Silent Fallbacks — DONE

**Implemented.** `Priority` enum (`High`, `Medium`, `Low`) in `holon-api/src/types.rs`.
Decoupled from org's A/B/C convention — org letters are a serialization format.

API: `from_letter(&str) -> Result` (A→High, B→Medium, C→Low), `from_int(i32) -> Result` (3→High, 2→Medium, 1→Low), `to_letter()`, `to_int()`.
Both `from_letter` and `from_int` fail loudly on unknown values instead of silently mapping to a default.
`From<Priority> for Value` serializes as integer; `TryFrom<Value>` accepts both integer and string.

**Latent bug fixed:** Priority "D" in org file now panics at parse boundary instead of silently mapping to 0 → "A".

Migrated (clean):
- `parser.rs` — `priority_str_to_int()` deleted; uses `Priority::from_letter()` at parse boundary (panics on invalid)
- `models.rs` — `priority_int_to_str()` deleted; `OrgBlockExt::priority() -> Option<Priority>`, `set_priority(Option<Priority>)`; headline rendering uses `priority.to_letter()`
- `org_renderer.rs` — `Priority::from_letter()` / `Priority::from_int()` at PRIORITY property transfer (`.ok()` — lenient for legacy properties)
- `org_sync_controller.rs` — `priority.to_int()` for params
- `holon-core/traits.rs` — `Priority::from_letter()` with panic on invalid stored data (replaces silent `_ => {}` drop)
- `petri.rs` — `TaskInfo.priority: Option<Priority>`; `Priority::from_int()` at read boundary; `priority.to_int()` for Rhai context
- `org_utils.rs` — `priority.to_letter()` (replaces inline match with silent empty-string fallback)
- `round_trip_pbt.rs`, `general_e2e_pbt.rs`, `petri_e2e_pbt.rs` — all updated to use `Priority` enum

SQL storage unchanged: priority stored as INTEGER (1/2/3) in blocks table.

## Priority 11: `widget_type: String` — Fixed 3-Value Set — DONE

**Implemented.** `WidgetType` enum (`Checkbox`, `Text`, `Button`) in `holon-api/src/render_types.rs`.
`OperationWiring.widget_type: WidgetType` (not `String`).
`Display`, `FromStr` with loud failure on unknown values.
Serde `rename_all = "lowercase"` for JSON compatibility.
`Custom(String)` variant omitted — no current need; can be added when a fourth widget type is required.
Dart FRB-generated `WidgetType` enum in `render_types.dart`.

## Priority 12: `region: String` — Fixed 3-Value Navigation Region — DONE

**Implemented.** `Region` enum (`Main`, `LeftSidebar`, `RightSidebar`) in `holon-api/src/types.rs`.
`Display`, `FromStr` with loud failure on unknown values. `From<Region> for Value`, `TryFrom<Value> for Region`.
`Region::ALL` constant for iteration.

Migrated (clean):
- `navigation/provider.rs` — `focus()`, `go_back()`, `go_forward()`, `go_home()` take `Region` instead of `&str`
- `navigation/provider.rs` — `execute_operation()` parses `Region` from `Value` at the boundary
- `navigation/provider.rs` — `region_param()` builds `TypeHint::OneOf` from `Region::ALL`
- `schema_modules.rs` — `initialize_data()` iterates `Region::ALL` instead of hardcoded string array
- `test_environment.rs` — `navigate_focus/back/forward/home()` take `Region`
- `general_e2e_pbt.rs` — `E2ETransition::NavigateFocus/Back/Forward/Home` use `Region` (not `String`)
- `general_e2e_pbt.rs` — `ReferenceState.navigation_history: HashMap<Region, NavigationHistory>`
- `general_e2e_pbt.rs` — `can_go_back()`, `can_go_forward()`, `current_focus()` take `Region`

SQL boundary (`region_data`, `region_streams` in TestEnvironment) remains `HashMap<String, ...>` — CDC streams use stored text.

## Priority 13: `command_type: String` (Todoist) — Fixed 4-Value Set (NEW)

`SyncCommand.command_type: String` in `holon-todoist/models.rs:258`. Only four values ever assigned:
- `holon-todoist/client.rs:606,650,701,776` — `"item_add"`, `"item_update"`, `"item_close"`, `"item_delete"`

**Proposed type:** `enum SyncCommandType { ItemAdd, ItemUpdate, ItemClose, ItemDelete }`

Keep separate from `EventKind`: Todoist verbs (`item_close` = complete a task) are domain-specific, not generic CRUD.
`EventKind` is for the internal event bus (generic CRUD). Forcing them together would leak Todoist semantics into the event system.

## Priority 14: `depends_on: Option<String>` — Comma-Separated ID List — DONE

**Implemented.** `DependsOn` newtype wrapping `Vec<String>` in `holon-api/src/types.rs`.
No `Option` wrapper — empty is the zero value (like `Tags`).

API: `DependsOn::from_csv()`, `to_csv()`, `contains()`, `push()`, `iter()`, `as_slice()`, `is_empty()`.
`Display` → csv, `FromStr` → csv, `From<Vec<String>>`, `From<DependsOn> for Value`.

Migrated (clean):
- `OrgBlockExt::depends_on()` → `DependsOn` (not `Option<String>`)
- `OrgBlockExt::set_depends_on(DependsOn)` — removes property key when empty
- `OrgBlockExt::depends_on_ids()` — **deleted** (redundant; `DependsOn` IS the parsed list)
- `petri.rs::TaskInfo.depends_on: DependsOn` — replaces inline `split(',')` parsing at boundary
- `petri.rs::resolve_sequential_deps()` — uses `DependsOn::contains()` / `push()`
- `petri_e2e_pbt.rs::RefBlock.depends_on: DependsOn` — **latent bug fixed**: was `Option<String>` checking only a single dep ID; now iterates all deps in invariant checks

## Priority 15: `labels: Option<String>` (Todoist) — Comma-Joined List (NEW)

API returns `Vec<String>` but immediately flattened: `holon-todoist/models.rs:29,295`.

**Proposed type:** `Vec<String>`, joined only for SQL storage.

Todoist labels are global (shared across projects, colored). Org tags are per-heading and inherited.
Semantically different but could share a `Tag` newtype, with a `TagSource` enum (`Org`/`Todoist`/`Manual`) if provenance matters.

---

## Summary: Latent Bugs from Stringly-Typed Code

1. ~~**Priority roundtrip corruption**: Unknown priority "D" → `0` → "A". Silent data loss.~~ **FIXED** — `Priority` enum in `holon-api/src/types.rs`; `from_letter()` panics on invalid letters at parse boundary.
2. **Todoist vs Org state vocabulary**: `set_state("completed")` and `set_state("DONE")` are the same type. Consumer checking `== "DONE"` misses Todoist completions.
3. ~~**string_properties `.ok()` drop**: Malformed JSON silently becomes "no properties".~~ **FIXED** — `StringProperties` type in `holon-api/src/types.rs`; `from_json()` panics on stored data, `from_json_lenient()` logs at parse boundaries.
4. ~~**holon-core done-check**: `traits.rs:1011-1012` only checks `"DONE"`, misses CANCELLED/CLOSED.~~ **FIXED** — reads correct property key + uses `TaskState::is_done()`.
5. ~~**deadline format mismatch**: `petri.rs` parses `%Y-%m-%d` but org timestamps are `<2026-02-21 Fri>` — `.ok()` silently defaults to `f64::MAX`.~~ **FIXED** — `Timestamp::parse()` handles org format; `.ok()` replaced with `.expect()` on stored data + `tracing::warn!()` at parse boundaries.
