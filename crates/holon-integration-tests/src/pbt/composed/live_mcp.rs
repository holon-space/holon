//! **The out-of-process LIVE-MCP rung of the composed keystone PBT (§8.11
//! highest-available driver).**
//!
//! [`LiveMcpE2E`] is a sibling [`ComposedSlice`] to [`WideE2E`] whose SUT is a
//! REAL Holon app reached over its embedded MCP server ([`McpUserDriver`]) —
//! the iOS simulator, a desktop GPUI window, anything serving MCP at
//! `http://127.0.0.1:$MCP_SERVER_PORT/mcp`. It reuses the EXACT same reference
//! machine ([`WideE2EMachine`] transitions/preconditions/apply, delegated by
//! [`WideE2ELiveMcpMachine`]) and the EXACT same composed invariant catalog as
//! the headless keystone; only the SUT axis changes — from an in-process
//! `compose_sut(full_headless)` `CapMap` to a `CapMap` whose providers speak
//! MCP.
//!
//! Honesty (never fake a cap): the live rung registers ONLY the caps the MCP
//! surface can answer truthfully — the block/SQL/Loro/org READ caps and the
//! gesture WRITE caps the [`UserDriver`] drives through the app's real input
//! pipeline. Caps that need an in-process handle (peer Loro docs, the live
//! `ReactiveEngine`/window geometry, the editor mirror) are simply ABSENT; the
//! captured [`CapSet`] narrows both the generated alphabet and the per-tick
//! non-vacuity floor accordingly, exactly as the windowed sibling does.
//!
//! Per-case isolation is an in-process `reset_vault` on the server (Phase 1
//! Option A): [`LiveMcpE2E::build`] rebuilds the vault from the embedded
//! `scripts/seed_wide/*.org` seed and self-checks the resulting `block_raw` id
//! set, so every proptest case starts from the same deterministic tree.
//!
//! @pbt kind infra
//! @pbt covers live-mcp-slice — out-of-process twin of the keystone over a live
//! MCP app

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

use holon_api::Block;
use holon_api::EntityUri;
use holon_api::Key;
use holon_api::KeyChord;
use holon_api::Value;
use holon_api::block::BlockWire;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::user_driver::UserDriver;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::capabilities::SutBlockTreeWrite;
use holon_pbt_core::capabilities::SutDenseTools;
use holon_pbt_core::capabilities::SutEditorMirrorWrite;
use holon_pbt_core::capabilities::SutFocusWrite;
use holon_pbt_core::capabilities::SutLoroLog;
use holon_pbt_core::capabilities::SutOrgRead;
use holon_pbt_core::capabilities::SutOrgRender;
use holon_pbt_core::capabilities::SutQuiesce;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::composition::CapSet;
use holon_pbt_core::composition::InvariantId;
use proptest::strategy::BoxedStrategy;
use proptest::strategy::Strategy;
use proptest_state_machine::ReferenceStateMachine;

use crate::McpUserDriver;
use crate::pbt::composed::harness::ComposedSlice;
use crate::pbt::composed::harness::sut_ids;
use crate::pbt::composed::seed_primitives::fixed_ids;
use crate::pbt::composed::wide_e2e::WideE2E;
use crate::pbt::composed::wide_e2e::WideE2EMachine;
use crate::pbt::composed::wide_e2e::page_root;
use crate::pbt::composed::wide_e2e::wide_e2e_windowed_ref;
use crate::pbt::op_write_cap::IdResolver;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::sut_row_parsing::BLOCK_RAW_SNAPSHOT_SQL;
use crate::pbt::sut_row_parsing::parse_block_rows;
use crate::pbt::transitions::E2ETransition;

// ── Embedded seed (include_str! so the test and the on-disk seed can't drift)
// ──

/// The structural working page — MUST stay byte-identical to
/// `wide_e2e::WIDE_TREE_ORG` (asserted by [`tests::seed_wide_stays_aligned`]).
const SEED_STRUCTURAL_ORG: &str = include_str!("../../../scripts/seed_wide/structural-page.org");
/// The default layout (sidebars + main panel) the live app boots — the same
/// `assets/default/index.org` the iOS app seeds, pinned to a fixed `#+ID:` so
/// the rebuilt vault's layout doc id is deterministic across resets.
const SEED_INDEX_ORG: &str = include_str!("../../../scripts/seed_wide/index.org");
/// The first-boot journals page.
const SEED_JOURNALS_ORG: &str = include_str!("../../../scripts/seed_wide/Journals.org");

/// `await_quiescence` budget — matches the headless `converge_projections` cap.
const QUIESCE_BUDGET_MS: u64 = 30_000;

/// Working-tree block ids that MUST be present in the rebuilt vault's
/// `block_raw` (the `reset_vault` self-check). Everything else the app boots
/// (layout, journals, `__default__`) is scaffold.
const EXPECTED_WORKING_IDS: &[&str] = &["block:parent", "block:c1", "block:c2"];

// ─────────────────────────────────────────────────────────────────────────────
// The MCP-backed cap provider
// ─────────────────────────────────────────────────────────────────────────────

/// One provider backing every live cap: the read caps query over MCP, the
/// gesture-write caps drive the app's real input pipeline through the
/// [`UserDriver`] verbs, and quiescence is the server's `await_quiescence`.
pub struct LiveMcp {
    driver: Arc<McpUserDriver>,
    resolver: IdResolver,
}

impl LiveMcp {
    fn resolve(&self, id: &EntityUri) -> EntityUri {
        self.resolver
            .lock()
            .expect("resolver lock")
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.clone())
    }

    async fn snapshot(&self) -> serde_json::Value {
        self.driver
            .call_tool_json("debug_pbt_snapshot", serde_json::json!({}))
            .await
            .expect("debug_pbt_snapshot over MCP failed")
    }

    /// Run raw SQL and return the `rows` array as JSON objects.
    async fn rows(&self, sql: &str) -> Vec<serde_json::Map<String, serde_json::Value>> {
        let resp = self
            .driver
            .execute_raw_sql(sql)
            .await
            .unwrap_or_else(|e| panic!("execute_raw_sql over MCP failed for {sql:?}: {e:#}"));
        resp.get("rows")
            .and_then(|r| r.as_array())
            .expect("execute_raw_sql response missing `rows` array")
            .iter()
            .map(|row| {
                row.as_object()
                    .cloned()
                    .expect("execute_raw_sql row is not a JSON object")
            })
            .collect()
    }

    /// The block_raw `content` for an already-resolved id (byte→keystroke
    /// source).
    async fn block_raw_content(&self, resolved: &EntityUri) -> String {
        let rows = self
            .rows(&format!(
                "SELECT content FROM block_raw WHERE id = {}",
                sql_lit(resolved)
            ))
            .await;
        rows.first()
            .and_then(|r| r.get("content"))
            .map(json_to_string)
            .unwrap_or_default()
    }

    /// The Main-region focused block (for caret-relative editor writes).
    async fn current_main_focus(&self) -> Option<EntityUri> {
        let rows = self
            .rows("SELECT block_id FROM current_focus WHERE region = 'main'")
            .await;
        rows.first()
            .and_then(|r| r.get("block_id"))
            .filter(|v| !v.is_null())
            .map(|v| EntityUri::parse(&json_to_string(v)).expect("current_focus block_id URI"))
    }

    async fn focus_editor(&self, resolved: &EntityUri, ctx: &str) {
        // Drive the SAME single `click_entity` call real MCP dogfooding does.
        // The retry-until-committed for a freshly-(re)rendered element (e.g. a
        // `:__virtual:` creation slot that has no committed bounds for a frame
        // or two) now lives INSIDE `click_entity` — not in this test-only
        // wrapper. Keeping the retry here would let the E2E paper over a prod
        // regression the way the dogfood #3 bug escaped: real MCP callers
        // never had the wrapper's retry, so the bug only reproduced outside
        // the test. Any failure now is a genuine driver failure — fail loud.
        let budget = Duration::from_secs(10);
        if let Err(e) = self.driver.click_entity(resolved, "main").await {
            panic!("[{ctx}] focus {resolved} over MCP failed: {e:#}");
        }

        // The click's editor focus lands a frame LATER on iOS (idle rendering:
        // often the next keystroke is what drives the frame that applies focus),
        // so caret keystrokes sent immediately would miss the editor. Wait until
        // the engine's authoritative focus IS this block before returning — fail
        // loud on timeout with the last seen focus.
        let want = resolved.to_string();
        let poll_start = std::time::Instant::now();
        loop {
            let last_seen = self.snapshot().await["focused_block"].clone();
            if json_to_string(&last_seen) == want {
                return;
            }
            if poll_start.elapsed() > budget {
                panic!(
                    "[{ctx}] focus {resolved} clicked but engine focus never landed within \
                     {budget:?} (last seen focused_block = {last_seen}); the click→keystroke \
                     focus race is unresolved"
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn key(&self, keystroke: &str, modifiers: &[&str], ctx: &str) {
        self.driver
            .send_raw_keystroke(keystroke, modifiers)
            .await
            .unwrap_or_else(|e| {
                panic!("[{ctx}] key {keystroke} {modifiers:?} over MCP failed: {e:#}")
            });
    }

    /// Drive a block-reorder op through the app's chord-resolution path,
    /// exactly like `KeystrokeBlockTreeWriter::send_block_chord` — but with
    /// the fixed production binding (Alt+Up / Alt+Down; the live registry
    /// is in-process and unreadable over MCP). `send_key_chord`'s
    /// `root`/`tree` args are ignored by [`McpUserDriver`] (the app bubbles
    /// through its own tree), so a default VM is sound.
    async fn send_reorder_chord(&self, resolved: &EntityUri, arrow: Key, ctx: &str) {
        let chord = KeyChord([Key::Alt, arrow].into_iter().collect());
        let root = holon_api::root_layout_block_uri();
        let tree = ReactiveViewModel::default();
        let dispatched = self
            .driver
            .send_key_chord(
                &root,
                &tree,
                resolved,
                &chord,
                std::collections::HashMap::new(),
            )
            .await
            .unwrap_or_else(|e| {
                panic!("[{ctx}] send_key_chord {chord:?} on {resolved} failed: {e:#}")
            });
        assert!(
            dispatched,
            "[{ctx}] chord {chord:?} did not dispatch a reorder on {resolved}"
        );
    }
}

// ── Read caps ────────────────────────────────────────────────────────────────

#[async_trait::async_trait(?Send)]
impl SutBackend for LiveMcp {
    async fn live_block_snapshot(&self) -> Vec<Block> {
        wires_to_blocks(&self.snapshot().await["live_blocks"])
    }

    async fn block_raw_snapshot(&self) -> Vec<Block> {
        let rows = self.rows(BLOCK_RAW_SNAPSHOT_SQL).await;
        let entities: Vec<holon_core::storage::types::StorageEntity> =
            rows.iter().map(json_row_to_storage_entity).collect();
        parse_block_rows(&entities)
    }

    async fn live_focus_root_rows(&self) -> Vec<(String, String)> {
        self.snapshot().await["focus_roots"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|r| (json_to_string(&r["region"]), json_to_string(&r["root_id"])))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[async_trait::async_trait(?Send)]
impl SutSqlProjection for LiveMcp {
    async fn block_row(&self, id: &EntityUri) -> Option<Vec<String>> {
        let rows = self
            .rows(&format!(
                "SELECT id, parent_id, content, content_type, source_language, properties, tags, \
                 requires FROM block WHERE id = {}",
                sql_lit(id)
            ))
            .await;
        rows.first().map(row_to_string_vec)
    }

    async fn all_block_ids(&self) -> BTreeSet<EntityUri> {
        self.rows("SELECT id FROM block")
            .await
            .iter()
            .map(id_of_row)
            .collect()
    }

    async fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri> {
        self.rows(&format!(
            "SELECT id FROM block WHERE parent_id = {} ORDER BY sort_key",
            sql_lit(parent)
        ))
        .await
        .iter()
        .map(id_of_row)
        .collect()
    }

    /// Watches are not driven over MCP → honest `None` (the watch invariants
    /// deselect; no `SutWatch` cap is registered either).
    async fn watch_row_count(&self, _: &str) -> Option<usize> {
        None
    }

    async fn block_raw_row(&self, id: &EntityUri) -> Option<Vec<String>> {
        let rows = self
            .rows(&format!(
                "SELECT id, parent_id, content, content_type, source_language, properties FROM \
                 block_raw WHERE id = {}",
                sql_lit(id)
            ))
            .await;
        rows.first().map(row_to_string_vec)
    }

    async fn block_tag_block_ids(&self) -> BTreeSet<EntityUri> {
        self.rows("SELECT DISTINCT block_id AS id FROM block_tags")
            .await
            .iter()
            .map(id_of_row)
            .collect()
    }

    async fn block_task_state(&self, id: &EntityUri) -> Option<String> {
        let rows = self
            .rows(&format!(
                "SELECT json_extract(properties, '$.task_state') AS task_state FROM block_raw \
                 WHERE id = {}",
                sql_lit(id)
            ))
            .await;
        rows.first()
            .and_then(|r| r.get("task_state"))
            .filter(|v| !v.is_null())
            .map(json_to_string)
    }

    async fn block_content(&self, id: &EntityUri) -> Option<String> {
        let rows = self
            .rows(&format!(
                "SELECT content FROM block_raw WHERE id = {}",
                sql_lit(id)
            ))
            .await;
        rows.first()
            .and_then(|r| r.get("content"))
            .map(json_to_string)
    }
}

#[async_trait::async_trait(?Send)]
impl SutLoroLog for LiveMcp {
    async fn loro_had_errors(&self) -> bool {
        self.snapshot().await["loro_had_errors"]
            .as_bool()
            .unwrap_or(false)
    }

    async fn loro_children_of(&self, parent_stable_id: &str) -> Option<Vec<String>> {
        self.snapshot().await["loro_tree_children"]
            .get(parent_stable_id)
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().map(json_to_string).collect())
    }

    async fn loro_lamport_height(&self) -> Option<u32> {
        self.snapshot().await["lamport_height"]
            .as_u64()
            .map(|h| h as u32)
    }

    async fn loro_block_snapshot(&self) -> Option<Vec<Block>> {
        // Union every loaded Loro doc's blocks (the app is Loro-authority — this
        // is the live tree, bypassing SQL). `inspect_loro_blocks` returns the same
        // `BlockWire` shape `debug_pbt_snapshot.live_blocks` carries.
        let docs = self.list_documents().await;
        let mut out = Vec::new();
        for doc_id in docs {
            let resp = self
                .driver
                .call_tool_json(
                    "inspect_loro_blocks",
                    serde_json::json!({ "doc_id": doc_id }),
                )
                .await
                .unwrap_or_else(|e| panic!("inspect_loro_blocks({doc_id}) failed: {e:#}"));
            out.extend(wires_to_blocks(&resp["blocks"]));
        }
        Some(out)
    }
}

#[async_trait::async_trait(?Send)]
impl SutOrgRead for LiveMcp {
    async fn org_block_snapshot(&self) -> Vec<Block> {
        use holon_orgmode::parser::parse_org_file;
        let mut all = Vec::new();
        for (alias, _) in self.oracle_org_aliases().await {
            let (path, content) = self.read_org(&alias).await;
            let root = path.parent().unwrap_or_else(|| Path::new(""));
            let result = parse_org_file(&path, &content, &EntityUri::no_parent(), root)
                .unwrap_or_else(|e| panic!("SutOrgRead: parse {} failed: {e:#}", path.display()));
            all.extend(result.blocks);
        }
        all
    }
}

#[async_trait::async_trait(?Send)]
impl SutOrgRender for LiveMcp {
    async fn snapshot_org_render_pairs(&self) -> Vec<(String, String, String)> {
        let mut out = Vec::new();
        for (alias, _) in self.list_org_aliases().await {
            let (path, disk) = self.read_org(&alias).await;
            let resp = self
                .driver
                .call_tool_json(
                    "render_org",
                    serde_json::json!({ "doc_id": alias, "source": "sql", "scope": "document" }),
                )
                .await
                .unwrap_or_else(|e| panic!("render_org({alias}) over MCP failed: {e:#}"));
            let rendered = resp["rendered"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("render_org({alias}) response missing `rendered`: {resp}")
                })
                .to_string();
            out.push((path.to_string_lossy().to_string(), disk, rendered));
        }
        out
    }
}

impl LiveMcp {
    /// Doc ids of every loaded Loro document — on this app that is the ONE
    /// global `holon_tree` doc; `loro_block_snapshot` unions these ("every
    /// block held in the live Loro tree").
    async fn list_documents(&self) -> Vec<String> {
        let resp = self
            .driver
            .call_tool_json("list_loro_documents", serde_json::json!({}))
            .await
            .expect("list_loro_documents over MCP failed");
        resp["documents"]
            .as_array()
            .map(|arr| arr.iter().map(|d| json_to_string(&d["doc_id"])).collect())
            .unwrap_or_default()
    }

    /// `(alias id, file path)` of every tracked org FILE (uuid → *.org path
    /// mappings). Org files are not separate Loro docs — they are aliases into
    /// the global tree — so the org/render caps iterate these, mirroring the
    /// headless `SutOrgRead` ("parse every tracked org file on disk").
    async fn list_org_aliases(&self) -> Vec<(String, String)> {
        let resp = self
            .driver
            .call_tool_json("list_loro_documents", serde_json::json!({}))
            .await
            .expect("list_loro_documents over MCP failed");
        let aliases: Vec<(String, String)> = resp["aliases"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter(|a| json_to_string(&a["file_path"]).ends_with(".org"))
                    .map(|a| (json_to_string(&a["alias"]), json_to_string(&a["file_path"])))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !aliases.is_empty(),
            "no org-file aliases in the live Loro store — org caps would silently compare empty \
             snapshots"
        );
        aliases
    }

    /// The org files the ORACLE models: the composed headless boot registers
    /// only the working-tree doc in its `FileAdapterState` (index/Journals are
    /// engine-seeded scaffold with no reference org face), so the ref↔SUT org
    /// comparison is scoped to exactly these.
    async fn oracle_org_aliases(&self) -> Vec<(String, String)> {
        const ORACLE_TRACKED_ORG_FILES: &[&str] = &["structural-page.org"];
        self.list_org_aliases()
            .await
            .into_iter()
            .filter(|(_, path)| {
                Path::new(path)
                    .file_name()
                    .is_some_and(|n| ORACLE_TRACKED_ORG_FILES.contains(&n.to_str().unwrap()))
            })
            .collect()
    }

    /// `(file_path, disk_bytes)` for a doc via `read_org_file`.
    async fn read_org(&self, doc_id: &str) -> (std::path::PathBuf, String) {
        let resp = self
            .driver
            .call_tool_json("read_org_file", serde_json::json!({ "doc_id": doc_id }))
            .await
            .unwrap_or_else(|e| panic!("read_org_file({doc_id}) failed: {e:#}"));
        (
            std::path::PathBuf::from(json_to_string(&resp["file_path"])),
            json_to_string(&resp["content"]),
        )
    }
}

// ── Gesture-write caps (the app's real input pipeline over MCP)
// ───────────────
//
// The headless `KeystrokeBlockTreeWriter`/`*_via` bodies cannot be reused here:
// they read an in-process `ReactiveEngine` (live editor text, the chord
// registry) which does not exist across a process boundary. The keystroke
// SEQUENCES are the same production ones, dispatched through the same
// `UserDriver` verbs; the byte→ keystroke conversion reads live text over SQL
// and the settle is the server's `await_quiescence` (driven by the slice's
// `settle_after_apply`).

#[async_trait::async_trait(?Send)]
impl SutBlockTreeWrite for LiveMcp {
    async fn apply_split_block(&self, id: &EntityUri, position: usize) {
        let resolved = self.resolve(id);
        self.focus_editor(&resolved, "SplitBlock").await;
        let text = self.block_raw_content(&resolved).await;
        assert!(
            text.is_char_boundary(position),
            "[SplitBlock] position {position} not a char boundary of {text:?}"
        );
        let rights = text[..position].chars().count();
        self.key("home", &[], "SplitBlock").await;
        for _ in 0..rights {
            self.key("right", &[], "SplitBlock").await;
        }
        self.key("enter", &[], "SplitBlock").await;
    }

    async fn apply_join_block(&self, id: &EntityUri) {
        let resolved = self.resolve(id);
        self.focus_editor(&resolved, "JoinBlock").await;
        self.key("home", &[], "JoinBlock").await;
        self.key("backspace", &[], "JoinBlock").await;
    }

    async fn apply_indent(&self, id: &EntityUri) {
        let resolved = self.resolve(id);
        self.focus_editor(&resolved, "Indent").await;
        self.key("tab", &[], "Indent").await;
    }

    async fn apply_outdent(&self, id: &EntityUri) {
        let resolved = self.resolve(id);
        self.focus_editor(&resolved, "Outdent").await;
        self.key("tab", &["shift"], "Outdent").await;
    }

    async fn apply_move_up(&self, id: &EntityUri) {
        let resolved = self.resolve(id);
        self.send_reorder_chord(&resolved, Key::Up, "MoveBlockUp")
            .await;
    }

    async fn apply_move_down(&self, id: &EntityUri) {
        let resolved = self.resolve(id);
        self.send_reorder_chord(&resolved, Key::Down, "MoveBlockDown")
            .await;
    }
}

#[async_trait::async_trait(?Send)]
impl SutFocusWrite for LiveMcp {
    async fn apply_navigate_focus(&self, region: CapRegion, id: &EntityUri) {
        // At phone width the sidebar is a CLOSED drawer (if_space 600) — its
        // entries have no committed bounds, so the headless sidebar-click path
        // cannot be honest here. Dispatch the SAME `navigation.focus` op the
        // sidebar click-intent resolves to, one level below the click.
        let resolved = self.resolve(id);
        let mut params = std::collections::HashMap::new();
        params.insert(
            "region".to_string(),
            holon_api::Value::String(format!("{region:?}").to_lowercase()),
        );
        params.insert(
            "block_id".to_string(),
            holon_api::Value::String(resolved.to_string()),
        );
        self.driver
            .synthetic_dispatch("navigation", "focus", params)
            .await
            .unwrap_or_else(|e| {
                panic!("[NavigateFocus] navigation.focus({resolved}) over MCP failed: {e:#}")
            });
    }

    async fn apply_focus_editable_text(&self, id: &EntityUri) {
        let resolved = self.resolve(id);
        self.focus_editor(&resolved, "FocusEditableText").await;
    }
}

#[async_trait::async_trait(?Send)]
impl SutEditorMirrorWrite for LiveMcp {
    async fn apply_type_chars(&self, text: &str) {
        for ch in text.chars() {
            self.key(&ch.to_string(), &[], "TypeChars").await;
        }
    }

    async fn apply_delete_backward(&self, count: usize) {
        for _ in 0..count {
            self.key("backspace", &[], "DeleteBackward").await;
        }
    }

    async fn apply_move_cursor(&self, byte_position: usize) {
        // Caret is relative to the Main-focused block's live text (mirrors
        // `apply_move_cursor_via`, which reads `reactive.focused_block()`).
        let block = self
            .current_main_focus()
            .await
            .expect("[MoveCursor] no Main-focused block — FocusEditableText must run first");
        let text = self.block_raw_content(&block).await;
        assert!(
            text.is_char_boundary(byte_position),
            "[MoveCursor] byte_position {byte_position} not a char boundary of {text:?}"
        );
        let rights = text[..byte_position].chars().count();
        self.key("home", &[], "MoveCursor").await;
        for _ in 0..rights {
            self.key("right", &[], "MoveCursor").await;
        }
    }
}

#[async_trait::async_trait(?Send)]
impl SutQuiesce for LiveMcp {
    async fn quiesce(&self) {
        await_quiescence(&self.driver).await;
    }
}

/// `SutDenseTools` (the `DenseProjectionEdit` transition): the agent-facing
/// dense_query → edit → dense_patch round trip through the REAL MCP tool → op
/// path. Every error is a loud panic — the ref has already applied the append,
/// so a swallowed failure would mis-diagnose as a block-set divergence.
#[async_trait::async_trait(?Send)]
impl SutDenseTools for LiveMcp {
    async fn dense_append_child(&self, parent: &EntityUri, content: &str) {
        let resolved = self.resolve(parent);
        let query = format!(
            "SELECT * FROM block WHERE parent_id = '{}' ORDER BY sort_key",
            resolved.as_str().replace('\'', "''")
        );
        // A positioned create is now a SINGLE create op carrying
        // `after_block_id` (unified positional-create key, 2026-07-27) — the
        // create-then-move seam and its cross-provider visibility race are
        // gone, so the transition asserts DIRECT success with no retry.
        let proj = self
            .driver
            .call_tool_json(
                "dense_query",
                serde_json::json!({ "query": query, "language": "holon_sql" }),
            )
            .await
            .unwrap_or_else(|e| {
                panic!("[DenseProjectionEdit] dense_query for {resolved} failed: {e:#}")
            });
        let handle = proj["projection_handle"].as_str().unwrap_or_else(|| {
            panic!("[DenseProjectionEdit] dense_query response missing projection_handle: {proj}")
        });
        let dense = proj["dense_org"].as_str().unwrap_or_else(|| {
            panic!("[DenseProjectionEdit] dense_query response missing dense_org: {proj}")
        });
        let rows = proj["block_count"].as_u64().unwrap_or_else(|| {
            panic!("[DenseProjectionEdit] dense_query response missing block_count: {proj}")
        });
        // The generator guarantees ≥1 existing child; an empty projection
        // would anchor to SYNTHETIC_ROOT (page context lost) and silently
        // change the append target — fail loud instead.
        assert!(
            rows >= 1,
            "[DenseProjectionEdit] projection for {resolved} is empty (dense_org = \
             {dense:?}) — generator precondition (≥1 child) not honored by the live projection"
        );
        let mut edited = dense.to_string();
        if !edited.ends_with('\n') {
            edited.push('\n');
        }
        edited.push_str(&format!("* {content}\n"));
        self.driver
            .call_tool_json(
                "dense_patch",
                serde_json::json!({ "handle": handle, "text": edited }),
            )
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "[DenseProjectionEdit] dense_patch appending {content:?} under {resolved} \
                     failed: {e:#}"
                )
            });
    }

    async fn dense_move_first_child_to_end(&self, parent: &EntityUri) {
        let resolved = self.resolve(parent);
        let query = format!(
            "SELECT * FROM block WHERE parent_id = '{}' ORDER BY sort_key",
            resolved.as_str().replace('\'', "''")
        );
        let proj = self
            .driver
            .call_tool_json(
                "dense_query",
                serde_json::json!({ "query": query, "language": "holon_sql" }),
            )
            .await
            .unwrap_or_else(|e| {
                panic!("[DenseProjectionEdit] dense_query for {resolved} failed: {e:#}")
            });
        let handle = proj["projection_handle"].as_str().unwrap_or_else(|| {
            panic!("[DenseProjectionEdit] dense_query response missing projection_handle: {proj}")
        });
        let dense = proj["dense_org"].as_str().unwrap_or_else(|| {
            panic!("[DenseProjectionEdit] dense_query response missing dense_org: {proj}")
        });
        // Split into the `#+ID:` header and the top-level ROWS (a row = its
        // `* ` headline plus any continuation lines, e.g. a property drawer);
        // move the first row (with its `{#alias}` token) to the end. The
        // generator guarantees >= 2 children, so fewer rows is a loud drift.
        let mut lines = dense.lines();
        let header = lines.next().unwrap_or_default();
        assert!(
            header.starts_with("#+ID:"),
            "[DenseProjectionEdit] dense_org for {resolved} missing #+ID: header: {dense:?}"
        );
        let mut rows: Vec<Vec<&str>> = Vec::new();
        for line in lines {
            if line.starts_with('*') || rows.is_empty() {
                rows.push(vec![line]);
            } else {
                rows.last_mut().expect("rows non-empty checked").push(line);
            }
        }
        assert!(
            rows.len() >= 2,
            "[DenseProjectionEdit] projection for {resolved} has {} rows, need >= 2 for a move \
             (dense_org = {dense:?}) — generator precondition not honored by the live projection",
            rows.len()
        );
        let first = rows.remove(0);
        rows.push(first);
        let mut edited = String::from(header);
        edited.push('\n');
        for row in &rows {
            for l in row {
                edited.push_str(l);
                edited.push('\n');
            }
        }
        self.driver
            .call_tool_json(
                "dense_patch",
                serde_json::json!({ "handle": handle, "text": edited }),
            )
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "[DenseProjectionEdit] dense_patch move-first-to-end under {resolved} \
                     failed: {e:#}"
                )
            });
    }
}

/// Register every cap the live rung honestly provides. The captured [`CapSet`]
/// of the resulting map is what narrows the generated alphabet + non-vacuity
/// floor. Absent (honest) caps: `SutLoro` (peer docs), the renderer / ViewModel
/// / window-geometry / editor-mirror-read / watch / nav-history / view-control
/// / undo / seam-mutate / lifecycle caps — none answerable over the current MCP
/// surface, so the transitions/invariants that need them DESELECT (disclosed
/// via the cap set), rather than being faked.
fn register_live_caps(caps: &mut CapMap, provider: Arc<LiveMcp>) {
    caps.insert(provider.clone() as Arc<dyn SutBackend>);
    caps.insert(provider.clone() as Arc<dyn SutSqlProjection>);
    caps.insert(provider.clone() as Arc<dyn SutLoroLog>);
    caps.insert(provider.clone() as Arc<dyn SutOrgRead>);
    caps.insert(provider.clone() as Arc<dyn SutOrgRender>);
    caps.insert(provider.clone() as Arc<dyn SutBlockTreeWrite>);
    caps.insert(provider.clone() as Arc<dyn SutFocusWrite>);
    caps.insert(provider.clone() as Arc<dyn SutEditorMirrorWrite>);
    caps.insert(provider.clone() as Arc<dyn SutDenseTools>);
    caps.insert(provider as Arc<dyn SutQuiesce>);
}

/// The `CapId`s [`register_live_caps`] provides — the live-MCP composition's
/// static cap surface, exposed for the non-vacuity guard so a transition alive
/// ONLY over live-MCP (e.g. `DenseProjectionEdit` via `SutDenseTools`, which no
/// `blessed_manifests` compose_sut registers) still counts as ALIVE — the CAP
/// is the evidence, NOT a name allowlist. MUST stay in sync with the inserts in
/// [`register_live_caps`] above: it inserts typed `Arc`s, which needs a live
/// provider (a real MCP connection), so the guard cannot derive this by
/// booting; co-located here so a cap added above is added here too.
pub fn live_mcp_cap_ids() -> Vec<holon_pbt_core::composition::CapId> {
    use holon_pbt_core::composition::CapId;
    vec![
        CapId::of::<dyn SutBackend>(),
        CapId::of::<dyn SutSqlProjection>(),
        CapId::of::<dyn SutLoroLog>(),
        CapId::of::<dyn SutOrgRead>(),
        CapId::of::<dyn SutOrgRender>(),
        CapId::of::<dyn SutBlockTreeWrite>(),
        CapId::of::<dyn SutFocusWrite>(),
        CapId::of::<dyn SutEditorMirrorWrite>(),
        CapId::of::<dyn SutDenseTools>(),
        CapId::of::<dyn SutQuiesce>(),
    ]
}

/// The browser-worker (dioxus-web) variant of [`register_live_caps`]. The
/// in-browser `holon-worker` engine is headless (`BackendEngine` +
/// `ReactiveEngine`, no `LoroDocumentStore` and no on-wasm org parser —
/// `holon-orgmode` pulls `notify`, which has no wasm backend), so the three
/// caps whose bodies call the Loro/org MCP tools that the worker rejects
/// (`SutLoroLog` → `inspect_loro_blocks`/`list_loro_documents`, `SutOrgRead` →
/// `read_org_file`, `SutOrgRender` → `render_org`) are OMITTED.
///
/// The gesture WRITE caps stay registered: the worker answers `click` /
/// `type_text` / `insert_text` / `send_key_chord` headlessly through the same
/// [`holon_frontend::user_driver::ReactiveEngineDriver`] the in-process
/// keystone uses (`HeadlessInputRouter` + `HeadlessEditorMirror`) — no window.
///
/// Absent caps narrow the generated alphabet and the per-tick non-vacuity
/// floor via the captured [`CapSet`], exactly as the honest-cap discipline
/// intends — no transition is special-cased.
fn register_live_caps_browser(caps: &mut CapMap, provider: Arc<LiveMcp>) {
    caps.insert(provider.clone() as Arc<dyn SutBackend>);
    caps.insert(provider.clone() as Arc<dyn SutSqlProjection>);
    // OMITTED (worker-unsupported): SutLoroLog, SutOrgRead, SutOrgRender.
    caps.insert(provider.clone() as Arc<dyn SutBlockTreeWrite>);
    caps.insert(provider.clone() as Arc<dyn SutFocusWrite>);
    caps.insert(provider.clone() as Arc<dyn SutEditorMirrorWrite>);
    caps.insert(provider as Arc<dyn SutQuiesce>);
}

/// Select the cap-registration function for the current live target. The
/// dioxus-web browser worker exposes a narrower MCP tool surface than a GPUI /
/// iOS app, selected by `HOLON_PBT_LIVE_MCP_BROWSER=1`. Both call sites
/// (`capture_live_cap_set` and `LiveMcpE2E::build`) route through here so the
/// captured cap set and the per-case build agree on the alphabet.
fn register_live_caps_for_target(caps: &mut CapMap, provider: Arc<LiveMcp>) {
    if std::env::var("HOLON_PBT_LIVE_MCP_BROWSER").is_ok() {
        register_live_caps_browser(caps, provider);
    } else {
        register_live_caps(caps, provider);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// The slice + machine
// ─────────────────────────────────────────────────────────────────────────────

/// The live windowed cap set, captured ONCE (a throwaway MCP connect) before
/// the proptest strategy is built — mirrors `wide_e2e::WINDOWED_CAP_SET`.
static LIVE_MCP_CAP_SET: OnceLock<CapSet> = OnceLock::new();

/// Capture the live rung's cap set (once per process). Connects to the app,
/// registers the live caps, reads the cap set, disconnects — NO `reset_vault`
/// (cap presence is static). MUST run before the strategy is built.
pub fn capture_live_cap_set() {
    if LIVE_MCP_CAP_SET.get().is_some() {
        return;
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build runtime for live cap-set capture");
    let cs = rt.block_on(async {
        let driver = Arc::new(
            McpUserDriver::connect_from_env()
                .await
                .expect("connect to live MCP app for cap-set capture"),
        );
        let resolver: IdResolver = Arc::new(Mutex::new(std::collections::BTreeMap::new()));
        let mut caps = CapMap::new();
        register_live_caps_for_target(&mut caps, Arc::new(LiveMcp { driver, resolver }));
        caps.cap_set()
    });
    drop(rt);
    let _ = LIVE_MCP_CAP_SET.set(cs);
}

/// The windowed sibling of [`WideE2EMachine`] for the live rung: identical
/// transition generation / preconditions / apply (delegated), but `init_state`
/// FIXES the oracle to the captured live [`CapSet`] instead of drawing
/// `any_valid_wiring()`. That set auto-narrows `aggregate_transitions` to
/// exactly what the MCP surface can drive and is the same set the per-tick
/// non-vacuity floor is computed against.
pub struct WideE2ELiveMcpMachine;

impl ReferenceStateMachine for WideE2ELiveMcpMachine {
    type State = ReferenceState;
    type Transition = E2ETransition;

    fn init_state() -> BoxedStrategy<Self::State> {
        use proptest::prelude::Just;
        let cap_set = LIVE_MCP_CAP_SET
            .get()
            .expect("LIVE_MCP_CAP_SET must be captured (capture_live_cap_set) before the strategy")
            .clone();
        Just(wide_e2e_windowed_ref(cap_set)).boxed()
    }

    fn transitions(state: &Self::State) -> BoxedStrategy<Self::Transition> {
        <WideE2EMachine as ReferenceStateMachine>::transitions(state)
    }

    fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
        <WideE2EMachine as ReferenceStateMachine>::preconditions(state, transition)
    }

    fn apply(state: Self::State, transition: &Self::Transition) -> Self::State {
        <WideE2EMachine as ReferenceStateMachine>::apply(state, transition)
    }
}

/// The live-MCP slice: the production `E2ETransition` alphabet over a REAL
/// Holon app reached via [`McpUserDriver`], per-case reset via `reset_vault`.
pub struct LiveMcpE2E;

impl ComposedSlice for LiveMcpE2E {
    type Transition = E2ETransition;
    type Machine = WideE2ELiveMcpMachine;
    /// The driver is the whole handle: `settle_after_apply` awaits quiescence
    /// and refreshes the UI snapshot through it.
    type Handle = Arc<McpUserDriver>;

    // The per-draw `required_invariants` override (delegated to `WideE2E`) derives
    // the floor from the drawn cap set, so no static list is needed.
    const REQUIRED_INVARIANTS: &'static [&'static str] = &[];
    const SETTLE: Duration = Duration::from_millis(QUIESCE_BUDGET_MS);
    const MULTI_THREAD: bool = true;

    async fn build(
        resolver: &IdResolver,
        ref_state: &ReferenceState,
    ) -> (CapMap, Self::Handle, BTreeSet<EntityUri>) {
        let driver = Arc::new(
            McpUserDriver::connect_from_env()
                .await
                .expect("connect to live MCP app (is it running and serving MCP?)"),
        );

        // Per-case isolation: rebuild the vault from the embedded seed and
        // self-check the resulting block_raw id set (fail loud on drift).
        let reset = driver
            .call_tool_json(
                "reset_vault",
                serde_json::json!({
                    "files": [
                        { "name": "structural-page.org", "content": SEED_STRUCTURAL_ORG },
                        { "name": "index.org", "content": SEED_INDEX_ORG },
                        { "name": "Journals.org", "content": SEED_JOURNALS_ORG },
                    ]
                }),
            )
            .await
            .expect("reset_vault over MCP failed (HOLON_MCP_ALLOW_RESET=1 on the app?)");
        let ids = reset["block_raw_ids"]
            .as_array()
            .unwrap_or_else(|| panic!("reset_vault response missing block_raw_ids array: {reset}"));
        let joined = ids.iter().map(json_to_string).collect::<Vec<_>>().join(",");
        for expected in EXPECTED_WORKING_IDS {
            assert!(
                joined.contains(expected),
                "reset_vault self-check: rebuilt vault is missing working block {expected:?} \
                 (block_raw_ids = {joined:?}) — the seed did not land deterministically"
            );
        }

        // Ids are pinned by `:ID:` drawers → the ref↔SUT map is the identity
        // (unmapped ids resolve to themselves; see seed_primitives), so the
        // resolver stays empty.
        let provider = Arc::new(LiveMcp {
            driver: driver.clone(),
            resolver: resolver.clone(),
        });
        let mut caps = CapMap::new();
        register_live_caps_for_target(&mut caps, provider);

        // Scaffold = everything booted OR modeled by the oracle EXCEPT the non-seed
        // working tree (parent/c1/c2) and `block:journals` (present + asserted on
        // both sides). Identical math to `boot_and_seed_wide`.
        let fixed = fixed_ids();
        let journals = EntityUri::parse("block:journals").expect("journals id");
        let tree: BTreeSet<EntityUri> = [fixed.parent, fixed.c1, fixed.c2].into_iter().collect();
        let booted = sut_ids(&caps).await;
        let ref_ids: BTreeSet<EntityUri> = ref_state
            .domain
            .block_state
            .blocks
            .keys()
            .cloned()
            .collect();
        let scaffold: BTreeSet<EntityUri> = booted
            .union(&ref_ids)
            .filter(|id| !tree.contains(id))
            .filter(|id| **id != journals)
            .cloned()
            .collect();

        // Fresh-drive the initial focus onto the page root, matching the oracle
        // (and `boot_and_seed_wide`) — the main panel renders the working tree
        // only once its focus_root points at the page.
        holon_pbt_core::TransitionImpl::apply_to_sut(
            &crate::pbt::transitions::NavigateFocus {
                region: holon_api::Region::Main,
                block_id: page_root(),
            },
            ref_state,
            &mut caps,
        )
        .await;
        await_quiescence(&driver).await;
        driver
            .refresh_ui(&page_root())
            .await
            .expect("refresh_ui after boot navigation failed");

        (caps, driver, scaffold)
    }

    async fn apply_transition(
        transition: &E2ETransition,
        ref_state: &ReferenceState,
        caps: &mut CapMap,
    ) {
        holon_pbt_core::TransitionImpl::apply_to_sut(transition, ref_state, caps).await;
    }

    /// Settle = the server's `await_quiescence` (budget exhaustion is an MCP
    /// tool error → fail loud), then refresh the UI snapshot so any
    /// gesture-observation read sees the post-transition frame.
    async fn settle_after_apply(handle: &Self::Handle, _: &CapMap) {
        await_quiescence(handle).await;
        handle
            .refresh_ui(&page_root())
            .await
            .expect("refresh_ui after settle failed");
    }

    /// Same per-draw non-vacuity floor as the headless keystone — derived from
    /// the drawn cap set, so a narrower live cap set auto-drops the ids it
    /// has no caps for. Delegated to `WideE2E` (single source of truth).
    fn required_invariants(ref_state: &ReferenceState) -> Vec<InvariantId> {
        <WideE2E as ComposedSlice>::required_invariants(ref_state)
    }
}

/// Await the app's quiescence over MCP; a budget-exhaustion tool error
/// propagates as a test failure (fail loud, never swallow).
async fn await_quiescence(driver: &McpUserDriver) {
    let resp = driver
        .call_tool_json(
            "await_quiescence",
            serde_json::json!({ "budget_ms": QUIESCE_BUDGET_MS }),
        )
        .await
        .expect("await_quiescence over MCP failed (budget exhausted or transport error)");
    assert!(
        resp["converged"].as_bool().unwrap_or(false),
        "await_quiescence did not converge: {resp}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// JSON ↔ domain helpers
// ─────────────────────────────────────────────────────────────────────────────

/// SQL string literal for an id, single-quotes escaped.
fn sql_lit(id: &EntityUri) -> String {
    format!("'{}'", id.as_str().replace('\'', "''"))
}

/// Stringify a JSON scalar the way the SQL/String cap surface expects (strings
/// unquoted; everything else via its compact JSON form).
fn json_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// The `id` column of a row as an `EntityUri`.
fn id_of_row(row: &serde_json::Map<String, serde_json::Value>) -> EntityUri {
    EntityUri::parse(&json_to_string(
        row.get("id").expect("row missing id column"),
    ))
    .expect("row id is a valid URI")
}

/// A row's values as Strings, in the SELECT column order.
fn row_to_string_vec(row: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    row.values().map(json_to_string).collect()
}

/// Convert an MCP JSON scalar into a `holon_api::Value` (the row-parser's value
/// type), so `parse_block_rows` can consume MCP `execute_raw_sql` output.
fn json_value_to_holon(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Value::Integer)
            .unwrap_or_else(|| Value::Float(n.as_f64().expect("JSON number is f64"))),
        serde_json::Value::String(s) => Value::String(s.clone()),
        serde_json::Value::Array(a) => Value::Array(a.iter().map(json_value_to_holon).collect()),
        serde_json::Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), json_value_to_holon(v)))
                .collect(),
        ),
    }
}

/// A JSON SQL row → the `StorageEntity` shape `parse_block_row` reads.
fn json_row_to_storage_entity(
    row: &serde_json::Map<String, serde_json::Value>,
) -> holon_core::storage::types::StorageEntity {
    row.iter()
        .map(|(k, v)| (Arc::<str>::from(k.as_str()), json_value_to_holon(v)))
        .collect()
}

/// A JSON array of `BlockWire` (the shape `inspect_loro_blocks` /
/// `debug_pbt_snapshot.live_blocks` return) → typed `Block`s.
fn wires_to_blocks(v: &serde_json::Value) -> Vec<Block> {
    v.as_array()
        .expect("expected a JSON array of BlockWire")
        .iter()
        .map(|w| {
            let wire: BlockWire = serde_json::from_value(w.clone()).expect("deserialize BlockWire");
            Block::from(wire)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The embedded `include_str!` seed MUST stay byte-aligned with the in-repo
    /// sources of truth, so the live rung seeds the SAME tree the headless
    /// keystone and the iOS app boot.
    #[test]
    fn seed_wide_stays_aligned() {
        // 1. structural-page.org IS the headless `WIDE_TREE_ORG`, byte-for-byte.
        assert_eq!(
            SEED_STRUCTURAL_ORG,
            crate::pbt::composed::wide_e2e::WIDE_TREE_ORG,
            "scripts/seed_wide/structural-page.org drifted from wide_e2e::WIDE_TREE_ORG"
        );

        // 2. index.org IS the iOS app's default layout (assets/default/index.org, the
        //    `DEFAULT_INDEX_ORG` const in frontends/gpui/src/mobile.rs) PLUS a pinned
        //    `#+ID:` header so the rebuilt vault's layout doc id is deterministic
        //    across resets. That const is private to the gpui crate (not importable),
        //    so we compare against the SAME on-disk asset it `include_str!`s —
        //    fail-loud note: this is the asset copy, not the const.
        const DEFAULT_INDEX_ORG: &str = include_str!("../../../../../assets/default/index.org");
        let body = SEED_INDEX_ORG
            .strip_prefix("#+ID: 15223f86-4b69-49b0-8ad7-c5b15fbc9f95\n")
            .expect(
                "scripts/seed_wide/index.org must start with the pinned `#+ID:` header \
                 (deterministic layout-doc id across resets)",
            );
        assert_eq!(
            body, DEFAULT_INDEX_ORG,
            "scripts/seed_wide/index.org body drifted from assets/default/index.org (the \
             DEFAULT_INDEX_ORG the iOS app boots)"
        );
    }

    /// DRIFT GUARD (hard gate). The browser worker has NO org parser
    /// (holon-orgmode won't build on wasm), so
    /// `frontends/holon-worker/src/seed.rs` HAND-CODES the `reset_vault`
    /// working page + journals as raw SQL. Those tuples MUST equal what the
    /// REAL org parser produces from `scripts/seed_wide/{structural-page,
    /// Journals}.org`. This test parses the same on-disk org and asserts
    /// the parse matches the tuples the worker hardcodes. If it fails,
    /// update BOTH `EXPECTED_*` below AND `seed.rs::seed_structural` /
    /// `seed_journals` together — they are one contract split across a wasm
    /// boundary.
    ///
    /// `sort_key` is intentionally NOT compared: the parser mints no sort_keys
    /// (holon-org-format `parser.rs`: "The parser must NOT mint keys" — the
    /// order owner assigns them on create). Document ORDER is guarded by each
    /// tuple's list position, ascending exactly as `seed.rs` assigns ascending
    /// sort_keys.
    #[test]
    fn seed_wide_matches_worker_seed() {
        use std::path::PathBuf;

        // (id, parent_id, content), doc first then blocks in document order.
        // MUST equal frontends/holon-worker/src/seed.rs `seed_structural`.
        const EXPECTED_STRUCTURAL: &[(&str, &str, &str)] = &[
            (
                "block:structural-page",
                "sentinel:no_parent",
                "structural-page",
            ),
            ("block:parent", "block:structural-page", "parent"),
            ("block:c1", "block:structural-page", "c1"),
            ("block:c2", "block:structural-page", "c2"),
        ];
        // MUST equal frontends/holon-worker/src/seed.rs `seed_journals`.
        const EXPECTED_JOURNALS: &[(&str, &str, &str)] =
            &[("block:journals", "sentinel:no_parent", "Journals")];

        for (name, org, expected) in [
            (
                "structural-page.org",
                SEED_STRUCTURAL_ORG,
                EXPECTED_STRUCTURAL,
            ),
            ("Journals.org", SEED_JOURNALS_ORG, EXPECTED_JOURNALS),
        ] {
            let path = PathBuf::from(format!("/seed/{name}"));
            let root = PathBuf::from("/seed");
            let parsed = holon_orgmode::parser::parse_org_file(
                &path,
                org,
                &holon_api::EntityUri::no_parent(),
                &root,
            )
            .unwrap_or_else(|e| panic!("parse {name}: {e}"));

            let actual: Vec<(String, String, String)> = std::iter::once(&parsed.document)
                .chain(parsed.blocks.iter())
                .map(|b| (b.id.to_string(), b.parent_id.to_string(), b.content.clone()))
                .collect();
            let expected_vec: Vec<(String, String, String)> = expected
                .iter()
                .map(|(i, p, c)| (i.to_string(), p.to_string(), c.to_string()))
                .collect();

            assert_eq!(
                actual, expected_vec,
                "{name}: parsed org blocks drifted from the browser worker's hardcoded seed \
                 (frontends/holon-worker/src/seed.rs). Update the worker seed AND EXPECTED_* \
                 together — they are one contract."
            );
        }
    }
}
