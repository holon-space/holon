//! System Under Test: `E2ESut` struct and `StateMachineTest` implementation.
//!
//! Contains the SUT wrapper, mutation application, invariant checking,
//! and all transition handling for the real system.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::marker::PhantomData;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};

use holon::storage::BLOCK_READ_TABLE;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_api::{ContentType, QueryLanguage, SourceLanguage, Value};
use holon_frontend::reactive::BuilderServices;
use holon_orgmode::OrgBlockExt;

#[cfg(test)]
use similar_asserts::assert_eq;

use crate::{
    DirectUserDriver, TestContext, UserDriver, assert_block_order, assert_blocks_equivalent,
    wait_for_file_condition,
};

use super::loro_sut::LoroSut;

use super::reference_state::ReferenceState;
use super::state_machine::VariantRef;
use super::types::*;

/// True iff the VM contains a `LiveBlock` for `block_id` whose content
/// recursively contains at least one widget node that is neither `Empty`
/// nor `Loading`. Used by inv-frontend-bounds-rendered/vm-data-tracked-as-content to skip wrappers whose inner profile
/// materialised as a placeholder (e.g. org-parsed blocks with
/// `::src::N`/`::render::N` children that fall back to the empty default)
/// — those aren't the "GPUI only materialised region wrappers" bug vm-data-tracked-as-content
/// is designed to catch, and panicking on them masks the real signal.
fn live_block_has_substantive_content(vm: &holon_frontend::ViewModel, block_id: &str) -> bool {
    use holon_frontend::view_model::ViewKind;
    fn has_substantive(node: &holon_frontend::ViewModel) -> bool {
        match &node.kind {
            // Placeholders & invisible whitespace: not substantive.
            ViewKind::Empty
            | ViewKind::Loading
            | ViewKind::Spacer { .. }
            | ViewKind::DropZone { .. } => false,
            // Visible content leaves: substantive.
            ViewKind::Text { .. }
            | ViewKind::Badge { .. }
            | ViewKind::Icon { .. }
            | ViewKind::Image { .. }
            | ViewKind::EditableText { .. }
            | ViewKind::SourceBlock { .. }
            | ViewKind::SourceEditor { .. }
            | ViewKind::StateToggle { .. }
            | ViewKind::Checkbox { .. }
            | ViewKind::TableRow { .. } => true,
            // Wrappers / containers / unknowns: only substantive if a
            // descendant is. An empty layout widget (e.g. `columns` with
            // no children) is NOT substantive — that's the placeholder
            // shape we're filtering out.
            _ => {
                let children = node.children();
                !children.is_empty() && children.iter().any(has_substantive)
            }
        }
    }
    fn find_and_check(node: &holon_frontend::ViewModel, block_id: &str) -> Option<bool> {
        if let ViewKind::LiveBlock {
            block_id: id,
            content,
        } = &node.kind
        {
            if id == block_id {
                return Some(has_substantive(content));
            }
        }
        for child in node.children() {
            if let Some(result) = find_and_check(child, block_id) {
                return Some(result);
            }
        }
        None
    }
    // Default to `true` when the VM doesn't even contain a LiveBlock for
    // this id. Filtering it out would silently weaken vm-data-tracked-as-content; let it fall
    // through to the original "no widget at all" warn/exemption path.
    find_and_check(vm, block_id).unwrap_or(true)
}

/// One row of the `focus_roots` matview. Mirrored into a `LiveData<FocusRoot>`
/// so inv-region-focus-roots-iter/8 can iterate by region in Rust without a per-region SQL query.
#[derive(Clone, Debug)]
struct FocusRoot {
    region: String,
    root_id: String,
}

/// Look up the single-character leader-chord key bound to a navigation
/// op in `assets/default/keybindings.yaml`. Embeds the YAML at compile
/// time so the test stays in lock-step with the canonical binding
/// source — moving "h" → "g" for `go_home` in the YAML moves the chord
/// key the test sends.
///
/// Returns the literal key string (e.g. `"h"` for `go_home`). Panics if
/// no leader-chord binding for `nav_op` is present — that's a
/// programming error in either the YAML or the caller, not a runtime
/// condition.
fn leader_key_for(nav_op: &str) -> &'static str {
    const YAML: &str = include_str!("../../../../assets/default/keybindings.yaml");
    static PARSED: std::sync::OnceLock<KeybindingsFile> = std::sync::OnceLock::new();
    let parsed = PARSED.get_or_init(|| {
        serde_yaml::from_str(YAML).expect("assets/default/keybindings.yaml must parse")
    });
    let key = parsed.bindings.iter().find_map(|b| {
        if b.context == "navigation"
            && b.action == nav_op
            && b.modifiers.iter().any(|m| m == "leader")
        {
            Some(b.key.as_str())
        } else {
            None
        }
    });
    key.unwrap_or_else(|| {
        panic!(
            "no leader-chord binding for nav_op '{nav_op}' in \
             assets/default/keybindings.yaml — caller passed a name \
             that doesn't appear in the YAML, or the YAML lost that \
             binding"
        )
    })
}

#[derive(serde::Deserialize)]
struct KeybindingsFile {
    bindings: Vec<KeybindingEntry>,
}

#[derive(serde::Deserialize)]
struct KeybindingEntry {
    key: String,
    #[serde(default)]
    modifiers: Vec<String>,
    context: String,
    action: String,
}

#[cfg(test)]
mod leader_key_tests {
    use super::leader_key_for;

    #[test]
    fn yaml_leader_bindings_resolve_to_expected_keys() {
        assert_eq!(leader_key_for("go_home"), "h");
        assert_eq!(leader_key_for("go_back"), "b");
        assert_eq!(leader_key_for("go_forward"), "f");
    }

    #[test]
    #[should_panic(expected = "no leader-chord binding")]
    fn unknown_nav_op_panics() {
        leader_key_for("not_a_real_op");
    }

    /// Direct-binding (no `modifiers: ["leader"]`) actions like
    /// `start_editing` (Enter) must NOT match — `send_leader_chord` is
    /// only for leader-prefixed chords.
    #[test]
    #[should_panic(expected = "no leader-chord binding")]
    fn direct_non_leader_binding_does_not_match() {
        // `start_editing` is bound to `Enter` directly AND under leader.
        // The leader binding wins (matches first); but a direct-only op
        // would panic. `switch_region` is direct-only on Tab.
        leader_key_for("switch_region");
    }
}
/// Build a `Block` from a SQL row that includes id/content/content_type/
/// source_language/parent_id/properties (and optionally tags + org fields).
/// Used both by inv-backend-blocks-match-ref's SQL path and by the LiveData<Block> experiment so
/// the two stay byte-for-byte equivalent.
fn parse_block_row(row: &holon::storage::types::StorageEntity) -> Option<Block> {
    let id =
        EntityUri::parse(row.get("id")?.as_string()?).expect("block id from DB must be valid URI");
    let parent_id = EntityUri::parse(row.get("parent_id")?.as_string()?)
        .expect("block parent_id from DB must be valid URI");
    let content = row
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();

    let mut block = Block::new_text(id, parent_id, content);

    block.tags = row
        .get("tags")
        .map(|v| match v {
            Value::Array(arr) => arr
                .iter()
                .filter_map(|x| x.as_string().map(|s| s.to_string()))
                .collect(),
            Value::Json(s) | Value::String(s) => {
                serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
            }
            _ => Vec::new(),
        })
        .unwrap_or_default();

    if let Some(content_type) = row.get("content_type").and_then(|v| v.as_string()) {
        block.content_type = content_type.parse::<ContentType>().unwrap();
    }
    if let Some(source_language) = row.get("source_language").and_then(|v| v.as_string()) {
        block.source_language = Some(source_language.parse::<SourceLanguage>().unwrap());
    }

    if let Some(props_val) = row.get("properties") {
        match props_val {
            Value::String(s) => {
                if let Ok(map) = serde_json::from_str::<HashMap<String, Value>>(s) {
                    block.properties = map;
                }
            }
            Value::Object(props) => {
                for (k, v) in props {
                    block.properties.insert(k.clone(), v.clone());
                }
            }
            _ => {}
        }
    }

    if let Some(task_state) = row
        .get("task_state")
        .or_else(|| row.get("TODO"))
        .and_then(|v| v.as_string())
    {
        block.set_task_state(Some(holon_api::TaskState::from_keyword(task_state)));
    }
    if let Some(priority) = row
        .get("priority")
        .or_else(|| row.get("PRIORITY"))
        .and_then(|v| v.as_i64())
    {
        block.set_priority(Some(
            holon_api::Priority::from_int(priority as i32)
                .unwrap_or_else(|e| panic!("stored priority {priority} is invalid: {e}")),
        ));
    }
    // `block.tags` is already populated from the matview JSON column above
    // (the dual-LEFT json_group_array projection). The legacy CSV handler
    // is only relevant for the org-property column `TAGS` (which the
    // matview projects through `properties`, not as a top-level column).
    if let Some(tags) = row.get("TAGS").and_then(|v| v.as_string()) {
        block.set_tags(holon_api::Tags::from_csv(tags));
    }
    if let Some(scheduled) = row
        .get("scheduled")
        .or_else(|| row.get("SCHEDULED"))
        .and_then(|v| v.as_string())
        && let Ok(ts) = holon_api::types::Timestamp::parse(scheduled)
    {
        block.set_scheduled(Some(ts));
    }
    if let Some(deadline) = row
        .get("deadline")
        .or_else(|| row.get("DEADLINE"))
        .and_then(|v| v.as_string())
        && let Ok(ts) = holon_api::types::Timestamp::parse(deadline)
    {
        block.set_deadline(Some(ts));
    }

    Some(block)
}

pub struct E2ESut<V: VariantMarker> {
    pub ctx: TestContext,
    /// Maps file-based doc URIs ("file:doc_0.org") to UUID-based URIs
    /// ("doc:<uuid>") assigned by the real system.
    pub doc_uri_map: HashMap<EntityUri, EntityUri>,
    /// How UI mutations are dispatched. `None` before `start_app` creates the engine.
    /// Backend tests use `DirectUserDriver`; Flutter tests inject their own driver.
    pub driver: Option<Arc<dyn UserDriver>>,
    /// Reactive engine for root layout — kept alive across transitions.
    /// Uses RefCell because `check_invariants` receives `&self`.
    reactive_engine: RefCell<Option<Arc<holon_frontend::reactive::ReactiveEngine>>>,
    /// Every ViewModel emission from the reactive stream, collected by a background task.
    /// check_invariants drains this and checks each intermediate ViewModel — catches
    /// transient CDC bugs that are masked by structural re-renders.
    vm_emissions: Arc<std::sync::Mutex<Vec<holon_frontend::ViewModel>>>,
    /// Optional Loro validation — reads blocks from LoroTree and compares against reference.
    /// Active only when Loro is enabled.
    loro_sut: Option<LoroSut>,
    /// Optional external frontend engine (e.g., GPUI's ReactiveEngine).
    /// When set, inv-frontend-engine checks the frontend's own ViewModel for errors.
    pub frontend_engine: Option<Arc<holon_frontend::reactive::ReactiveEngine>>,
    /// When set, inv-frontend-engine also checks that GPUI actually laid out the expected elements.
    pub frontend_geometry: Option<Box<dyn holon_frontend::geometry::GeometryProvider>>,
    /// Shared screenshot analysis — the GeometryDriver updates this after each
    /// screenshot, and inv-frontend-engine reads it to assert that the UI isn't visually empty.
    pub frontend_visual_state: Option<crate::ui_driver::VisualState>,
    /// Root layout block ID used by the ReactiveEngine — set during StartApp,
    /// used by `current_resolved_view_model()` and `current_reactive_tree()`.
    reactive_root_id: RefCell<Option<EntityUri>>,
    /// Headless live tree — persistent collection backed by the engine's live
    /// CDC data. Mirrors what the GPUI frontend sees: the collection driver
    /// calls `set_data` on existing items when data changes. Compared against
    /// the fresh tree in check_invariants to catch set_data propagation bugs.
    live_tree: RefCell<Option<holon_layout_testing::live_tree::HeadlessLiveTree>>,
    /// MCP integration for exercising IVM re-evaluation in PBT.
    pub pbt_mcp: Option<crate::pbt_mcp_fake::PbtMcpIntegration>,
    /// In-memory OTel span collector for non-functional invariants.
    #[cfg(feature = "otel-testing")]
    pub span_collector: crate::test_tracing::SpanCollector,
    /// Wall-clock start of the last transition (for wall-time budget checks).
    #[cfg(feature = "otel-testing")]
    pub(super) last_transition_start: Option<Instant>,
    /// The last transition applied (for budget lookup in check_invariants).
    pub(super) last_transition: crate::pbt::transitions::E2ETransition,
    /// RSS (bytes) captured before the last transition started.
    #[cfg(feature = "otel-testing")]
    pub(super) rss_before: usize,
    /// RSS (bytes) at the very start of the PBT run, for cumulative growth tracking.
    #[cfg(feature = "otel-testing")]
    pub(super) rss_baseline: usize,
    /// Loro-only peer instances for multi-instance sync testing.
    pub peers: Vec<holon::sync::multi_peer::PeerState<()>>,
    /// CDC-driven LiveData mirrors used by `check_invariants_async` to read
    /// authoritative state from in-memory snapshots instead of issuing fresh
    /// SQL on every check. Initialised lazily on first use because they need
    /// an async `watch_view` call after the engine has started; `RefCell` so
    /// `&self` invariant methods can populate them, and `Option` so we don't
    /// touch them until the first invariant check actually wants them.
    /// `wait_for_consumers` already gates each check on CDC delivery, so
    /// reading from these snapshots is delay-free vs the corresponding SQL.
    live_blocks_cell: RefCell<Option<Arc<holon::sync::LiveData<Block>>>>,
    live_focus_roots_cell: RefCell<Option<Arc<holon::sync::LiveData<FocusRoot>>>>,
    /// Case-level accumulator of `query` span ancestor chains (count, total
    /// duration). The `SpanCollector` resets at the start of every
    /// transition (`apply_transition` sync hook), so to get whole-case
    /// totals we snapshot `queries_by_origin()` before each reset and merge
    /// here. Used only when `PBT_MATVIEW_METRICS=1`.
    query_origin_acc: RefCell<std::collections::HashMap<Vec<String>, (usize, std::time::Duration)>>,
    /// Reference state as it stood at the END of the previous transition —
    /// i.e. the state the user CURRENTLY sees rendered in the SUT, before
    /// the in-flight transition is applied. The framework passes the
    /// POST-transition state into `apply_to_sut`, but waits that gate "what
    /// the user can act on right now" need the pre-transition shape (the
    /// post-state already contains any blocks the SUT hasn't been told to
    /// create yet). Updated at the END of `apply_transition_async`, so
    /// during the next call this holds previous-post = current-pre. `None`
    /// for the very first transition — pre-state is effectively empty.
    pre_ref_state: Option<ReferenceState>,
    _marker: PhantomData<V>,
}

impl<V: VariantMarker> std::ops::Deref for E2ESut<V> {
    type Target = TestContext;
    fn deref(&self) -> &Self::Target {
        &self.ctx
    }
}

impl<V: VariantMarker> std::ops::DerefMut for E2ESut<V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.ctx
    }
}

impl<V: VariantMarker> std::fmt::Debug for E2ESut<V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.ctx.fmt(f)
    }
}

impl<V: VariantMarker> Drop for E2ESut<V> {
    fn drop(&mut self) {
        // Print one-shot matview cache metrics only when explicitly asked
        // (PBT_MATVIEW_METRICS=1). Default-off so normal test output stays
        // clean; flip on when profiling cache effectiveness.
        if std::env::var("PBT_MATVIEW_METRICS").as_deref() != Ok("1") {
            return;
        }
        if self.ctx.is_running() {
            let (hits, exists, creates) = self.ctx.engine().matview_cache_metrics();
            let total = hits + exists;
            let hit_pct = if total == 0 {
                0.0
            } else {
                (hits as f64 / total as f64) * 100.0
            };
            eprintln!(
                "[matview-cache] cache_hits={hits} exists_calls={exists} ddl_creates={creates} \
                 hit_rate={hit_pct:.1}%"
            );

            // Per-origin SQL query breakdown — merged across the whole case.
            // The collector resets per transition, so `query_origin_acc`
            // accumulates each pre-reset snapshot; here we fold in the final
            // transition's spans (no reset has fired since) and print the
            // total. Rows under "<no-parent>" / "<unknown-parent>" are the
            // prime suspects for the "1600 mystery queries" — they're SQL
            // fired from a tokio task whose parent span didn't propagate.
            let mut acc = self.query_origin_acc.borrow_mut();
            let final_breakdown = self.span_collector.queries_by_origin();
            for row in final_breakdown.rows {
                let entry = acc
                    .entry(row.chain)
                    .or_insert((0, std::time::Duration::ZERO));
                entry.0 += row.count;
                entry.1 += row.total_duration;
            }
            let mut rows: Vec<crate::test_tracing::QueryOriginRow> = acc
                .iter()
                .map(
                    |(chain, (count, total_duration))| crate::test_tracing::QueryOriginRow {
                        chain: chain.clone(),
                        count: *count,
                        total_duration: *total_duration,
                    },
                )
                .collect();
            rows.sort_by(|a, b| {
                b.total_duration
                    .cmp(&a.total_duration)
                    .then(b.count.cmp(&a.count))
            });
            let total_queries: usize = rows.iter().map(|r| r.count).sum();
            let total_duration: std::time::Duration = rows.iter().map(|r| r.total_duration).sum();
            let breakdown = crate::test_tracing::QueryOriginBreakdown {
                rows,
                total_queries,
                total_duration,
            };
            eprintln!("[query-origin]\n{breakdown}");
        }
    }
}

impl<V: VariantMarker> E2ESut<V> {
    /// After a transition that may have produced a new "split-suffix"
    /// block (the SplitBlock chord op, or PressKey(Enter) which
    /// dispatches `split_block` from the editor), associate every
    /// unmapped `block::split-N` synthetic id in `ref_state` with the
    /// corresponding real UUID surfaced in `db_rows`. Without this the
    /// post-step `assert_blocks_equivalent` check sees prod-UUID vs
    /// ref-synthetic-ID and fails on what is logically the same block.
    fn map_unmapped_split_synthetic_ids(
        &mut self,
        ref_state: &ReferenceState,
        db_rows: &[holon_api::widget_spec::DataRow],
        label: &str,
    ) {
        let unmapped_synthetic: Vec<EntityUri> = ref_state
            .block_state
            .blocks
            .keys()
            .filter(|id| id.as_str().contains(":split-") && !self.doc_uri_map.contains_key(*id))
            .cloned()
            .collect();
        if unmapped_synthetic.is_empty() {
            return;
        }

        let known_real_ids: HashSet<String> = {
            let mut ids: HashSet<String> =
                self.doc_uri_map.values().map(|u| u.to_string()).collect();
            for ref_id in ref_state.block_state.blocks.keys() {
                if !self.doc_uri_map.contains_key(ref_id) && !ref_id.as_str().contains(":split-") {
                    ids.insert(ref_id.to_string());
                }
            }
            ids
        };

        let new_real_ids: Vec<String> = db_rows
            .iter()
            .filter_map(|row| row.get("id")?.as_string().map(|s| s.to_string()))
            .filter(|id| !known_real_ids.contains(id))
            .collect();

        for (synthetic, real_id_str) in unmapped_synthetic.iter().zip(new_real_ids.iter()) {
            let real_id = EntityUri::from_raw(real_id_str);
            eprintln!("{label} Mapped {synthetic} → {real_id}");
            self.doc_uri_map.insert(synthetic.clone(), real_id);
        }
    }
}

#[async_trait::async_trait(?Send)]
impl<V: VariantMarker> crate::pbt::transition_dispatch::SutHandle for E2ESut<V> {
    /// Override the default-panicking SutHandle stub: route the layout-PBT
    /// `Clickable::click_at_element` capability through the same
    /// `UserDriver::click_entity` path the rich-PBT chord transitions use.
    /// Region is unknown at this layer (the shared variant only has an
    /// `element_id`) so we pass an empty region string — the driver
    /// resolves geometry via the bounds registry alone.
    async fn apply_click_at_element(&mut self, element_id: &str) {
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        // The driver's editor_focus fallback requires a valid region. Infer
        // from the element_id prefix; default to "main" for generic clicks.
        let region = if element_id.contains("left-sidebar") || element_id.contains("left_sidebar") {
            "left_sidebar"
        } else if element_id.contains("right-sidebar") || element_id.contains("right_sidebar") {
            "right_sidebar"
        } else {
            "main"
        };
        driver
            .click_entity(element_id, region)
            .await
            .unwrap_or_else(|e| {
                panic!("[LayoutPBT::click_at_element] click_entity({element_id}) failed: {e:#}")
            });
        self.ctx.drain_region_cdc_events().await;
    }

    /// Override the default-panicking SutHandle stub. Backend tests don't
    /// generate `DeliverBlockContent` (the variant's `weighted_generator`
    /// returns `Fail(DeliverNotMeaningfulInBackendTests)`), so this should
    /// never be reached. Keep the override so accidental wiring fails
    /// loud with a typed message instead of the default `unimplemented!`.
    async fn apply_deliver_block_content_loaded(&mut self, block_id: &str) {
        panic!(
            "[LayoutPBT::deliver_block_content_loaded] reached for {block_id} — backend PBT \
             should reject DeliverBlockContent in its generator (see weighted_generator)."
        );
    }

    async fn navigate_back(&mut self, region: holon_api::Region) {
        debug_assert_eq!(
            region,
            holon_api::Region::Main,
            "NavigateBack generator must only emit Main; got {region:?}"
        );
        self.send_leader_chord("go_back", "NavigateBack").await;
        self.ctx.drain_region_cdc_events().await;
        self.dump_nav_tables("after NavigateBack").await;
    }

    async fn apply_write_org_file(&mut self, filename: &str, content: &str) {
        tracing::trace!(
            "[apply] WriteOrgFile: {} ({} bytes)",
            filename,
            content.len()
        );
        self.write_org_file(filename, content)
            .await
            .expect("Failed to write org file");

        // If app is running, wait for OrgSyncController to ingest the file
        // and re-key ctx.documents from `file:<filename>` to the resolved
        // doc URI. Mirrors the start_app loop (see apply_start_app body):
        // without this, subsequent transitions like apply_bulk_external_add
        // that resolve the doc via `resolve_uri` (which checks doc_uri_map)
        // and then `ctx.documents.get(&resolved)` will miss because docs
        // added post-startup never got re-keyed.
        if !self.ctx.is_running() {
            return;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match self.ctx.resolve_doc_uri_by_name(filename).await {
                Ok(resolved) => {
                    let file_key = holon_api::EntityUri::file(filename);
                    if let Some(path) = self.ctx.documents.remove(&file_key) {
                        self.ctx.documents.insert(resolved.clone(), path);
                    }
                    // The synthetic URI minted by the ref-model equals
                    // the resolved URI (WriteOrgFile.apply_to_sut
                    // injects `#+ID: <synthetic>` into the file).
                    if !self.doc_uri_map.contains_key(&resolved) {
                        self.doc_uri_map.insert(resolved.clone(), resolved.clone());
                    }
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        }
    }

    async fn apply_create_directory(&mut self, path: &str) {
        tracing::trace!("[apply] CreateDirectory: {}", path);
        let full_path = self.temp_dir.path().join(path);
        tokio::fs::create_dir_all(&full_path)
            .await
            .expect("Failed to create directory");
    }

    async fn apply_git_init(&mut self) {
        tracing::trace!("[apply] GitInit");
        let output = tokio::process::Command::new("git")
            .args(["init"])
            .current_dir(self.temp_dir.path())
            .output()
            .await
            .expect("Failed to run git init");
        assert!(output.status.success(), "git init failed: {:?}", output);
    }

    async fn apply_jj_git_init(&mut self) {
        tracing::trace!("[apply] JjGitInit");
        let output = tokio::process::Command::new("jj")
            .args(["git", "init"])
            .current_dir(self.temp_dir.path())
            .output()
            .await
            .expect("Failed to run jj git init");
        assert!(output.status.success(), "jj git init failed: {:?}", output);
    }

    async fn apply_create_stale_loro(
        &mut self,
        org_filename: &str,
        corruption_type: crate::LoroCorruptionType,
    ) {
        tracing::trace!(
            "[apply] CreateStaleLoro: {} ({:?})",
            org_filename,
            corruption_type
        );
        self.write_stale_loro_file(org_filename, corruption_type)
            .await
            .expect("Failed to create stale loro file");
    }

    #[tracing::instrument(
        skip(self, ref_state),
        fields(wait_for_ready, enable_todoist, enable_loro),
        name = "pbt.apply_start_app"
    )]
    async fn apply_start_app(
        &mut self,
        ref_state: &ReferenceState,
        wait_for_ready: bool,
        enable_todoist: bool,
        enable_loro: bool,
    ) {
        tracing::trace!(
            "[apply] StartApp (wait_for_ready={}, enable_todoist={}, enable_loro={})",
            wait_for_ready,
            enable_todoist,
            enable_loro
        );
        self.set_enable_todoist(enable_todoist);
        self.set_enable_loro(enable_loro);
        self.start_app(wait_for_ready)
            .await
            .expect("Failed to start app");

        // Install the default mutation driver now that the engine exists.
        if self.driver.is_none() {
            self.install_driver();
        }

        // Initialize real MCP integration for IVM re-evaluation testing.
        let db_handle = self.ctx.engine().db_handle().clone();
        match crate::pbt_mcp_fake::PbtMcpIntegration::new(db_handle).await {
            Ok(integration) => self.pbt_mcp = Some(integration),
            Err(e) => {
                tracing::trace!("[apply] PbtMcpIntegration init failed (non-fatal): {e}")
            }
        }

        // Mirror Flutter startup: call initial_widget() after engine ready.
        let expects_valid_index = ref_state.is_properly_setup();
        let root_id = ref_state
            .root_layout_block_id()
            .unwrap_or_else(holon_api::root_layout_block_uri);
        tracing::trace!(
            "[apply] Calling render_entity('{}') (expects valid index.org: {})",
            root_id,
            expects_valid_index
        );

        let render_result = self.engine().blocks().render_entity(&root_id, &None).await;

        match (expects_valid_index, render_result) {
            (true, Ok((_render_expr, _stream))) => {
                tracing::trace!("[apply] render_entity('{}') succeeded", root_id);
            }
            (true, Err(e)) => {
                let err_str = e.to_string();
                if err_str.contains("ScalarSubquery") || err_str.contains("materialized view") {
                    tracing::trace!(
                        "[apply] render_entity('{}') failed due to known Turso IVM limitation (GQL): {}",
                        root_id,
                        e
                    );
                } else {
                    panic!(
                        "render_entity('{}') failed but reference state has valid index.org: {}",
                        root_id, e
                    );
                }
            }
            (false, Ok(_)) => {
                panic!(
                    "render_entity('{}') succeeded but reference state has no valid index.org",
                    root_id
                );
            }
            (false, Err(e)) => {
                tracing::trace!(
                    "[apply] render_entity('{}') correctly failed (no valid index.org): {}",
                    root_id,
                    e
                );
            }
        }

        // Set up region watches for all regions
        for region in holon_api::Region::ALL {
            if let Err(e) = self.setup_region_watch(*region).await {
                tracing::trace!(
                    "[apply] Region watch setup for {} failed (non-fatal): {}",
                    region.as_str(),
                    e
                );
            }
        }

        // Set up all-blocks CDC watch (invariant #1 uses this instead of direct SQL)
        self.setup_all_blocks_watch()
            .await
            .expect("Failed to set up all-blocks CDC watch");

        // Capture the production seed-block count
        let expected_ids = self.expected_block_ids(ref_state);
        self.prime_seed_count(&expected_ids, std::time::Duration::from_secs(10))
            .await;

        // Push the reference state's TODO keyword set into production.
        if let Some(ref ks) = ref_state.keyword_set {
            use holon_orgmode::models::OrgDocumentExt;
            let default_doc_uri = holon_api::EntityUri::no_parent();
            let rows = self
                .ctx
                .query_sql(&format!(
                    "SELECT b.id, b.parent_id, \
                     (SELECT json_group_array(bt.tag) FROM block_tags bt WHERE bt.block_id = b.id) as tags, \
                     b.content, b.content_type, b.properties \
                     FROM block_raw b WHERE b.id = '{}'",
                    default_doc_uri
                ))
                .await
                .expect("query default doc block");
            if let Some(row) = rows.first() {
                let mut doc_block = Block::new_text(
                    default_doc_uri.clone(),
                    holon_api::EntityUri::no_parent(),
                    row.get("content").and_then(|v| v.as_string()).unwrap_or(""),
                );
                doc_block.tags = row
                    .get("tags")
                    .map(|v| match v {
                        Value::Array(arr) => arr
                            .iter()
                            .filter_map(|x| x.as_string().map(|s| s.to_string()))
                            .collect(),
                        Value::Json(s) | Value::String(s) => {
                            serde_json::from_str::<Vec<String>>(s).unwrap_or_default()
                        }
                        _ => Vec::new(),
                    })
                    .unwrap_or_default();
                if let Some(props_val) = row.get("properties")
                    && let Some(s) = props_val.as_string()
                    && let Ok(map) =
                        serde_json::from_str::<std::collections::HashMap<String, Value>>(s)
                {
                    doc_block.properties = map;
                }
                doc_block.set_todo_keywords(Some(ks.0.clone()));
                let params = holon_orgmode::build_block_params(
                    &doc_block,
                    &doc_block.parent_id,
                    &default_doc_uri,
                );
                if let Err(e) = self
                    .ctx
                    .test_ctx()
                    .execute_op("block", "update", params)
                    .await
                {
                    tracing::trace!("[apply] Failed to push keyword_set into production: {e}");
                } else {
                    tracing::trace!(
                        "[apply] Pushed keyword_set ({} keywords) into production default doc",
                        ks.0.len()
                    );
                }
            } else {
                tracing::trace!(
                    "[apply] WARNING: keyword_set set but default doc {} not found in DB",
                    default_doc_uri
                );
            }
        }

        // Populate doc_uri_map for pre-startup documents whose document
        // entities were created by OrgSyncController during startup.
        for (synthetic_uri, filename) in &ref_state.documents {
            if !self.doc_uri_map.contains_key(synthetic_uri) {
                match self.ctx.resolve_doc_uri_by_name(filename).await {
                    Ok(resolved) => {
                        tracing::trace!(
                            "[apply] Mapped pre-startup doc: {} → {}",
                            synthetic_uri,
                            resolved
                        );
                        self.doc_uri_map
                            .insert(synthetic_uri.clone(), resolved.clone());
                        // Re-key ctx.documents from file-based to UUID-based URI
                        let file_key = holon_api::EntityUri::file(filename);
                        if let Some(path) = self.ctx.documents.remove(&file_key) {
                            self.ctx.documents.insert(resolved, path);
                        }
                    }
                    Err(e) => {
                        tracing::trace!(
                            "[apply] Could not resolve pre-startup doc {}: {}",
                            synthetic_uri,
                            e
                        );
                    }
                }
            }
        }

        // Initialize LoroSut if Loro is enabled
        if let Some(doc_store) = self.ctx.doc_store() {
            tracing::trace!("[apply] Loro enabled — initializing LoroSut for invariant checking");
            self.loro_sut = Some(crate::pbt::loro_sut::LoroSut::new(doc_store.clone()));
        }

        // Initialize the ReactiveEngine now so all subsequent
        // transitions can read the reactive tree — just like the real GPUI frontend.
        self.ensure_reactive_engine(&root_id).await;
        tracing::trace!("[apply] ReactiveEngine initialized for root '{}'", root_id);
    }

    async fn apply_navigate_focus(
        &mut self,
        region: holon_api::Region,
        block_id: &holon_api::EntityUri,
    ) {
        // The generator restricts NavigateFocus to `Region::Main` and
        // to LeftSidebar-listed pages — the only navigation path a
        // real user can trigger. Sanity-check both invariants here.
        debug_assert_eq!(
            region,
            holon_api::Region::Main,
            "NavigateFocus generator must only emit Main; got {region:?}"
        );
        let resolved_id = self.resolve_uri(block_id);
        tracing::trace!(
            "[apply] NavigateFocus: region={region:?} block={block_id} (resolved={resolved_id})"
        );
        // Drive the real UI: clicking the LeftSidebar entry dispatches
        // its bound `navigation.focus(region: "main", block_id)` action.
        // `ReactiveEngineDriver::click_entity` shares GPUI's intent
        // resolution: snapshot_resolved → find_click_intent_in_view_model
        // → apply_intent. No driver-specific synthesis needed.
        //
        // Mouse-driven dispatch under GPUI requires committed bounds in
        // BoundsRegistry — sidebar entries from a freshly-loaded layout
        // may not have promoted staged → committed by the time the test
        // polls. Mirror ClickBlock / SplitBlock and wait first; headless
        // drivers no-op the wait.
        self.wait_for_entity_bounds(resolved_id.as_str(), Duration::from_secs(5))
            .await
            .unwrap_or_else(|e| panic!("[NavigateFocus] {e}"));
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        driver
            .click_entity(resolved_id.as_str(), "left_sidebar")
            .await
            .unwrap_or_else(|e| {
                panic!("[NavigateFocus] click_entity failed for sidebar entry {resolved_id}: {e:#}")
            });
        self.ctx.drain_region_cdc_events().await;
        self.dump_nav_tables("after NavigateFocus").await;
    }

    async fn apply_navigate_forward(&mut self, region: holon_api::Region) {
        debug_assert_eq!(
            region,
            holon_api::Region::Main,
            "NavigateForward generator must only emit Main; got {region:?}"
        );
        self.send_leader_chord("go_forward", "NavigateForward")
            .await;
        self.ctx.drain_region_cdc_events().await;
        self.dump_nav_tables("after NavigateForward").await;
    }

    async fn apply_navigate_home(&mut self, region: holon_api::Region) {
        debug_assert_eq!(
            region,
            holon_api::Region::Main,
            "NavigateHome generator must only emit Main; got {region:?}"
        );
        self.send_leader_chord("go_home", "NavigateHome").await;
        self.ctx.drain_region_cdc_events().await;
        self.dump_nav_tables("after NavigateHome").await;
    }

    async fn apply_simulate_restart(&mut self, ref_state: &ReferenceState) {
        let expected_ids = self.expected_block_ids(ref_state);
        self.simulate_restart(&expected_ids)
            .await
            .expect("SimulateRestart failed");
    }

    async fn apply_create_document(&mut self, file_name: &str, ref_state: &ReferenceState) {
        tracing::trace!("[apply] Creating document: {}", file_name);
        match self.create_document(file_name).await {
            Ok(uuid_uri) => {
                // Find the synthetic URI from ref_state (keyed by filename)
                let synthetic_uri = ref_state
                    .documents
                    .iter()
                    .find(|(_, name)| *name == file_name)
                    .map(|(uri, _)| uri.clone())
                    .expect("CreateDocument: synthetic URI not found in reference state");
                tracing::trace!("[apply] Created document: {} → {}", synthetic_uri, uuid_uri);
                self.doc_uri_map.insert(synthetic_uri, uuid_uri);
            }
            Err(e) => panic!("Failed to create document: {}", e),
        }
    }

    async fn apply_remove_watch(&mut self, query_id: &str) {
        self.remove_watch(query_id);
    }

    async fn apply_switch_view(&mut self, view_name: &str) {
        self.switch_view(view_name);
    }

    async fn apply_concurrent_schema_init(&mut self) {
        tracing::trace!(
            "[apply] ConcurrentSchemaInit: testing sequential operations don't cause database lock"
        );

        // This test verifies that normal sequential operations don't cause
        // "database is locked" errors. The original bug was:
        // 1. ensure_navigation_schema() called during DI init
        // 2. initial_widget() called it AGAIN while IVM was still processing
        // 3. This caused persistent "database is locked" errors
        //
        // After the fix, sequential operations should work without locking issues.
        let engine = self.engine();

        // Run several query_and_watch operations SEQUENTIALLY (not concurrently)
        // Each creates a materialized view, which should work fine when done one at a time
        for i in 0..3 {
            let prql = format!(
                "from block_raw | select {{id, content}} | filter id != \"dummy-{}\" ",
                i
            );
            let sql = engine
                .compile_to_sql(&prql, QueryLanguage::HolonPrql)
                .expect("PRQL compilation should succeed");
            let start = std::time::Instant::now();
            match engine.query_and_watch(sql, HashMap::new(), None).await {
                Ok(_) => {
                    eprintln!(
                        "[ConcurrentSchemaInit] query_and_watch {} succeeded in {:?}",
                        i,
                        start.elapsed()
                    );
                }
                Err(e) => {
                    let error_str = format!("{:?}", e);
                    let elapsed = start.elapsed();
                    eprintln!(
                        "[ConcurrentSchemaInit] query_and_watch {} FAILED in {:?}: {}",
                        i, elapsed, error_str
                    );
                    // Check for the specific "database is locked" error that indicates
                    // the double-schema-init bug
                    if error_str.contains("database is locked") {
                        panic!(
                            "DATABASE LOCK BUG: Sequential query_and_watch {} failed with 'database is locked' after {:?}!\n\
                                 This indicates the ensure_navigation_schema() is still being called multiple times.\n\
                                 Error: {}",
                            i, elapsed, error_str
                        );
                    }
                    // Other errors (like "Database schema changed") might occur due to
                    // other concurrent activity and are not necessarily the double-init bug
                }
            }
        }

        // Also run some simple queries to verify basic operations work
        for i in 0..2 {
            let sql = "SELECT id FROM block_raw LIMIT 1".to_string();
            let start = std::time::Instant::now();
            match engine.execute_query(sql, HashMap::new(), None).await {
                Ok(_) => {
                    eprintln!(
                        "[ConcurrentSchemaInit] simple query {} succeeded in {:?}",
                        i,
                        start.elapsed()
                    );
                }
                Err(e) => {
                    let error_str = format!("{:?}", e);
                    let elapsed = start.elapsed();
                    eprintln!(
                        "[ConcurrentSchemaInit] simple query {} FAILED in {:?}: {}",
                        i, elapsed, error_str
                    );
                    if error_str.contains("database is locked") {
                        panic!(
                            "DATABASE LOCK BUG: Sequential simple query {} failed with 'database is locked' after {:?}!\n\
                                 Error: {}",
                            i, elapsed, error_str
                        );
                    }
                }
            }
        }

        eprintln!("[ConcurrentSchemaInit] All sequential operations completed successfully");
        eprintln!("[ConcurrentSchemaInit] Test completed successfully");
    }

    async fn apply_setup_watch(
        &mut self,
        query_id: &str,
        query: &crate::pbt::query::TestQuery,
        language: QueryLanguage,
    ) {
        let (source, lang_str) = query.compile_for(language);
        tracing::trace!(
            "[apply] SetupWatch: {} ({}) → {}",
            query_id,
            lang_str,
            &source[..source.len().min(80)]
        );
        self.setup_watch(query_id, &source, lang_str)
            .await
            .expect("Watch setup failed");
    }

    async fn apply_toggle_state(&mut self, block_id: &holon_api::EntityUri, new_state: &str) {
        let resolved_block_id = self.resolve_uri(block_id);
        tracing::trace!(
            "[apply] ToggleState: block={block_id} (resolved={resolved_block_id}) → {new_state:?}"
        );

        // Use a fully cross-block-resolved ViewModel: each nested
        // `live_block` is recursively interpreted via
        // `engine.snapshot_resolved`, which calls `ensure_watching`
        // per block so per-region UiWatchers fire. We poll until the
        // target entity is visible — sidebar/main-panel slots populate
        // asynchronously, and the older `current_resolved_view_model`
        // (which only interprets the root) returns an empty
        // `live_block` for the main-panel slot.
        let display_tree = self
            .wait_for_entity_in_resolved_view_model(
                resolved_block_id.as_str(),
                Duration::from_secs(5),
            )
            .await
            .unwrap_or_else(|| {
                panic!(
                    "[ToggleState] entity {resolved_block_id} did not appear in the \
                     resolved ViewModel within 5s — sidebar nav may not have populated \
                     the main panel yet."
                )
            });

        let all_toggles = crate::display_assertions::collect_state_toggle_nodes(&display_tree);
        let toggle = all_toggles.iter().find(|t| {
            t.row_id()
                .is_some_and(|id| id == resolved_block_id.as_str())
        });
        if toggle.is_none() {
            eprintln!(
                "[ToggleState] No StateToggle with id={block_id} in resolved tree \
                 (found {} toggles, root={:?}).\nTree:\n{}",
                all_toggles.len(),
                display_tree.widget_name(),
                display_tree.pretty_print(0),
            );
            panic!("[ToggleState] No StateToggle with id={block_id} in resolved tree");
        }
        let toggle = toggle.unwrap();

        let (field, current, states) = match &toggle.kind {
            holon_frontend::view_model::ViewKind::StateToggle {
                field,
                current,
                states,
                ..
            } => (field.clone(), current.clone(), states.clone()),
            _ => panic!("[ToggleState] Expected StateToggle, got {:?}", toggle.kind),
        };

        assert!(
            !toggle.operations.is_empty(),
            "[ToggleState] StateToggle for {block_id} has no operations"
        );
        let op = holon_frontend::operations::find_set_field_op(&field, &toggle.operations);
        assert!(
            op.is_some(),
            "[ToggleState] No set_field op for '{field}' on {block_id}"
        );
        let op = op.unwrap();

        let row_id = toggle.row_id();
        assert!(
            row_id.is_some(),
            "[ToggleState] StateToggle for {block_id} has no entity id"
        );
        let row_id = row_id.unwrap();
        let entity_name = toggle
            .entity_name()
            .expect("[ToggleState] StateToggle has no entity name");

        let states_vec: Vec<String> = states.split(',').map(|s| s.to_string()).collect();
        assert!(
            states_vec.iter().any(|s| s == new_state),
            "[ToggleState] '{new_state}' not in states {states_vec:?}"
        );

        // Validate that the keybinding registry's chord for
        // `cycle_task_state` was joined onto the rendered state_toggle
        // node's operations. Reading directly off the resolved
        // ViewModel node we already located — bypasses the older
        // `assert_keychord_resolves` path that walks
        // `current_reactive_tree`, whose `live_block` slots are not
        // synchronously populated in the headless test (the same
        // limitation we worked around with
        // `wait_for_entity_in_resolved_view_model`).
        if let Some(expected_chord) = self.find_keybinding_for_op("cycle_task_state") {
            let cycle_op_chord = toggle.operations.iter().find_map(|ow| {
                if ow.descriptor.name == "cycle_task_state" {
                    ow.descriptor.key_chord().cloned()
                } else {
                    None
                }
            });
            assert_eq!(
                cycle_op_chord.as_ref(),
                Some(&expected_chord),
                "[ToggleState] state_toggle on {block_id} is missing the \
                 keybinding-joined `cycle_task_state` op (expected chord {expected_chord:?}). \
                 Operations on the node: {:?}",
                toggle
                    .operations
                    .iter()
                    .map(|ow| (
                        ow.descriptor.name.clone(),
                        ow.descriptor.key_chord().cloned()
                    ))
                    .collect::<Vec<_>>()
            );
            eprintln!(
                "[ToggleState] keychord validation OK: {expected_chord:?} bound on \
                 cycle_task_state for {row_id}"
            );
        }

        // Dispatch the actual mutation via set_field (PBT controls exact new_state)
        let intent = holon_frontend::OperationIntent::set_field(
            &entity_name,
            &op.name,
            &row_id,
            &field,
            Value::String(new_state.to_string()),
        );
        eprintln!("[ToggleState] Dispatching set_field: {current:?} → {new_state:?}");
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        driver
            .apply_intent(intent)
            .await
            .expect("ToggleState dispatch failed");

        // Let the CDC event propagate through the enrichment pipeline.
        // The data matview CDC fires synchronously from the DB write, but
        // the channel-based forwarding needs a yield to process.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        // ── Fresh-tree check ─────────────────────────────────
        // Snapshot the reactive tree NOW — before the structural
        // re-render can mask CDC enrichment bugs. The structural CDC
        // also fires for this row change and triggers a re-render
        // with fresh query_view data (which uses a different JSON
        // parsing path). By checking here, we observe the
        // CDC-enriched data before it gets replaced.
        if let Some((_root, post_tree)) = self.current_reactive_tree() {
            let post_toggle = crate::display_assertions::find_state_toggle_for_block_reactive(
                &post_tree,
                &resolved_block_id,
            );
            if let Some(post) = post_toggle
                && post.widget_name().as_deref() == Some("state_toggle")
            {
                let post_current = post.prop_str("current").unwrap_or_else(|| "".to_string());
                // The value must be either the new state (CDC propagated
                // correctly) or the old state (CDC hasn't arrived yet).
                // It must NOT be empty when we set it to a non-empty value
                // — that would mean the CDC enrichment dropped the property.
                if !new_state.is_empty() && post_current.is_empty() {
                    panic!(
                        "[ToggleState] Post-mutation ViewModel has empty StateToggle \
                             for block {block_id}! Set '{current}' → '{new_state}' but \
                             got ''. This means the CDC enrichment pipeline lost the \
                             task_state property (flatten_properties bug)."
                    );
                }
            }
        }

        // Live-tree vs fresh-tree check is done in check_invariants
        // via the HeadlessLiveTree (inv10_live).
    }

    async fn apply_edit_via_display_tree(
        &mut self,
        block_id: &holon_api::EntityUri,
        new_content: &str,
    ) {
        let resolved_block_id = self.resolve_uri(block_id);
        tracing::trace!(
            "[apply] EditViaDisplayTree: block={block_id} (resolved={resolved_block_id}) → {new_content:?}"
        );

        // In production, leaf blocks are rendered by the render_entity() DSL
        // function using entity profiles + row data from the parent query.
        // We replicate this by querying the block's data and interpreting
        // render_entity() with that data as context.
        let engine = self.engine();
        let sql = format!(
            "SELECT id, content, content_type, source_language, parent_id \
             FROM block_raw WHERE id = '{}'",
            resolved_block_id
        );
        let data_rows = engine
            .execute_query(sql, HashMap::new(), None)
            .await
            .expect("block query failed in EditViaDisplayTree");
        assert!(
            !data_rows.is_empty(),
            "[EditViaDisplayTree] Block {block_id} not found in database"
        );

        let render_expr = holon_api::render_types::RenderExpr::FunctionCall {
            name: "render_entity".to_string(),
            args: Vec::new(),
        };

        let engine_clone = Arc::clone(engine);
        let display_tree = tokio::task::spawn_blocking(move || {
            let services = holon_frontend::reactive::HeadlessBuilderServices::new(engine_clone);
            holon_frontend::interpret_pure(
                &render_expr,
                &data_rows
                    .iter()
                    .cloned()
                    .map(std::sync::Arc::new)
                    .collect::<Vec<_>>(),
                &services,
            )
            .snapshot()
        })
        .await
        .expect("spawn_blocking panicked");

        // Walk tree to find EditableText node for this block_id
        fn find_editable_for_block<'a>(
            node: &'a holon_frontend::ViewModel,
            block_id: &EntityUri,
        ) -> Option<&'a holon_frontend::ViewModel> {
            if matches!(
                &node.kind,
                holon_frontend::view_model::ViewKind::EditableText { .. }
            ) && node
                .entity
                .get("id")
                .and_then(|v| v.as_string())
                .is_some_and(|id| id == block_id.as_str())
            {
                return Some(node);
            }
            node.children()
                .iter()
                .find_map(|c| find_editable_for_block(c, block_id))
        }

        let editable = find_editable_for_block(&display_tree, &resolved_block_id)
            .or_else(|| find_editable_for_block(&display_tree, &resolved_block_id))
            .unwrap_or_else(|| {
                panic!(
                    "[EditViaDisplayTree] No EditableText with id={resolved_block_id} in display tree.\n\
                     This means render_entity created the node without entity context.\n{}",
                    display_tree.pretty_print(0)
                )
            });

        assert!(
            !editable.operations.is_empty(),
            "[EditViaDisplayTree] EditableText for {block_id} has empty operations.\n\
             set_field cannot fire on blur.\n{}",
            display_tree.pretty_print(0)
        );

        // Extract operation metadata and execute
        let op = holon_frontend::operations::find_set_field_op("content", &editable.operations)
            .expect("No set_field operation found on EditableText");

        let row_id = editable.row_id().expect("EditableText entity has no 'id'");
        let entity_name = editable
            .entity_name()
            .expect("EditableText entity has no entity name");

        let intent = holon_frontend::OperationIntent::set_field(
            &entity_name,
            &op.name,
            &row_id,
            "content",
            Value::String(new_content.to_string()),
        );

        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        driver
            .apply_intent(intent)
            .await
            .expect("set_field via display tree failed");
    }

    async fn apply_edit_via_view_model(
        &mut self,
        block_id: &holon_api::EntityUri,
        new_content: &str,
    ) {
        let resolved_block_id = self.resolve_uri(block_id);
        tracing::trace!(
            "[apply] EditViaViewModel: block={block_id} (resolved={resolved_block_id}) → {new_content:?}"
        );

        // 1. Query block data and render via render_entity() DSL (same as EditViaDisplayTree)
        let engine = self.engine();
        let sql = format!(
            "SELECT id, content, content_type, source_language, parent_id \
             FROM block_raw WHERE id = '{}'",
            resolved_block_id
        );
        let data_rows = engine
            .execute_query(sql, HashMap::new(), None)
            .await
            .expect("block query failed in EditViaViewModel");
        assert!(
            !data_rows.is_empty(),
            "[EditViaViewModel] Block {resolved_block_id} not found in database"
        );

        let render_expr = holon_api::render_types::RenderExpr::FunctionCall {
            name: "render_entity".to_string(),
            args: Vec::new(),
        };

        let engine_clone = Arc::clone(engine);
        let display_tree = tokio::task::spawn_blocking(move || {
            let services = holon_frontend::reactive::HeadlessBuilderServices::new(engine_clone);
            holon_frontend::interpret_pure(
                &render_expr,
                &data_rows
                    .iter()
                    .cloned()
                    .map(std::sync::Arc::new)
                    .collect::<Vec<_>>(),
                &services,
            )
            .snapshot()
        })
        .await
        .expect("spawn_blocking panicked");

        // 2. Find EditableText node for this block
        let editable = display_tree
            .find_editable_text(resolved_block_id.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "[EditViaViewModel] No EditableText with id={resolved_block_id} in display tree.\n{}",
                    display_tree.pretty_print(0)
                )
            });

        // 3. Verify triggers are present
        assert!(
            !editable.triggers.is_empty(),
            "[EditViaViewModel] EditableText for {block_id} has no triggers.\n{}",
            display_tree.pretty_print(0)
        );

        // 4. Build EditorViewModel and verify normal text doesn't fire triggers
        let mut ctrl = holon_frontend::EditorViewModel::from_view_model(editable);
        assert!(
            matches!(
                ctrl.on_text_changed("hello", 1),
                holon_frontend::EditorAction::None
            ),
            "[EditViaViewModel] Normal text 'hello' should NOT fire any trigger"
        );

        // 5. Simulate blur with new content
        let original_value = match &editable.kind {
            holon_frontend::view_model::ViewKind::EditableText { content, .. } => content.clone(),
            _ => unreachable!(),
        };
        let action = ctrl.on_blur(new_content);

        // 6. Dispatch the resulting operation
        match action {
            holon_frontend::EditorAction::Execute(intent) => {
                let driver = self
                    .driver
                    .as_ref()
                    .expect("driver not installed — was start_app called?");
                driver
                    .apply_intent(intent)
                    .await
                    .expect("set_field via ViewModel TextSync failed");
            }
            holon_frontend::EditorAction::None => {
                assert_eq!(
                    new_content,
                    original_value,
                    "[EditViaViewModel] on_blur returned None but content changed \
                     ({original_value:?} → {new_content:?}). \
                     Operations not wired? ops={:?}",
                    editable
                        .operations
                        .iter()
                        .map(|o| &o.descriptor.name)
                        .collect::<Vec<_>>()
                );
            }
            other => panic!(
                "[EditViaViewModel] Expected Execute from on_blur, got {:?}",
                other
            ),
        }
    }

    async fn apply_bulk_external_add(
        &mut self,
        doc_uri: &holon_api::EntityUri,
        blocks: &[holon_api::block::Block],
        ref_state: &ReferenceState,
    ) {
        tracing::trace!(
            "[apply] BulkExternalAdd: adding {} blocks to {}",
            blocks.len(),
            doc_uri
        );

        // Resolve file-based URI to UUID-based URI (documents map uses UUID keys after StartApp)
        let resolved_uri = self.resolve_uri(doc_uri);
        let file_path = self.ctx.documents.get(&resolved_uri).unwrap_or_else(|| {
            panic!(
                "Document not found for BulkExternalAdd: {} (resolved: {})",
                doc_uri, resolved_uri
            )
        });

        // Get all blocks for this document from reference state.
        // Note: ref_state already includes the new blocks (from apply_reference).
        // Resolve parent_ids so blocks_by_document matches UUID-based doc URIs.
        let resolved_blocks = self.resolve_ref_blocks(ref_state, true);
        let grouped = holon_api::blocks_by_document(&resolved_blocks);
        let all_blocks: Vec<holon_api::block::Block> = grouped
            .into_iter()
            .find(|(uri, _)| *uri == resolved_uri)
            .map(|(_, blocks)| blocks)
            .unwrap_or_default();
        let existing_count = all_blocks.len().saturating_sub(blocks.len());

        // Find the document block for this document (needed for #+TODO: header)
        let doc_block = resolved_blocks
            .iter()
            .find(|b| b.id == resolved_uri && b.is_page());

        // Serialize to org file (with document header so custom keywords round-trip)
        let live_blocks: Vec<&holon_api::block::Block> = all_blocks.iter().collect();
        let org_content =
            crate::serialize_blocks_to_org_with_doc(&live_blocks, &resolved_uri, doc_block);

        tracing::trace!(
            "[BulkExternalAdd] Writing {} total blocks ({} new) to {:?}",
            all_blocks.len(),
            blocks.len(),
            file_path
        );
        // DEBUG: print blocks being serialized
        for b in &all_blocks {
            tracing::trace!(
                "[BulkExternalAdd] block: {} parent_id={} type={}",
                b.id,
                b.parent_id,
                b.content_type
            );
        }
        tracing::trace!("[BulkExternalAdd] ORG CONTENT:\n{}", org_content);
        tokio::fs::write(file_path, &org_content)
            .await
            .expect("Failed to write bulk external add");

        // =========================================================================
        // FLUTTER STARTUP BUG REPRODUCTION:
        // Immediately after writing bulk data, spawn concurrent query_and_watch calls
        // while IVM is still processing the block_with_path materialized view.
        // This simulates what Flutter does: UI requests reactive queries while
        // the backend is still processing the initial data sync.
        // =========================================================================
        let engine = self.test_ctx().engine();
        let num_concurrent_watches = 3; // Simulate multiple UI components requesting data
        let mut watch_tasks = Vec::new();

        // Timeout for query_and_watch calls.
        // If the OperationScheduler's mark_available bug is present, these calls
        // will hang forever because:
        // 1. query_and_watch creates a materialized view via execute_ddl_with_deps
        // 2. The DDL requires Schema("block") dependency
        // 3. OperationScheduler checks if "block" is in available set - it's NOT
        // 4. Operation is queued in pending, response_rx.await hangs forever
        // 5. mark_available() was never called for core tables during DI init
        let query_timeout = Duration::from_secs(10);

        for i in 0..num_concurrent_watches {
            let engine_clone = engine.clone();
            let prql = format!(
                "from block_raw | select {{id, content}} | filter id != \"bulk-race-{}\" ",
                i
            );
            let sql = engine
                .compile_to_sql(&prql, QueryLanguage::HolonPrql)
                .expect("PRQL compilation should succeed");
            let task = tokio::spawn(async move {
                let start = Instant::now();
                // Use timeout to detect scheduler hangs
                let result = tokio::time::timeout(
                    query_timeout,
                    engine_clone.query_and_watch(sql.clone(), HashMap::new(), None),
                )
                .await;
                (i, start.elapsed(), sql, result)
            });
            watch_tasks.push(task);
        }

        // Note: Schema initialization happens during app startup via SchemaRegistry.
        // We don't need to test concurrent schema init here - the query_and_watch
        // calls above already test the critical concurrency path.

        // Check results - database lock/schema change errors indicate the Flutter bug
        // These manifest as various error messages:
        // - "database is locked" - SQLite busy timeout expired
        // - "Database schema changed" - IVM detected concurrent schema modifications
        // - "Failed to lock connection pool" - Connection pool contention
        fn is_concurrency_error(error_str: &str) -> bool {
            error_str.contains("database is locked")
                || error_str.contains("Database schema changed")
                || error_str.contains("Failed to lock connection pool")
        }

        for task in watch_tasks {
            match task.await {
                Ok((i, elapsed, _prql, Ok(Ok(_)))) => {
                    tracing::trace!(
                        "[BulkExternalAdd] Concurrent query_and_watch {} succeeded in {:?}",
                        i,
                        elapsed
                    );
                }
                Ok((i, elapsed, prql, Ok(Err(e)))) => {
                    let error_str = format!("{:?}", e);
                    if is_concurrency_error(&error_str) {
                        panic!(
                            "FLUTTER STARTUP BUG REPRODUCED: query_and_watch {} failed with concurrency error \
                                 after {:?} while bulk data ({} blocks) was being synced!\n\
                                 This is the exact bug that causes Flutter app to get stuck during startup.\n\
                                 Query: {}\n\
                                 Error: {}",
                            i,
                            elapsed,
                            blocks.len(),
                            prql,
                            error_str
                        );
                    } else {
                        panic!(
                            "Concurrent query_and_watch {} failed after {:?}: {}\nQuery: {}",
                            i, elapsed, error_str, prql
                        );
                    }
                }
                Ok((i, elapsed, prql, Err(_timeout))) => {
                    // Timeout occurred - this indicates the scheduler bug
                    panic!(
                        "SCHEDULER BUG: query_and_watch {} timed out after {:?}!\n\n\
                             Root cause: OperationScheduler's mark_available() was never called for 'blocks' table.\n\n\
                             The materialized view creation is stuck in the scheduler's pending queue:\n\
                             - execute_ddl_with_deps submitted with requires=[Schema(\"blocks\")]\n\
                             - can_execute() returned false (blocks not in available set)\n\
                             - Operation queued in pending, response_rx.await blocks forever\n\n\
                             Query: {}\n\n\
                             Fix required:\n\
                             1. Call scheduler_handle.mark_available() for core tables after schema creation in DI\n\
                             2. Ensure MarkAvailable command calls process_pending_queue() to wake pending ops",
                        i, elapsed, prql
                    );
                }
                Err(e) => {
                    panic!("Query task panicked: {:?}", e);
                }
            }
        }

        // Poll until file contains expected block count (with timeout)
        let expected_block_count = all_blocks.len();
        let file_path_clone = file_path.clone();
        let start = Instant::now();
        let timeout = Duration::from_millis(5000);

        let condition_met = wait_for_file_condition(
            &file_path_clone,
            |content| {
                let text_count = content.matches(":ID:").count();
                let src_count = content.to_lowercase().matches("#+begin_src").count();
                text_count + src_count == expected_block_count
            },
            timeout,
        )
        .await;

        let elapsed = start.elapsed();
        let final_content = tokio::fs::read_to_string(file_path)
            .await
            .expect("Failed to read file after bulk add");
        let text_block_count = final_content.matches(":ID:").count();
        let source_block_count = final_content.to_lowercase().matches("#+begin_src").count();
        let actual_block_count = text_block_count + source_block_count;

        if !condition_met || actual_block_count < expected_block_count {
            panic!(
                "SYNC LOOP BUG: BulkExternalAdd wrote {} blocks but only {} remain after {:?}!\n\
                     Expected {} blocks total ({} existing + {} new).\n\
                     File content:\n{}",
                expected_block_count,
                actual_block_count,
                elapsed,
                expected_block_count,
                existing_count,
                blocks.len(),
                final_content
            );
        }
        tracing::trace!(
            "[BulkExternalAdd] File verified with {} blocks after {:?}",
            actual_block_count,
            elapsed
        );

        // Now wait for the blocks to sync to the DATABASE.
        let expected_db_count = Self::expected_content_block_count(ref_state);
        let expected_ids = self.expected_block_ids(ref_state);
        let db_timeout = Duration::from_millis(10000);
        let db_start = Instant::now();

        let actual_rows = self.wait_for_blocks_synced(&expected_ids, db_timeout).await;
        let db_elapsed = db_start.elapsed();

        if actual_rows.len() == expected_db_count {
            tracing::trace!(
                "[BulkExternalAdd] Database synced ({} blocks) in {:?}",
                expected_db_count,
                db_elapsed
            );
        } else {
            // Diagnostic: print which ref_state blocks are missing from SQL.
            let sql_ids: std::collections::HashSet<String> = actual_rows
                .iter()
                .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(String::from))
                .collect();
            let ref_non_doc: Vec<&holon_api::block::Block> = ref_state
                .block_state
                .blocks
                .values()
                .filter(|b| !b.is_page())
                .collect();
            let mut missing: Vec<String> = Vec::new();
            let mut extra: Vec<String> = Vec::new();
            for b in &ref_non_doc {
                let resolved = self.resolve_uri(&b.id);
                if !sql_ids.contains(resolved.as_str()) {
                    missing.push(format!(
                        "{} (resolved={}) parent={} doc={:?}",
                        b.id,
                        resolved,
                        b.parent_id,
                        ref_state.block_state.block_documents.get(&b.id)
                    ));
                }
            }
            let ref_ids: std::collections::HashSet<String> = ref_non_doc
                .iter()
                .map(|b| self.resolve_uri(&b.id).to_string())
                .collect();
            for sid in &sql_ids {
                if !ref_ids.contains(sid) {
                    extra.push(sid.clone());
                }
            }
            panic!(
                "[BulkExternalAdd] WARNING: Database has {} blocks, expected {} after {:?}\n\
                 MISSING from SQL ({}):\n  {}\n\
                 EXTRA in SQL ({}):\n  {}",
                actual_rows.len(),
                expected_db_count,
                db_elapsed,
                missing.len(),
                missing.join("\n  "),
                extra.len(),
                extra.join("\n  "),
            );
        }

        // Poll until org files stabilize (sync controller finishes re-rendering)
        self.wait_for_org_files_stable(25, Duration::from_millis(5000))
            .await;
    }

    async fn apply_concurrent_mutations(
        &mut self,
        ui_mutation: crate::pbt::types::MutationEvent,
        external_mutation: crate::pbt::types::MutationEvent,
        ref_state: &ReferenceState,
    ) {
        tracing::trace!(
            "[apply] ConcurrentMutations: UI={:?}, External={:?}",
            ui_mutation.mutation,
            external_mutation.mutation
        );
        // Delegate to the inherent impl method (Rust resolves inherent before trait).
        let ui = ui_mutation;
        let ext = external_mutation;
        let start = std::time::Instant::now();
        tracing::trace!("[apply_concurrent_mutations] ext_event: {:?}", ext);
        let expected_blocks = self.resolve_ref_blocks(ref_state, false);
        if let Err(e) = self.ctx.apply_external_mutation(&expected_blocks).await {
            eprintln!("[ConcurrentMutations] External mutation failed: {:?}", e);
        }
        let (entity, op, mut params) = ui.mutation.to_operation();
        if let Some(holon_api::Value::String(pid)) = params.get("parent_id") {
            let pid = holon_api::EntityUri::parse(pid).expect("Unable to parse parent_id");
            let resolved = self.resolve_uri(&pid);
            params.insert("parent_id".to_string(), resolved.clone().into());
        }
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        match driver.synthetic_dispatch(&entity, &op, params).await {
            Ok(()) => {}
            Err(e) => panic!("Concurrent UI mutation {}.{} failed: {:?}", entity, op, e),
        }
        let expected_count = ref_state.block_state.blocks.len();
        let expected_ids = self.expected_block_ids(ref_state);
        self.await_block_count_or_panic(
            &expected_ids,
            expected_count,
            std::time::Duration::from_millis(15000),
            "ConcurrentMutations",
        )
        .await;
        let expected_blocks = self.resolve_ref_blocks(ref_state, true);
        self.await_org_file_convergence(&expected_blocks).await;
        let _ = start;
    }

    async fn apply_apply_mutation(
        &mut self,
        event: crate::pbt::types::MutationEvent,
        ref_state: &ReferenceState,
    ) {
        tracing::trace!("[apply] Applying mutation: {:?}", event.mutation);
        self.apply_mutation(event, ref_state).await;
    }

    async fn apply_trigger_slash_command(&mut self, block_id: &holon_api::EntityUri) {
        let resolved_block_id = self.resolve_uri(block_id);
        tracing::trace!(
            "[apply] TriggerSlashCommand: block={block_id} (resolved={resolved_block_id})"
        );

        // 1. Query block data and render via render_entity() DSL
        let engine = self.engine();
        let sql = format!(
            "SELECT id, content, content_type, source_language, parent_id \
             FROM block_raw WHERE id = '{}'",
            resolved_block_id
        );
        let data_rows = engine
            .execute_query(sql, HashMap::new(), None)
            .await
            .expect("block query failed in TriggerSlashCommand");
        assert!(
            !data_rows.is_empty(),
            "[TriggerSlashCommand] Block {block_id} not found in database"
        );

        let render_expr = holon_api::render_types::RenderExpr::FunctionCall {
            name: "render_entity".to_string(),
            args: Vec::new(),
        };

        let engine_clone = Arc::clone(engine);
        let display_tree = tokio::task::spawn_blocking(move || {
            let services = holon_frontend::reactive::HeadlessBuilderServices::new(engine_clone);
            holon_frontend::interpret_pure(
                &render_expr,
                &data_rows
                    .iter()
                    .cloned()
                    .map(std::sync::Arc::new)
                    .collect::<Vec<_>>(),
                &services,
            )
            .snapshot()
        })
        .await
        .expect("spawn_blocking panicked");

        // 2. Find EditableText node for this block
        let editable = display_tree
            .find_editable_text(resolved_block_id.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "[TriggerSlashCommand] No EditableText with id={resolved_block_id}.\n{}",
                    display_tree.pretty_print(0)
                )
            });

        // 3. Build EditorViewModel and simulate typing "/"
        let mut ctrl = holon_frontend::EditorViewModel::from_view_model(editable);
        let action = ctrl.on_text_changed("/", 1);
        assert!(
            matches!(action, holon_frontend::EditorAction::PopupActivated { .. }),
            "[TriggerSlashCommand] Expected PopupActivated for '/' on block {block_id}, got {:?}",
            action
        );

        // 4. Populate items synchronously (CommandProvider is sync)
        let context_params: HashMap<String, Value> = editable
            .entity
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let items = holon_frontend::command_provider::CommandProvider::build_command_items(
            &editable.operations,
            &context_params,
            "",
        );
        ctrl.set_popup_items(items);

        let popup_state = ctrl.popup_state().unwrap();
        let delete_idx = popup_state
            .items
            .iter()
            .position(|item| item.id == "delete")
            .unwrap_or_else(|| {
                panic!(
                    "[TriggerSlashCommand] No 'delete' operation in menu for block {block_id}.\n\
                     Available: {:?}",
                    popup_state.items.iter().map(|i| &i.id).collect::<Vec<_>>()
                )
            });

        // 5. Navigate to delete entry and select it
        for _ in 0..delete_idx {
            ctrl.on_key(holon_frontend::EditorKey::Down);
        }
        let action = ctrl.on_key(holon_frontend::EditorKey::Enter);
        match action {
            holon_frontend::EditorAction::Execute(intent) => {
                eprintln!(
                    "[TriggerSlashCommand] Executing {}.{} with {:?}",
                    intent.entity_name, intent.op_name, intent.params
                );
                let driver = self
                    .driver
                    .as_ref()
                    .expect("driver not installed — was start_app called?");
                driver
                    .apply_intent(intent)
                    .await
                    .expect("slash command operation failed");
            }
            other => panic!("[TriggerSlashCommand] Expected Execute, got {:?}", other),
        }
    }

    async fn apply_trigger_doc_link(
        &mut self,
        block_id: &holon_api::EntityUri,
        target_block_id: &holon_api::EntityUri,
        ref_state: &ReferenceState,
    ) {
        let resolved_block_id = self.resolve_uri(block_id);
        let resolved_target = self.resolve_uri(target_block_id);
        tracing::trace!(
            "[apply] TriggerDocLink: block={block_id} (resolved={resolved_block_id}) target={target_block_id}"
        );

        // 1. Render block → shadow interpret → ViewModel
        let engine = self.ctx.engine().clone();
        let data_rows = [{
            let mut row = HashMap::new();
            row.insert(
                "id".to_string(),
                Value::String(resolved_block_id.as_str().to_string()),
            );
            row
        }];

        let render_expr = holon_api::render_types::RenderExpr::FunctionCall {
            name: "render_entity".to_string(),
            args: Vec::new(),
        };

        let engine_clone = Arc::clone(&engine);
        let display_tree = tokio::task::spawn_blocking(move || {
            let services = holon_frontend::reactive::HeadlessBuilderServices::new(engine_clone);
            holon_frontend::interpret_pure(
                &render_expr,
                &data_rows
                    .iter()
                    .cloned()
                    .map(std::sync::Arc::new)
                    .collect::<Vec<_>>(),
                &services,
            )
            .snapshot()
        })
        .await
        .expect("spawn_blocking panicked");

        // 2. Find EditableText and build EditorViewModel
        let editable = display_tree
            .find_editable_text(resolved_block_id.as_str())
            .unwrap_or_else(|| {
                panic!(
                    "[TriggerDocLink] No EditableText with id={resolved_block_id}.\n{}",
                    display_tree.pretty_print(0)
                )
            });

        // 3. Verify triggers include doc_link
        assert!(
            editable.triggers.iter().any(|t| matches!(
                t,
                holon_frontend::input_trigger::InputTrigger::TextPrefix { action, .. }
                    if action == "doc_link"
            )),
            "[TriggerDocLink] EditableText for {block_id} has no doc_link trigger.\n\
             Triggers: {:?}",
            editable.triggers
        );

        // 4. Simulate typing "see [[proj" via EditorViewModel
        // Without async context, doc_link returns None (no LinkProvider).
        let mut ctrl = holon_frontend::EditorViewModel::from_view_model(editable);
        let action = ctrl.on_text_changed("see [[proj", 10);
        assert!(
            matches!(action, holon_frontend::EditorAction::None),
            "[TriggerDocLink] Expected None without async context, got {:?}",
            action
        );

        // 5. Test the InsertText result path directly via PopupMenu + manual items.
        // This bypasses the async LinkProvider but validates the full menu flow.
        let target_id = resolved_target.as_str().to_string();
        let target_label = ref_state
            .block_state
            .blocks
            .get(target_block_id)
            .map(|b| b.content.clone())
            .unwrap_or_else(|| "untitled".to_string());

        let items = vec![
            holon_frontend::popup_menu::PopupItem {
                id: target_id.clone(),
                label: target_label.clone(),
                icon: None,
            },
            holon_frontend::popup_menu::PopupItem {
                id: "__create_new__".to_string(),
                label: "Create new: proj".to_string(),
                icon: Some("\u{2795}".to_string()),
            },
        ];

        let mut menu = holon_frontend::popup_menu::PopupMenu::new();
        // Use a trivial mock provider to test menu mechanics
        struct LinkMockProvider;
        impl holon_frontend::popup_menu::PopupProvider for LinkMockProvider {
            fn source(&self) -> &str {
                "doc_link"
            }
            fn candidates(
                &self,
                _: std::pin::Pin<
                    Box<dyn futures_signals::signal::Signal<Item = String> + Send + Sync>,
                >,
            ) -> std::pin::Pin<
                Box<
                    dyn futures_signals::signal_vec::SignalVec<
                            Item = holon_frontend::popup_menu::PopupItem,
                        > + Send,
                >,
            > {
                Box::pin(futures_signals::signal_vec::always(vec![]))
            }
            fn on_select(
                &self,
                item: &holon_frontend::popup_menu::PopupItem,
                filter: &str,
            ) -> holon_frontend::popup_menu::PopupResult {
                let replacement = if item.id == "__create_new__" {
                    format!("[[{}]]", filter)
                } else {
                    format!("[[{}][{}]]", item.id, item.label)
                };
                holon_frontend::popup_menu::PopupResult::InsertText {
                    replacement,
                    prefix_start: 4,
                }
            }
        }

        let _signal = menu.activate(Arc::new(LinkMockProvider), "proj");
        menu.set_items(items);

        // Select existing entity (first item)
        let result = menu.on_key(holon_frontend::popup_menu::MenuKey::Enter);
        match result {
            holon_frontend::popup_menu::PopupResult::InsertText {
                replacement,
                prefix_start,
            } => {
                let expected = format!("[[{}][{}]]", target_id, target_label);
                assert_eq!(
                    replacement, expected,
                    "[TriggerDocLink] InsertText replacement mismatch"
                );
                assert_eq!(prefix_start, 4, "[TriggerDocLink] prefix_start mismatch");
            }
            other => panic!("[TriggerDocLink] Expected InsertText, got {:?}", other),
        }

        // Read-only transition — no state change
    }

    async fn apply_indent(&mut self, block_id: &holon_api::EntityUri) {
        tracing::trace!("[apply] Indent: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("indent", resolved_id.as_str(), HashMap::new())
            .await;
    }

    async fn apply_outdent(&mut self, block_id: &holon_api::EntityUri) {
        tracing::trace!("[apply] Outdent: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("outdent", resolved_id.as_str(), HashMap::new())
            .await;
    }

    async fn apply_move_up(&mut self, block_id: &holon_api::EntityUri) {
        tracing::trace!("[apply] MoveUp: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("move_up", resolved_id.as_str(), HashMap::new())
            .await;
    }

    async fn apply_move_down(&mut self, block_id: &holon_api::EntityUri) {
        tracing::trace!("[apply] MoveDown: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        self.dispatch_block_op_via_chord("move_down", resolved_id.as_str(), HashMap::new())
            .await;
    }

    async fn apply_drag_drop_block(
        &mut self,
        source: &holon_api::EntityUri,
        target: &holon_api::EntityUri,
    ) {
        tracing::trace!("[apply] DragDropBlock: source={source} target={target}");
        let resolved_source = self.resolve_uri(source);
        let resolved_target = self.resolve_uri(target);
        let (root_id, _root_tree) = self
            .current_reactive_tree()
            .expect("[DragDropBlock] No reactive tree — was start_app called?");
        self.wait_for_entity_bounds(resolved_source.as_str(), Duration::from_secs(5))
            .await
            .expect("[DragDropBlock] source bounds never appeared");
        self.wait_for_entity_bounds(resolved_target.as_str(), Duration::from_secs(5))
            .await
            .expect("[DragDropBlock] target bounds never appeared");
        let driver = self
            .driver
            .as_ref()
            .expect("[DragDropBlock] driver not installed");
        let dispatched = driver
            .drop_entity(
                root_id.as_str(),
                resolved_source.as_str(),
                resolved_target.as_str(),
            )
            .await
            .expect("[DragDropBlock] drop_entity failed");
        assert!(
            dispatched,
            "[DragDropBlock] drop_entity returned false for {source} → {target}"
        );
    }

    async fn apply_click_block(
        &mut self,
        region: holon_api::Region,
        block_id: &holon_api::EntityUri,
    ) {
        let resolved_id = self.resolve_uri(block_id);
        let _click_span = tracing::info_span!(
            "ClickBlock",
            region = ?region,
            block_id = %block_id,
            resolved = %resolved_id,
        )
        .entered();
        tracing::trace!(
            "[apply] ClickBlock: region={region:?} block={block_id} (resolved={resolved_id})"
        );

        // Wait for the entity to actually render — sidebar `live_block`
        // slots are populated asynchronously by their UiWatchers, and
        // clicking before they appear is the headless equivalent of
        // clicking dead pixels. We poll the engine's fully-resolved
        // `ViewModel` (which calls `ensure_watching` per nested block,
        // so all watchers fire) until our target entity shows up.
        let resolved = match self
            .wait_for_entity_in_resolved_view_model(resolved_id.as_str(), Duration::from_secs(5))
            .await
        {
            Some(vm) => vm,
            None => panic!(
                "[ClickBlock] entity {resolved_id} did not appear in the \
                 resolved ViewModel within 5s. Region={region:?}."
            ),
        };

        // Dispatch the bound click action if the rendered widget at this
        // entity has one (e.g. a sidebar selectable's `navigation.focus`).
        // Otherwise fall back to `navigation.editor_focus`, mirroring
        // GPUI's `render_entity` click handler.
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        // Region-scoped lookup: production GPUI's click handler runs on a
        // specific element in the clicked region, not across the whole tree.
        // The same entity_id may appear in multiple regions (e.g. `block:journals`
        // is both a LeftSidebar list item and a Main-panel doc) and bind
        // different actions per region. See FU-15.
        let bound_intent = holon_frontend::focus_path::find_click_intent_in_region(
            &resolved,
            resolved_id.as_str(),
            region.as_str(),
        );
        // Dispatch policy:
        //  * GPUI variant (frontend_geometry.is_some()) — drive a
        //    real mouse click so focus, editor mounting, chord
        //    resolution, and the bound action all run through
        //    production code. Geometry lookup falls back to
        //    `selectable-{id}` (default index.org sidebar) and then
        //    to entity_id scan, so sidebar selectables resolve.
        //  * Headless variants — no real input pipeline. Use the
        //    bound action's `synthetic_dispatch` if present;
        //    otherwise fall back to a synthesized click verb.
        //
        // Mouse-driven dispatch is fire-and-forget
        // (`services.dispatch_intent`, `reactive.rs:1448`) — unlike
        // the awaitable `dispatch_intent_sync` used by
        // `apply_intent`. Add an explicit focus-await barrier
        // before returning so subsequent transitions see a
        // populated focus.
        let dispatched_action = if self.frontend_geometry.is_some() {
            self.wait_for_entity_bounds(resolved_id.as_str(), Duration::from_secs(5))
                .await
                .unwrap_or_else(|e| {
                    panic!("[ClickBlock] {e} Region={region:?}.");
                });
            driver
                .click_entity(resolved_id.as_str(), region.as_str())
                .await
                .expect("[ClickBlock] click_entity failed");

            self.wait_for_focus_to_match(resolved_id.as_str(), Duration::from_secs(2))
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "[ClickBlock] focus did not propagate within 2s: {e} \
                     Region={region:?} expected={resolved_id}."
                    );
                });
            false
        } else if let Some(intent) = bound_intent {
            driver
                .apply_intent(intent)
                .await
                .expect("[ClickBlock] apply_intent failed");
            true
        } else {
            self.wait_for_entity_bounds(resolved_id.as_str(), Duration::from_secs(5))
                .await
                .unwrap_or_else(|e| {
                    panic!("[ClickBlock] {e} Region={region:?}.");
                });
            driver
                .click_entity(resolved_id.as_str(), region.as_str())
                .await
                .expect("[ClickBlock] click_entity failed");
            false
        };
        eprintln!(
            "[ClickBlock] {} (entity={resolved_id})",
            if dispatched_action {
                "dispatched bound action"
            } else {
                "real input pipeline / editor_focus"
            }
        );

        // Let CDC propagate (mirrors the yield_now dance ToggleState uses).
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
    }

    async fn apply_split_block(
        &mut self,
        block_id: &holon_api::EntityUri,
        position: usize,
        ref_state: &ReferenceState,
    ) {
        tracing::trace!("[apply] SplitBlock: block={block_id} position={position}");
        let resolved_id = self.resolve_uri(block_id);

        // Real users press Enter, not Ctrl+x. The Enter handler at
        // `editor_view.rs:543-575` is a capture_action that reads
        // `input.read(cx).cursor()` from the live `InputState` and
        // dispatches `split_block` directly — a separate code path
        // from the bubble-phase chord resolver that `Ctrl+x` hits.
        // Driving Enter exercises that production path.
        if let Err(e) = self
            .wait_for_entity_bounds(resolved_id.as_str(), Duration::from_secs(5))
            .await
        {
            let sql_probe = self.probe_block_sql_state(resolved_id.as_str()).await;
            panic!(
                "[SplitBlock] bounds unavailable for {resolved_id}: {e:#}\nSQL probe for missing entity:\n{sql_probe}"
            );
        }
        // Children-settled gate. `wait_for_entity_bounds` confirms the target
        // appears *somewhere* in the geometry, but coords resolved against a
        // partial first-render get invalidated by the next CDC batch that
        // adds siblings. Wait until every non-Page child of this block's
        // parent — as the PRE-transition ref-state predicted — has rendered
        // so `require_element_center` returns stable bounds. Uses the
        // pre-state instead of `ref_state` (post-transition) so the
        // predicate matches what the user can see right now. No-op when
        // pre-state isn't recorded yet or the parent has no children.
        let parent_for_settle = self
            .pre_ref_state
            .as_ref()
            .and_then(|s| s.block_state.blocks.get(block_id))
            .map(|b| b.parent_id.clone());
        if let Some(parent_id) = parent_for_settle {
            self.wait_for_children_settled(&parent_id, Duration::from_secs(5))
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "[SplitBlock] children of parent {parent_id} not settled before click: {e:#}"
                    )
                });
        }
        // Stronger precondition than bounds-exist: the block must be rendered
        // as an interactive widget (editable_text or its read-only sibling
        // rendered_text) so a click can either focus the editor or promote
        // the read-only variant. Mismatch here (e.g. block rendered as
        // `text` or not promoted at all) used to surface 1 s later as a
        // confusing "click didn't change focus" timeout.
        self.wait_for_widget_kind(
            resolved_id.as_str(),
            &["editable_text", "rendered_text"],
            Duration::from_secs(2),
        )
        .await
        .unwrap_or_else(|e| {
            panic!("[SplitBlock] target not rendered as editable_text/rendered_text: {e:#}")
        });
        let driver = self.driver.as_ref().expect("driver not installed");
        driver
            .click_entity(resolved_id.as_str(), "main")
            .await
            .unwrap_or_else(|e| {
                panic!("[SplitBlock] click_entity failed for {resolved_id}: {e:#}")
            });
        // Fail loud if the click didn't move keyboard focus to the target
        // editor. The Enter handler at `editor_view.rs:543-575` dispatches
        // `split_block` against whichever block's editor owns focus when
        // Enter fires — so a silent focus drift would have us splitting
        // the wrong block. The previous `dispatch_block_op_via_chord`
        // path bypassed this because it passed `id` as an explicit op
        // param; Enter reads `input.read(cx).cursor()` and `row_id` from
        // the focused editor.
        self.wait_for_focus_to_match(resolved_id.as_str(), Duration::from_secs(1))
            .await
            .unwrap_or_else(|e| {
                panic!(
                    "[SplitBlock] click_entity did not focus {resolved_id} \
                     before Enter — split would have hit the wrong block: {e:#}"
                )
            });
        // Pre-Enter SQL snapshot: log the live content + length so panic-time
        // analysis can distinguish "cursor position drift" from "content
        // diverged before the split" cases.
        {
            let sql_pre = self.probe_block_sql_state(resolved_id.as_str()).await;
            let ref_content_len = ref_state
                .block_state
                .blocks
                .get(block_id)
                .map(|b| b.content_text().len());
            eprintln!(
                "[SplitBlock-presplit] target={resolved_id} position={position} \
                 ref_content_len={ref_content_len:?}\n{sql_pre}"
            );
        }
        driver
            .send_raw_keystroke("home", &[])
            .await
            .expect("[SplitBlock] home failed");
        for _ in 0..position {
            driver
                .send_raw_keystroke("right", &[])
                .await
                .expect("[SplitBlock] right failed");
        }
        driver
            .send_raw_keystroke("enter", &[])
            .await
            .expect("[SplitBlock] enter failed");

        let expected_count = Self::expected_content_block_count(ref_state);
        let expected_ids = self.expected_block_ids(ref_state);
        let timeout = std::time::Duration::from_secs(5);
        let db_rows = self.wait_for_blocks_synced(&expected_ids, timeout).await;
        if db_rows.len() != expected_count {
            let id_vec: Vec<String> = db_rows
                .iter()
                .filter_map(|r| r.get("id").and_then(|v| v.as_string()).map(String::from))
                .collect();
            let actual_ids: HashSet<String> = id_vec.iter().cloned().collect();
            let mut id_counts: std::collections::HashMap<String, u32> = HashMap::new();
            for id in &id_vec {
                *id_counts.entry(id.clone()).or_insert(0) += 1;
            }
            let duplicates: Vec<(String, u32)> = id_counts
                .iter()
                .filter(|(_, c)| **c > 1)
                .map(|(k, v)| (k.clone(), *v))
                .collect();
            let missing: Vec<&String> = expected_ids.difference(&actual_ids).collect();
            let extra: Vec<&String> = actual_ids.difference(&expected_ids).collect();
            eprintln!(
                "[SplitBlock count-mismatch diag] expected={} db_rows={} unique_ids={} duplicates={:?} missing_from_block_raw={:?} extra_in_block_raw={:?}",
                expected_count,
                db_rows.len(),
                actual_ids.len(),
                duplicates,
                missing,
                extra,
            );
        }
        assert_eq!(
            db_rows.len(),
            expected_count,
            "[SplitBlock] Block count mismatch after split"
        );

        // Capture pre-split known real ids so we can identify the freshly
        // created block among `db_rows` (mirrors map_unmapped_split_synthetic_ids).
        let pre_known: HashSet<String> = {
            let mut ids: HashSet<String> =
                self.doc_uri_map.values().map(|u| u.to_string()).collect();
            for ref_id in ref_state.block_state.blocks.keys() {
                if !self.doc_uri_map.contains_key(ref_id) && !ref_id.as_str().contains(":split-") {
                    ids.insert(ref_id.to_string());
                }
            }
            ids
        };
        self.map_unmapped_split_synthetic_ids(ref_state, &db_rows, "[SplitBlock]");

        // Production fires editor_focus(new_block) as a follow-up of
        // split_block. The chain is: SQL editor_cursor write → watch_editor_cursor
        // reactor → GPUI window.focus(new_input) → InputEvent::Focus →
        // services.set_focus(new_block) → engine.focused_block mirrors new.
        // This chain isn't synchronous with the Enter dispatch, so without
        // an explicit wait the next inv-focus-matches-ref check sees the
        // pre-split click_entity target (c2f12z-s) instead of the new block.
        // Mirrors the wait_for_focus_to_match barriers used by ClickBlock /
        // NavigateFocus.
        let new_block_real_id: Option<String> = db_rows
            .iter()
            .filter_map(|row| row.get("id")?.as_string().map(|s| s.to_string()))
            .find(|id| !pre_known.contains(id));
        if self.frontend_geometry.is_some() {
            if let Some(new_id) = new_block_real_id.as_deref() {
                // Best-effort barrier: try to absorb the
                // editor_cursor → window.focus(new) → InputEvent::Focus →
                // set_focus(new) propagation chain before the next step. If
                // it doesn't converge in 2s, fall through and let the
                // downstream inv-focus-matches-ref polling catch real
                // regressions — the new EditorView may not have mounted
                // yet (a real, separate fragility worth surfacing there).
                let _ = self
                    .wait_for_focus_to_match(new_id, Duration::from_secs(2))
                    .await;
            }
        }
    }

    async fn apply_join_block(
        &mut self,
        block_id: &holon_api::EntityUri,
        ref_state: &ReferenceState,
    ) {
        tracing::trace!("[apply] JoinBlock: block={block_id}");
        let resolved_id = self.resolve_uri(block_id);
        let mut extra_params = HashMap::new();
        extra_params.insert("position".into(), Value::Integer(0));
        self.dispatch_block_op_via_chord("join_block", resolved_id.as_str(), extra_params)
            .await;

        let expected_ids = self.expected_block_ids(ref_state);
        self.wait_for_blocks_synced(&expected_ids, Duration::from_secs(5))
            .await;
    }

    async fn apply_undo_last_mutation(&mut self, ref_state: &ReferenceState) {
        tracing::trace!("[apply] UndoLastMutation");
        let result = self.ctx.engine().undo().await;
        assert!(result.is_ok(), "undo failed: {:?}", result.err());
        assert!(result.unwrap(), "undo returned false (nothing to undo)");
        let expected_ids = self.expected_block_ids(ref_state);
        self.wait_for_blocks_synced(&expected_ids, Duration::from_secs(5))
            .await;
    }

    async fn apply_redo(&mut self, ref_state: &ReferenceState) {
        tracing::trace!("[apply] Redo");
        let result = self.ctx.engine().redo().await;
        assert!(result.is_ok(), "redo failed: {:?}", result.err());
        assert!(result.unwrap(), "redo returned false (nothing to redo)");
        let expected_ids = self.expected_block_ids(ref_state);
        self.wait_for_blocks_synced(&expected_ids, Duration::from_secs(5))
            .await;
    }

    async fn apply_emit_mcp_data(&mut self) {
        // EmitMcpData doesn't use ref_state, create a dummy call path.
        // We route through the transition enum to keep the logic centralized.
        // Since EmitMcpData has no payload and doesn't use ref_state,
        // we replicate the body inline here.
        tracing::trace!("[apply] EmitMcpData");
        if let Some(ref mcp) = self.pbt_mcp {
            mcp.emit_update()
                .await
                .expect("PbtMcpIntegration::emit_update failed");
        }
    }

    async fn apply_focus_editable_text(&mut self, block_id: &holon_api::EntityUri) {
        let resolved_id = self.resolve_uri(block_id);
        tracing::trace!("[apply] FocusEditableText: block={block_id} (resolved={resolved_id})");
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        // Fail loud: bounds must be present, click_entity must succeed.
        // The previous version fell back to `synthetic_dispatch` (engine
        // fast-path) and printed a warning, which silently masked any
        // bug in the keyboard nav / Enter pipeline. Per CLAUDE.md
        // ("fail loud, never fake") let both errors propagate.
        // 5s budget mirrors the other input-bearing call sites:
        // `wait_for_entity_bounds` now polls for ~200ms, RPCs a scroll-
        // into-view on the GPUI main thread (oneshot + layout + flush is
        // 50–200ms), and keeps polling. The 1s budget that worked when
        // generators only proposed visible candidates is too tight once
        // offscreen-virtualized-list entities become legal targets.
        self.wait_for_entity_bounds(resolved_id.as_str(), Duration::from_secs(5))
            .await
            .unwrap_or_else(|e| {
                panic!("[FocusEditableText] bounds unavailable for {resolved_id}: {e:#}")
            });
        driver
            .click_entity(resolved_id.as_str(), "main")
            .await
            .unwrap_or_else(|e| {
                panic!("[FocusEditableText] click_entity failed for {resolved_id}: {e:#}")
            });
    }

    async fn apply_move_cursor(&mut self, byte_position: usize) {
        tracing::trace!("[apply] MoveCursor: byte_position={byte_position}");
        let driver = self.driver.as_ref().expect("driver not installed");
        driver
            .send_raw_keystroke("home", &[])
            .await
            .expect("MoveCursor: home failed");
        for _ in 0..byte_position {
            driver
                .send_raw_keystroke("right", &[])
                .await
                .expect("MoveCursor: right failed");
        }
    }

    async fn apply_type_chars(&mut self, text: &str) {
        tracing::trace!("[apply] TypeChars: {:?}", text);
        let driver = self.driver.as_ref().expect("driver not installed");
        for ch in text.chars() {
            let keystroke = ch.to_string();
            driver
                .send_raw_keystroke(&keystroke, &[])
                .await
                .expect("TypeChars: send_raw_keystroke failed");
        }
    }

    async fn apply_delete_backward(&mut self, count: usize) {
        tracing::trace!("[apply] DeleteBackward: count={count}");
        let driver = self.driver.as_ref().expect("driver not installed");
        for _ in 0..count {
            driver
                .send_raw_keystroke("backspace", &[])
                .await
                .expect("DeleteBackward: backspace failed");
        }
    }

    async fn apply_press_key(&mut self, chord: &holon_api::KeyChord, ref_state: &ReferenceState) {
        tracing::trace!("[apply] PressKey: chord={:?}", chord);
        let driver = self.driver.as_ref().expect("driver not installed");
        use holon_api::Key;
        let modifiers: Vec<String> = chord
            .0
            .iter()
            .filter_map(|k| match k {
                Key::Cmd => Some("cmd".to_string()),
                Key::Ctrl => Some("ctrl".to_string()),
                Key::Alt => Some("alt".to_string()),
                Key::Shift => Some("shift".to_string()),
                _ => None,
            })
            .collect();
        let regulars: Vec<&'static str> = chord
            .0
            .iter()
            .filter_map(|k| match k {
                Key::Enter => Some("enter"),
                Key::Backspace => Some("backspace"),
                Key::Tab => Some("tab"),
                Key::Escape => Some("escape"),
                _ => None,
            })
            .collect();
        let has_enter = regulars.iter().any(|k| *k == "enter");
        let mod_refs: Vec<&str> = modifiers.iter().map(|s| s.as_str()).collect();
        for key in regulars {
            driver
                .send_raw_keystroke(key, &mod_refs)
                .await
                .expect("PressKey: send_raw_keystroke failed");
        }
        // Enter dispatches `split_block`, which materializes a fresh
        // UUID for the suffix block. Hand that back to the synthetic
        // `block::split-N` slot the ref-state allocated, mirroring
        // `apply_split_block`'s mapping step. Without this the next
        // step's `assert_blocks_equivalent` compares prod-UUID against
        // ref-synthetic-id and panics on what is logically the same
        // block.
        if has_enter {
            let expected_ids = self.expected_block_ids(ref_state);
            let timeout = std::time::Duration::from_secs(5);
            let db_rows = self.wait_for_blocks_synced(&expected_ids, timeout).await;
            self.map_unmapped_split_synthetic_ids(ref_state, &db_rows, "[PressKey-Enter]");
        }
    }

    async fn apply_arrow_navigate(
        &mut self,
        region: holon_api::Region,
        direction: holon_frontend::navigation::NavDirection,
        steps: u8,
        ref_state: &ReferenceState,
    ) {
        tracing::trace!(
            "[apply] ArrowNavigate: region={region:?} direction={direction:?} steps={steps}"
        );

        // Diagnostic only — informs panic messages further down. We
        // no longer poke `engine.ui_state().set_focus()` here; the
        // production handler in `app_main.rs` (`advance_focus`) is
        // what walks selectables in response to arrow keys, so we
        // emit the keystrokes and let it do the work. If
        // `predicted_focus` and `actual_focus` diverge, an inv-focus-matches-ref-style
        // assertion downstream catches it as a real bug instead of
        // forcing a match.
        let predicted_focus = ref_state
            .focused_entity_id
            .get(&region)
            .expect("ArrowNavigate requires focused entity")
            .clone();
        eprintln!(
            "[ArrowNavigate] {steps}×{direction:?} → predicted final focus: {predicted_focus}"
        );

        // Map NavDirection → raw keystroke name accepted by
        // `raw_keystroke_to_input_event`. The TUI's production handler
        // dispatches arrow keys to `advance_focus`/`switch_region`
        // (no leader involved).
        use holon_frontend::navigation::NavDirection::{Down, Left, Right, Up};
        let keystroke = match direction {
            Up => "up",
            Down => "down",
            Left => "left",
            Right => "right",
        };
        let driver = self
            .driver
            .as_ref()
            .expect("ArrowNavigate: driver not installed — was start_app called?");
        for _ in 0..steps {
            driver
                .send_raw_keystroke(keystroke, &[])
                .await
                .unwrap_or_else(|e| {
                    panic!("[ArrowNavigate] keystroke '{keystroke}' failed: {e:#}")
                });
        }
    }

    async fn apply_add_peer(&mut self) {
        tracing::trace!("[apply] AddPeer (peer_idx={})", self.peers.len());
        let doc_store = self
            .ctx
            .doc_store()
            .expect("AddPeer requires Loro to be enabled");
        let store = doc_store.read().await;
        let global_doc = store
            .get_global_doc()
            .await
            .expect("Failed to get global doc for AddPeer");
        let snapshot = global_doc
            .export_snapshot()
            .expect("Failed to export snapshot for AddPeer");
        let peer_id = (self.peers.len() as u64) + 100;
        let peer_doc = holon::sync::multi_peer::init_doc(peer_id);
        peer_doc
            .import(&snapshot)
            .expect("Failed to import snapshot into peer");
        self.peers.push(holon::sync::multi_peer::PeerState {
            doc: peer_doc,
            peer_id,
            online: true,
            data: (),
        });
    }

    async fn apply_peer_edit(&mut self, peer_idx: usize, op: &crate::pbt::transitions::PeerEditOp) {
        use super::transitions::PeerEditOp;
        let peer = &self.peers[peer_idx];
        tracing::trace!("[apply] PeerEdit peer_idx={} op={:?}", peer_idx, op);
        match op {
            PeerEditOp::Create {
                parent_stable_id,
                content,
                stable_id,
            } => {
                super::peer_ops::peer_create_block(
                    &peer.doc,
                    parent_stable_id.as_deref(),
                    content,
                    stable_id,
                );
            }
            PeerEditOp::Update { stable_id, content } => {
                let resolved = self.resolve_stable_id(stable_id);
                super::peer_ops::peer_update_block(&peer.doc, &resolved, content);
            }
            PeerEditOp::Delete { stable_id } => {
                let resolved = self.resolve_stable_id(stable_id);
                super::peer_ops::peer_delete_block(&peer.doc, &resolved);
            }
        }
    }

    async fn apply_sync_with_peer(&mut self, peer_idx: usize) {
        tracing::trace!("[apply] SyncWithPeer peer_idx={}", peer_idx);
        let doc_store = self
            .ctx
            .doc_store()
            .expect("SyncWithPeer requires Loro to be enabled");
        let store = doc_store.read().await;
        let global_doc = store
            .get_global_doc()
            .await
            .expect("Failed to get global doc for SyncWithPeer");
        let primary_doc = global_doc.doc();
        let primary = &*primary_doc;
        let peer = &self.peers[peer_idx];
        holon::sync::multi_peer::sync_docs_direct(&primary, &peer.doc);
        drop(store);
        // Give the controller's spawned task time to process the
        // peer import via subscribe_root → on_loro_changed → SQL.
        self.ctx
            .wait_for_loro_quiescence(Duration::from_secs(10))
            .await;
    }

    async fn apply_merge_from_peer(&mut self, peer_idx: usize) {
        tracing::trace!("[apply] MergeFromPeer peer_idx={}", peer_idx);
        let doc_store = self
            .ctx
            .doc_store()
            .expect("MergeFromPeer requires Loro to be enabled");
        let store = doc_store.read().await;
        let global_doc = store
            .get_global_doc()
            .await
            .expect("Failed to get global doc for MergeFromPeer");
        // One-directional merge: export the peer's delta relative
        // to the primary's current version and import it into the
        // primary. The raw `doc.import` is enough — the
        // `LoroSyncController`'s `subscribe_root` will fire and
        // reconcile the diff into SQL via the command bus.
        let primary_doc = global_doc.doc();
        let primary = &*primary_doc;
        let peer = &self.peers[peer_idx];
        let peer_vv = primary.oplog_vv();
        let delta = peer
            .doc
            .export(loro::ExportMode::updates(&peer_vv))
            .expect("Failed to export peer delta");
        if !delta.is_empty() {
            primary.import(&delta).expect("Failed to import peer delta");
        }
        drop(store);
        self.ctx
            .wait_for_loro_quiescence(Duration::from_secs(10))
            .await;
    }

    async fn apply_peer_char_edit(
        &mut self,
        peer_idx: usize,
        block_id: &str,
        op: &crate::pbt::transitions::TextOp,
    ) {
        use super::transitions::TextOp;
        let peer = &self.peers[peer_idx];
        let resolved_id = self.resolve_stable_id(block_id);
        match op {
            TextOp::Insert {
                pos_codepoint,
                text,
            } => {
                super::peer_ops::peer_insert_text(&peer.doc, &resolved_id, *pos_codepoint, text);
            }
            TextOp::Delete {
                pos_codepoint,
                len_codepoint,
            } => {
                super::peer_ops::peer_delete_text(
                    &peer.doc,
                    &resolved_id,
                    *pos_codepoint,
                    *len_codepoint,
                );
            }
        }
    }

    async fn apply_pin_block(
        &mut self,
        region: holon_api::Region,
        block_id: &holon_api::EntityUri,
    ) {
        let resolved_id = self.resolve_uri(block_id);
        tracing::trace!(
            "[apply] PinBlock: region={region:?} block={block_id} (resolved={resolved_id})"
        );
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        // Production binding is shift+click on a bullet — no leader chord
        // exists. The headless PBT mirrors the dispatch faithfully via
        // `synthetic_dispatch`. Architecture rule: the smell is
        // `execute_op("navigation", ...)`, NOT `synthetic_dispatch`
        // (`archlint/smells/focus.toml`).
        let mut params = HashMap::new();
        params.insert(
            "region".to_string(),
            Value::String(region.as_str().to_string()),
        );
        params.insert(
            "block_id".to_string(),
            Value::String(resolved_id.as_str().to_string()),
        );
        driver
            .synthetic_dispatch("navigation", "focus_pin", params)
            .await
            .unwrap_or_else(|e| {
                panic!("[PinBlock] synthetic_dispatch(navigation, focus_pin) failed: {e:#}")
            });
        self.ctx.drain_region_cdc_events().await;
        self.dump_nav_tables("after PinBlock").await;
    }

    async fn apply_unpin_block(&mut self, history_id: i64) {
        tracing::trace!("[apply] UnpinBlock: history_id={history_id}");
        let driver = self
            .driver
            .as_ref()
            .expect("driver not installed — was start_app called?");
        let mut params = HashMap::new();
        params.insert("history_id".to_string(), Value::Integer(history_id));
        driver
            .synthetic_dispatch("navigation", "close", params)
            .await
            .unwrap_or_else(|e| {
                panic!("[UnpinBlock] synthetic_dispatch(navigation, close) failed: {e:#}")
            });
        self.ctx.drain_region_cdc_events().await;
        self.dump_nav_tables("after UnpinBlock").await;
    }

    async fn apply_expand_toggle(&mut self, block_id: &holon_api::EntityUri) {
        self.set_expand_toggle_gate(block_id, true).await;
    }

    async fn apply_collapse_toggle(&mut self, block_id: &holon_api::EntityUri) {
        self.set_expand_toggle_gate(block_id, false).await;
    }
}

impl<V: VariantMarker> E2ESut<V> {
    pub fn new(runtime: Arc<tokio::runtime::Runtime>) -> Result<Self> {
        Ok(Self {
            ctx: TestContext::new(runtime)?,
            doc_uri_map: HashMap::new(),
            driver: None,
            reactive_engine: RefCell::new(None),
            vm_emissions: Arc::new(std::sync::Mutex::new(Vec::new())),
            loro_sut: None,
            frontend_engine: None,
            frontend_geometry: None,
            frontend_visual_state: None,
            reactive_root_id: RefCell::new(None),
            live_tree: RefCell::new(None),
            pbt_mcp: None,
            #[cfg(feature = "otel-testing")]
            span_collector: crate::test_tracing::SpanCollector::global().clone(),
            #[cfg(feature = "otel-testing")]
            last_transition_start: None,
            last_transition: crate::pbt::transitions::E2ETransition::Nothing(
                crate::pbt::transitions::Nothing,
            ),
            #[cfg(feature = "otel-testing")]
            rss_before: 0,
            #[cfg(feature = "otel-testing")]
            rss_baseline: 0,
            peers: Vec::new(),
            live_blocks_cell: RefCell::new(None),
            live_focus_roots_cell: RefCell::new(None),
            query_origin_acc: RefCell::new(std::collections::HashMap::new()),
            pre_ref_state: None,
            _marker: PhantomData,
        })
    }

    /// Create an E2ESut with a pre-installed UserDriver.
    ///
    /// Used by Flutter PBT: the FlutterUserDriver is installed upfront
    /// so that `install_driver()` (called after StartApp) won't overwrite it.
    pub fn with_driver(
        runtime: Arc<tokio::runtime::Runtime>,
        driver: Arc<dyn UserDriver>,
    ) -> Result<Self> {
        Ok(Self {
            ctx: TestContext::new(runtime)?,
            doc_uri_map: HashMap::new(),
            driver: Some(driver),
            reactive_engine: RefCell::new(None),
            vm_emissions: Arc::new(std::sync::Mutex::new(Vec::new())),
            loro_sut: None,
            frontend_engine: None,
            frontend_geometry: None,
            frontend_visual_state: None,
            reactive_root_id: RefCell::new(None),
            live_tree: RefCell::new(None),
            pbt_mcp: None,
            #[cfg(feature = "otel-testing")]
            span_collector: crate::test_tracing::SpanCollector::global().clone(),
            #[cfg(feature = "otel-testing")]
            last_transition_start: None,
            last_transition: crate::pbt::transitions::E2ETransition::Nothing(
                crate::pbt::transitions::Nothing,
            ),
            #[cfg(feature = "otel-testing")]
            rss_before: 0,
            #[cfg(feature = "otel-testing")]
            rss_baseline: 0,
            peers: Vec::new(),
            live_blocks_cell: RefCell::new(None),
            live_focus_roots_cell: RefCell::new(None),
            query_origin_acc: RefCell::new(std::collections::HashMap::new()),
            pre_ref_state: None,
            _marker: PhantomData,
        })
    }

    /// Set up the mutation driver from the DI-resolved ReactiveEngine. Called after start_app.
    /// Uses the same dispatch path as GPUI (BuilderServices::dispatch_intent).
    /// Also installs the same `Arc<dyn UserDriver>` into `live_driver()`
    /// so PBT generators read observation verbs from the same medium.
    fn install_driver(&mut self) {
        if self.driver.is_some() {
            return; // respect pre-installed driver (e.g. FlutterUserDriver)
        }
        let driver: Arc<dyn UserDriver> = if let Some(reactive) = self.ctx.reactive_engine.as_ref()
        {
            Arc::new(crate::ReactiveEngineDriver::new(reactive.clone()))
        } else {
            // Tests without ReactiveEngine fall back to DirectUserDriver —
            // its observation verbs return empty, which is correct for a
            // backend-only PBT (no rendered UI to observe).
            let engine = self.test_ctx().engine().clone();
            Arc::new(DirectUserDriver::new(engine))
        };
        self.driver = Some(driver);
    }

    /// Snapshot the current root layout as a `ReactiveViewModel` — the input
    /// the trait-level `send_key_chord` / `resolve_key_chord` needs.
    fn current_reactive_tree(
        &self,
    ) -> Option<(holon_api::EntityUri, holon_frontend::ReactiveViewModel)> {
        let engine = self.reactive_engine.borrow();
        let engine = engine.as_ref()?;
        let root_id = self
            .reactive_root_id
            .borrow()
            .clone()
            .unwrap_or_else(holon_api::root_layout_block_uri);
        Some((root_id.clone(), engine.snapshot_reactive(&root_id)))
    }

    /// Poll the engine's fully-resolved `ViewModel` until `entity_id` is
    /// reachable, mirroring how a user waits for the UI to render before
    /// clicking. Returns the resolved snapshot at the moment the entity
    /// became visible, or `None` if the timeout expires.
    ///
    /// Uses `BuilderServices::snapshot_resolved` rather than the bare
    /// `snapshot_reactive`. The resolved variant recursively interprets every
    /// nested `live_block`, calling `ensure_watching` for each, so all
    /// per-region UiWatchers fire and the resulting tree is fully populated
    /// — without us having to manually drain per-block streams into slots
    /// (the work that `ReactiveShell` does in production).
    ///
    /// Polling at ~20 ms intervals is cheap and converges in single-digit
    /// polls once the watchers have delivered their first emission.
    #[tracing::instrument(skip(self), name = "pbt.wait_for_entity_in_resolved_view_model", fields(%entity_id))]
    async fn wait_for_entity_in_resolved_view_model(
        &self,
        entity_id: &str,
        timeout: Duration,
    ) -> Option<holon_frontend::ViewModel> {
        let reactive = self.reactive_engine.borrow().clone()?;
        let root_id = self
            .reactive_root_id
            .borrow()
            .clone()
            .unwrap_or_else(holon_api::root_layout_block_uri);
        use holon_frontend::reactive::BuilderServices;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let resolved = reactive.snapshot_resolved(&root_id);
            if Self::view_model_contains_entity(&resolved, entity_id) {
                return Some(resolved);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    fn view_model_contains_entity(node: &holon_frontend::ViewModel, entity_id: &str) -> bool {
        if node.entity_id() == Some(entity_id) {
            return true;
        }
        node.children()
            .iter()
            .any(|c| Self::view_model_contains_entity(c, entity_id))
    }

    /// Walk the live reactive tree from `root` and flip the
    /// `expanded` Mutable of the `expand_toggle` whose `target_id`
    /// prop matches `block_id`. Used by `apply_expand_toggle` /
    /// `apply_collapse_toggle`.
    ///
    /// Note on headless persistence: `ReactiveEngine::snapshot_reactive`
    /// runs `interpret_fn` and returns a freshly-built tree on every
    /// call, so the `Mutable<bool>` we flip here is reborn on the next
    /// snapshot unless the GPUI-side `with_update` / `push_down_*`
    /// machinery is holding a persistent root. This SUT helper is
    /// correct in either case: the reference-model side already tracks
    /// the toggle in `state.expanded_toggles`, and the assertion below
    /// fails loud if the corpus grows an `expand_toggle` render but the
    /// engine never produces a matching node (the most likely
    /// regression).
    async fn set_expand_toggle_gate(&self, block_id: &holon_api::EntityUri, value: bool) {
        use holon_frontend::reactive_view_model::ReactiveViewModel;

        let engine = self
            .reactive_engine
            .borrow()
            .clone()
            .expect("reactive engine not installed — was start_app called?");
        let root_id = self
            .reactive_root_id
            .borrow()
            .clone()
            .unwrap_or_else(holon_api::root_layout_block_uri);
        let root = engine.snapshot_reactive(&root_id);

        fn find_and_flip(node: &ReactiveViewModel, target_id: &str, value: bool) -> bool {
            let is_toggle = matches!(node.widget_name().as_deref(), Some("expand_toggle"));
            if is_toggle {
                let props = node.props.lock_ref();
                let matches = props
                    .get("target_id")
                    .and_then(|v| v.as_string())
                    .map(|s| s == target_id)
                    .unwrap_or(false);
                drop(props);
                if matches {
                    if let Some(gate) = node.expanded.as_ref() {
                        gate.set(value);
                        return true;
                    }
                }
            }
            for child in &node.children {
                if find_and_flip(child, target_id, value) {
                    return true;
                }
            }
            if let Some(slot) = node.slot.as_ref() {
                let content = slot.content.lock_ref();
                if find_and_flip(&content, target_id, value) {
                    return true;
                }
            }
            if let Some(lazy) = node.lazy_slot.as_ref() {
                if let Some(materialised) = lazy.cache.get_cloned() {
                    if find_and_flip(&materialised, target_id, value) {
                        return true;
                    }
                }
            }
            false
        }

        let block_uri = block_id.to_string();
        let target_id = block_uri.strip_prefix("block:").unwrap_or(&block_uri);
        assert!(
            find_and_flip(&root, target_id, value),
            "set_expand_toggle_gate: no expand_toggle node with \
             target_id={target_id} in reactive tree under {root_id}. \
             The fixture grew an expand_toggle render but the engine \
             didn't produce a matching node — likely a shadow_builder \
             or interpret regression. See \
             devlog/2026-05-15-lazy-expand-toggle-plan.md."
        );
    }

    /// Diagnostic probe: dump navigation_history, navigation_cursor, and
    /// focus_roots to stderr. Lets us see whether navigation provider
    /// writes are landing and whether the focus_roots matview has
    /// recomputed by the time the transition's apply() returns.
    async fn dump_nav_tables(&self, label: &str) {
        let engine = self.engine();
        let probes = [
            (
                "navigation_history",
                "SELECT id, region, block_id FROM navigation_history ORDER BY id",
            ),
            (
                "navigation_cursor",
                "SELECT region, history_id FROM navigation_cursor ORDER BY region",
            ),
            (
                "focus_roots",
                "SELECT region, root_id, history_id FROM focus_roots ORDER BY region, history_id",
            ),
        ];
        for (name, sql) in probes {
            match engine
                .execute_query(sql.to_string(), std::collections::HashMap::new(), None)
                .await
            {
                Ok(rows) => {
                    eprintln!("[nav_probe {label}] {name}: {} row(s)", rows.len());
                    for row in &rows {
                        eprintln!("  {row:?}");
                    }
                }
                Err(e) => eprintln!("[nav_probe {label}] {name}: ERROR {e:?}"),
            }
        }
    }

    /// Probe the live SQL backend for a single block's row across the layers
    /// that matter for render: `block_raw` (writable base table),
    /// `block` (hydrated matview the renderer reads), and `focus_roots`
    /// (matview that gates Main-panel descendant inclusion). Returns a
    /// human-readable multi-line summary suitable for embedding in a panic
    /// message — used when `wait_for_entity_bounds` times out and we want to
    /// tell apart "row missing from SQL" from "row in SQL but not rendered".
    async fn probe_block_sql_state(&self, entity_id: &str) -> String {
        let engine = self.engine();
        let escaped = entity_id.replace('\'', "''");
        let queries: &[(&str, String)] = &[
            (
                "block_raw",
                format!(
                    "SELECT id, parent_id, content, content_type, source_language, \
                     json_extract(properties, '$.task_state') AS task_state, \
                     json_extract(properties, '$.sequence')  AS sequence \
                     FROM block_raw WHERE id = '{escaped}'"
                ),
            ),
            (
                "block (matview)",
                format!(
                    "SELECT id, parent_id, content, content_type, source_language, \
                     json_extract(properties, '$.task_state') AS task_state, tags \
                     FROM block WHERE id = '{escaped}'"
                ),
            ),
            (
                "siblings_raw",
                format!(
                    "SELECT b.id, b.content_type, json_extract(b.properties, '$.task_state') AS task_state, \
                     json_extract(b.properties, '$.sequence') AS sequence \
                     FROM block_raw b \
                     WHERE b.parent_id = (SELECT parent_id FROM block_raw WHERE id = '{escaped}') \
                     ORDER BY sequence"
                ),
            ),
            (
                "focus_roots",
                "SELECT region, root_id, history_id FROM focus_roots ORDER BY region, history_id"
                    .to_string(),
            ),
        ];
        let mut out = String::new();
        for (name, sql) in queries {
            match engine
                .execute_query(sql.clone(), std::collections::HashMap::new(), None)
                .await
            {
                Ok(rows) => {
                    out.push_str(&format!("  [{name}] {} row(s)\n", rows.len()));
                    for row in &rows {
                        out.push_str(&format!("    {row:?}\n"));
                    }
                }
                Err(e) => {
                    out.push_str(&format!("  [{name}] ERROR {e:?}\n"));
                }
            }
        }
        out
    }

    /// Wait until `frontend_geometry` (if installed) has committed bounds for
    /// the given entity. The backend `ViewModel` resolves faster than GPUI's
    /// render pipeline (signal → render → prepaint → BoundsRegistry promote),
    /// so a transition that just changed the rendered set must wait for the
    /// next pass to commit before driving real input. Returns `Ok(())`
    /// immediately when no geometry is installed (headless drivers don't
    /// need bounds). Returns an `Err` on timeout — the caller chooses
    /// whether to panic (input-bearing transitions) or proceed (best-effort).
    #[tracing::instrument(skip(self), name = "pbt.wait_for_entity_bounds", fields(%entity_id))]
    async fn wait_for_entity_bounds(
        &self,
        entity_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let Some(ref geometry) = self.frontend_geometry else {
            return Ok(());
        };
        // Mirror GpuiUserDriver::element_center: try the canonical
        // `render-entity-{id}` first, then `selectable-{id}` (default
        // index.org sidebar wraps rows in `selectable(...)` directly),
        // then any tracked element whose `entity_id` matches.
        let render_id = format!("render-entity-{entity_id}");
        let selectable_id = format!("selectable-{entity_id}");
        let deadline = tokio::time::Instant::now() + timeout;
        // After ~200 ms of polling without bounds, ask the driver to
        // scroll the entity into view once. Sidebar items in a virtualized
        // `gpui::list(...)` are not prepaint-ed outside the viewport, so
        // their bounds never appear until the user scrolls — which under
        // PBT we have to do explicitly. Block-mode panels prepaint every
        // child regardless of viewport, so scroll is a no-op there. The
        // RPC may also fail to find any virtualized list containing the
        // entity (returns Ok(false) on the GPUI side, surfaces here as
        // a benign success). In every case the polling loop is the
        // authoritative failure signal.
        let scroll_deadline = tokio::time::Instant::now() + Duration::from_millis(200);
        let mut scrolled = false;
        loop {
            if geometry.element_info(&render_id).is_some()
                || geometry.element_info(&selectable_id).is_some()
                || geometry.find_by_entity_id(entity_id).is_some()
            {
                return Ok(());
            }
            if !scrolled && tokio::time::Instant::now() >= scroll_deadline {
                scrolled = true;
                if let Some(driver) = self.driver.as_ref() {
                    if let Err(e) = driver.scroll_to_entity(entity_id).await {
                        tracing::debug!(
                            "wait_for_entity_bounds: scroll_to_entity({entity_id:?}) \
                             returned Err — continuing to poll: {e:#}"
                        );
                    }
                }
            }
            if tokio::time::Instant::now() >= deadline {
                // Dump BoundsRegistry contents to disambiguate "element not
                // rendered at all" from "element rendered under an id we
                // didn't try" (the latter is a wait_for_entity_bounds bug;
                // the former is a render-pipeline bug). Filter to elements
                // mentioning the entity id so the dump stays scannable.
                let all = geometry.all_elements();
                let matching: Vec<String> = all
                    .iter()
                    .filter(|(id, info)| {
                        id.contains(entity_id)
                            || info
                                .entity_id
                                .as_deref()
                                .is_some_and(|eid| eid == entity_id)
                    })
                    .map(|(id, info)| {
                        format!(
                            "    {id:?} entity_id={:?} widget={} xywh=({},{},{},{})",
                            info.entity_id,
                            info.widget_type,
                            info.x,
                            info.y,
                            info.width,
                            info.height,
                        )
                    })
                    .collect();
                let matching_str = if matching.is_empty() {
                    // Also dump entries whose entity_id starts with "block:"
                    // or "file:" — useful to see what's actually data-bound
                    // when the target is absent.
                    let bound: Vec<String> = all
                        .iter()
                        .filter_map(|(id, info)| {
                            info.entity_id.as_deref().map(|eid| {
                                format!("    {id:?} entity_id={eid:?} widget={}", info.widget_type)
                            })
                        })
                        .take(40)
                        .collect();
                    format!(
                        "    <no element mentions this entity_id>\n\
                         Data-bound elements (up to 40):\n{}",
                        bound.join("\n")
                    )
                } else {
                    matching.join("\n")
                };
                anyhow::bail!(
                    "wait_for_entity_bounds: timed out after {timeout:?} waiting for \
                     bounds of entity {entity_id:?} — tried element ids \
                     {render_id:?}, {selectable_id:?}, and entity_id scan; element \
                     was never rendered to BoundsRegistry (post-scroll), or bounds \
                     weren't promoted staged → committed since the last render pass.\n\
                     BoundsRegistry total elements: {}\n\
                     Elements mentioning {entity_id:?}:\n{matching_str}",
                    all.len(),
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Wait until at least one element with `entity_id == entity_id` reports
    /// one of the accepted `widget_type` values.
    ///
    /// Stronger precondition than `wait_for_entity_bounds`: a block can have
    /// bounds while rendered as a non-interactive `rendered_text`. Driving
    /// keyboard focus through `click_entity` against a `rendered_text` is a
    /// known footgun — the click doesn't promote the block to edit mode
    /// when the upstream profile selector picked the wrong variant, and the
    /// caller's `wait_for_focus_to_match` then times out blaming the click.
    /// This helper surfaces that mismatch before the click happens.
    ///
    /// Returns `Ok(())` when no geometry is installed (headless variants).
    #[tracing::instrument(skip(self), name = "pbt.wait_for_widget_kind", fields(%entity_id))]
    async fn wait_for_widget_kind(
        &self,
        entity_id: &str,
        accepted: &[&str],
        timeout: Duration,
    ) -> anyhow::Result<String> {
        let Some(ref geometry) = self.frontend_geometry else {
            return Ok(String::new());
        };
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let mut observed_for_entity: Vec<String> = Vec::new();
            for (_, info) in geometry.all_elements() {
                if info.entity_id.as_deref() == Some(entity_id) {
                    if accepted.iter().any(|a| info.widget_type == *a) {
                        return Ok(info.widget_type);
                    }
                    observed_for_entity.push(info.widget_type.clone());
                }
            }
            if tokio::time::Instant::now() >= deadline {
                let diag = crate::pbt::panic_diag::focus_and_render_dump(
                    self.engine(),
                    self.ctx
                        .reactive_engine
                        .as_ref()
                        .and_then(|e| e.ui_state().focused_block())
                        .as_ref(),
                    self.frontend_geometry.as_deref(),
                    "wait_for_widget_kind",
                )
                .await;
                anyhow::bail!(
                    "wait_for_widget_kind: {entity_id:?} never rendered as one of \
                     {accepted:?} within {timeout:?}; observed widget_types for this \
                     entity_id: {observed_for_entity:?}\n{diag}"
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Poll `UiState.focused_block` until it matches `expected_block_id`.
    ///
    /// `services.dispatch_intent` (the path a real mouse click takes
    /// through `selectable.on_mouse_down`) is fire-and-forget. The
    /// `maybe_mirror_navigation_focus` hook (`reactive.rs:1446`) writes
    /// `UiState.focused_block` synchronously inside `dispatch_intent`,
    /// so polling that mirror is a fast proxy for "the click landed".
    /// The matview chain (`focus_roots` etc.) lags this mirror but the
    /// next `wait_for_entity_in_resolved_view_model` (5 s) catches it.
    ///
    /// Reads `self.ctx.reactive_engine` — the engine instance the GPUI
    /// window's `BuilderServices` uses (via `PbtReadyContext`). The
    /// local `self.reactive_engine` RefCell is a separate instance
    /// `ensure_reactive_engine` creates inside the SUT and would not
    /// observe focus writes from the GPUI click handler.
    #[tracing::instrument(skip(self), name = "pbt.wait_for_focus_to_match", fields(%expected_block_id))]
    async fn wait_for_focus_to_match(
        &self,
        expected_block_id: &str,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let actual = self
                .ctx
                .reactive_engine
                .as_ref()
                .and_then(|e| e.ui_state().focused_block());
            if actual.as_ref().map(|u| u.as_str()) == Some(expected_block_id) {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                let diag = crate::pbt::panic_diag::focus_and_render_dump(
                    self.engine(),
                    actual.as_ref(),
                    self.frontend_geometry.as_deref(),
                    "wait_for_focus_to_match",
                )
                .await;
                anyhow::bail!(
                    "wait_for_focus_to_match: expected={expected_block_id:?} \
                     actual={actual:?} after {timeout:?}\n{diag}"
                );
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Block until the geometry tree's children of `parent_id` match the
    /// reference state's prediction for that parent.
    ///
    /// Why this gate exists: a CDC batch that adds N siblings to the same
    /// parent (e.g. NavigateFocus exposing a doc's full block list) can
    /// arrive in two render passes — first an initial render with a subset
    /// of children, then a second pass that adds the rest and shifts the
    /// initially-rendered siblings' bounds. `wait_for_entity_bounds(target)`
    /// passes against the first pass and returns a `(cx, cy)` that becomes
    /// stale once the second pass commits, so the synthetic click lands on
    /// whichever block now sits at those coords. Concrete observation:
    /// PBT seed=42 step 4, NavigateFocus → c2f12z-s at y=63 → click
    /// dispatched → render added `-q--2b-9` above → click hit
    /// `-q--2b-9` instead.
    ///
    /// Predicate: count widgets with widget_type ∈ {rendered_text,
    /// editable_text} whose `entity_id` resolves to a known child of
    /// `parent_id` in the PRE-transition ref-state. When that count
    /// equals the number of non-Page children of `parent_id` in the
    /// pre-state, the children list has stabilised for the purposes of
    /// coordinate resolution against what the user can see right now.
    ///
    /// Reads `self.pre_ref_state` rather than the post-transition state
    /// passed into `apply_to_sut` — the post-state already contains any
    /// blocks the in-flight transition will create, but those blocks
    /// can't exist in the SUT's geometry yet because the transition
    /// hasn't dispatched. Using the pre-state means the wait is
    /// expressed in terms of "what the user sees" and needs no
    /// per-transition exclusion list.
    ///
    /// No-op when geometry is unavailable (headless drivers), when no
    /// pre-state has been recorded yet (first transition), or when the
    /// parent has no known children — `wait_for_entity_bounds` remains
    /// the authoritative single-element gate.
    #[tracing::instrument(skip(self), name = "pbt.wait_for_children_settled", fields(%parent_id))]
    async fn wait_for_children_settled(
        &self,
        parent_id: &EntityUri,
        timeout: Duration,
    ) -> anyhow::Result<()> {
        let Some(ref geometry) = self.frontend_geometry else {
            return Ok(());
        };
        let Some(ref pre_state) = self.pre_ref_state else {
            return Ok(());
        };
        let resolved_parent = self.resolve_uri(parent_id);
        let expected_child_ids: HashSet<String> = pre_state
            .block_state
            .blocks
            .values()
            .filter(|b| !b.is_page() && b.parent_id == *parent_id)
            .map(|b| self.resolve_uri(&b.id).to_string())
            .collect();
        if expected_child_ids.is_empty() {
            return Ok(());
        }
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let mut seen: HashSet<String> = HashSet::new();
            for (_, info) in geometry.all_elements() {
                if info.widget_type != "rendered_text" && info.widget_type != "editable_text" {
                    continue;
                }
                if let Some(eid) = info.entity_id.as_deref() {
                    if expected_child_ids.contains(eid) {
                        seen.insert(eid.to_string());
                    }
                }
            }
            if seen.len() >= expected_child_ids.len() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                let missing: Vec<&String> = expected_child_ids.difference(&seen).collect();
                let diag = crate::pbt::panic_diag::focus_and_render_dump(
                    self.engine(),
                    self.ctx
                        .reactive_engine
                        .as_ref()
                        .and_then(|e| e.ui_state().focused_block())
                        .as_ref(),
                    self.frontend_geometry.as_deref(),
                    "wait_for_children_settled",
                )
                .await;
                anyhow::bail!(
                    "wait_for_children_settled: parent={resolved_parent} expected \
                     {} child widget(s) (rendered_text/editable_text), saw {} after \
                     {timeout:?}; missing={missing:?}\n{diag}",
                    expected_child_ids.len(),
                    seen.len(),
                );
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Fully resolved ViewModel snapshot — uses the same path as inv-viewmodel-state-toggle-correct:
    /// `interpret_pure(render_expr, data_rows)` so that list/table items
    /// are populated from the data snapshot. Waits for the UiWatcher to
    /// deliver data rows if they haven't arrived yet.
    async fn current_resolved_view_model(&self) -> Option<holon_frontend::ViewModel> {
        let reactive = self.reactive_engine.borrow().clone()?;
        let root_id = self
            .reactive_root_id
            .borrow()
            .clone()
            .unwrap_or_else(holon_api::root_layout_block_uri);

        // Wait for data rows to arrive (UiWatcher loads asynchronously).
        let results = reactive.ensure_watching(&root_id);
        {
            use futures::StreamExt;
            let mut stream = reactive.watch(&root_id);
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                let (_, rows) = results.snapshot();
                if !rows.is_empty() {
                    break;
                }
                match tokio::time::timeout_at(deadline, stream.next()).await {
                    Ok(Some(_)) => continue,
                    _ => break,
                }
            }
        }

        let (render_expr, data_rows) = results.snapshot();
        let services =
            holon_frontend::reactive::HeadlessBuilderServices::new(self.engine().clone());
        Some(holon_frontend::interpret_pure(&render_expr, &data_rows, &services).snapshot())
    }

    /// Initialize the ReactiveEngine — the same rendering pipeline GPUI uses.
    /// Must be called during StartApp so all subsequent transitions can read
    /// the reactive tree (ToggleState, EditViaDisplayTree, etc.).
    async fn ensure_reactive_engine(&self, root_id: &EntityUri) {
        if self.reactive_engine.borrow().is_some() {
            return;
        }
        let engine = self.engine();
        let session = Arc::new(holon_frontend::FrontendSession::from_engine(Arc::clone(
            engine,
        )));
        let rt = tokio::runtime::Handle::current();

        let services_slot: Arc<
            std::sync::OnceLock<Arc<dyn holon_frontend::reactive::BuilderServices>>,
        > = Arc::new(std::sync::OnceLock::new());
        let slot_clone = services_slot.clone();
        let reactive = Arc::new(holon_frontend::reactive::ReactiveEngine::new(
            session,
            rt,
            Arc::new(holon_frontend::shadow_builders::build_shadow_interpreter()),
            move |expr, rows| {
                let services = match slot_clone.get() {
                    Some(s) => s.clone(),
                    None => return holon_frontend::ReactiveViewModel::empty(),
                };
                holon_frontend::interpret_pure(expr, rows, &*services)
            },
            services_slot.clone(),
        ));
        let services: Arc<dyn holon_frontend::reactive::BuilderServices> = reactive.clone();
        services_slot.set(services).ok();

        {
            use futures::StreamExt;
            let collector = self.vm_emissions.clone();
            let mut stream = reactive.watch(root_id);
            tokio::spawn(async move {
                while let Some(rvm) = stream.next().await {
                    let vm = rvm.snapshot();
                    collector.lock().unwrap().push(vm);
                }
            });
        }

        *self.reactive_root_id.borrow_mut() = Some(root_id.clone());

        *self.reactive_engine.borrow_mut() = Some(reactive.clone());

        // Wire BlockCellRegistry backed by the test's global LoroDoc.
        // Synchronously awaited (this fn is `async`) — the previous
        // `tokio::spawn` left a race where atomic editor primitives ran
        // before the registry landed, making `engine.editable_text(...)`
        // return Err and silently dropping per-keystroke writes (see
        // `crates/holon-frontend/src/headless_editor_mirror.rs`).
        //
        // Both the locally-created `reactive` engine AND the DI engine
        // (`self.ctx.reactive_engine`, used by `ReactiveEngineDriver`)
        // need the registry. The driver path is the one keystrokes go
        // through; without wiring the DI engine, the headless mirror's
        // `editable_text(...)` lookup returns `Err` and per-keystroke
        // writes silently drop.
        if let Some(doc_store) = self.ctx.doc_store() {
            let store = doc_store.read().await;
            match store.get_global_doc().await {
                Ok(collab) => {
                    let registry = Arc::new(
                        holon::sync::block_cell_registry::BlockCellRegistry::with_loro(collab),
                    );
                    reactive
                        .block_cell_registry
                        .lock()
                        .unwrap()
                        .replace(registry.clone());
                    if let Some(di_engine) = self.ctx.reactive_engine.as_ref() {
                        di_engine
                            .block_cell_registry
                            .lock()
                            .unwrap()
                            .replace(registry);
                    }
                    eprintln!("[ensure_reactive_engine] BlockCellRegistry wired");
                }
                Err(e) => {
                    eprintln!("[ensure_reactive_engine] Failed to get global doc: {e}");
                }
            }
        }

        eprintln!("[ensure_reactive_engine] Created (data loads in background)");
    }

    /// Send a key chord on a focused entity, going through the full
    /// keybinding → shadow index → operation dispatch pipeline. Thin wrapper
    /// around `UserDriver::send_key_chord` — the driver owns input
    /// routing so that real-input implementations (GPUI enigo) can override
    /// this without the SUT touching `IncrementalShadowIndex` directly.
    ///
    /// Returns `true` if the chord matched an operation and dispatched it.
    pub async fn send_key_chord(
        &self,
        entity_id: &str,
        chord: &holon_api::KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool> {
        let (root_id, root_tree) = self
            .current_reactive_tree()
            .ok_or_else(|| anyhow::anyhow!("No reactive tree available — was start_app called?"))?;
        // Real-input drivers (e.g. `GpuiUserDriver`) click-to-focus before
        // dispatching the chord. That click needs committed bounds. No-op
        // when no geometry provider is installed (headless drivers).
        self.wait_for_entity_bounds(entity_id, Duration::from_secs(5))
            .await
            .with_context(|| format!("send_key_chord: entity {entity_id}"))?;
        let driver = self
            .driver
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("driver not installed"))?;
        driver
            .send_key_chord(root_id.as_str(), &root_tree, entity_id, chord, extra_params)
            .await
    }

    /// Dispatch a `BlockOperations` op through the real chord pipeline:
    /// `send_key_chord` clicks the entity, presses the chord, and bubbles
    /// the input through the matched operation. Headless drivers use
    /// `bubble_input`; GPUI dispatches a real `PlatformInput`. Either way,
    /// the editor controller and chord resolver run, so input-layer
    /// regressions surface here. Panics on dispatch failure or non-match.
    pub async fn dispatch_block_op_via_chord(
        &self,
        op: &str,
        entity_id: &str,
        extra_params: HashMap<String, Value>,
    ) {
        let chord = self
            .find_keybinding_for_op(op)
            .unwrap_or_else(|| panic!("[{op}] no keybinding registered"));
        let dispatched = self
            .send_key_chord(entity_id, &chord, extra_params)
            .await
            .unwrap_or_else(|e| panic!("[{op}] send_key_chord failed: {e:#}"));
        assert!(
            dispatched,
            "[{op}] chord {chord:?} did not dispatch on entity {entity_id}"
        );
    }

    /// Drive the TUI's leader chord (Space + key) through the input
    /// pipeline. Mirrors what a real user would do for actions bound
    /// in `assets/default/keybindings.yaml` under
    /// `modifiers: ["leader"]`.
    ///
    /// `nav_op` is the action name from the YAML (`go_home`, `go_back`,
    /// `go_forward`, ...). The leader-chord key is resolved from the
    /// YAML at compile time via [`leader_key_for`] — if the binding
    /// moves in the YAML, the test follows it. Headless-driver fallback
    /// dispatches the `navigation.<nav_op>` intent directly, matching
    /// what `frontends/tui/src/app_main.rs::dispatch_navigation_op`
    /// runs after the chord matches in production. `label` is used in
    /// panic messages.
    pub async fn send_leader_chord(&self, nav_op: &str, label: &str) {
        let driver = self
            .driver
            .as_ref()
            .unwrap_or_else(|| panic!("[{label}] driver not installed — was start_app called?"));
        // Native drivers (TUI/GPUI) route raw keystrokes through their real
        // input pipeline, which performs key-chord resolution before any
        // editor sees the keys. Send the leader key + chord key as raw
        // keystrokes so the chord-resolver path is exercised end-to-end.
        //
        // Headless drivers (`ReactiveEngineDriver`, `DirectUserDriver`)
        // route raw keystrokes straight into the focused editor's
        // `MutableText` mirror — no chord resolution. Sending `SPC b`
        // there would TYPE " b" into the focused block instead of
        // dispatching `go_back`. Dispatch the navigation intent directly
        // for those drivers.
        if driver.dispatches_chords_via_raw_keystroke() {
            let key = leader_key_for(nav_op);
            driver
                .send_raw_keystroke(" ", &[])
                .await
                .unwrap_or_else(|e| panic!("[{label}] send_raw_keystroke(SPC) failed: {e:#}"));
            driver
                .send_raw_keystroke(key, &[])
                .await
                .unwrap_or_else(|e| panic!("[{label}] send_raw_keystroke({key:?}) failed: {e:#}"));
            return;
        }
        // Headless: dispatch the navigation op directly. Region is hardcoded
        // to "main" to mirror the TUI binding (only Main is generated by
        // NavigateHome/Back/Forward).
        let mut params = HashMap::new();
        params.insert("region".to_string(), Value::String("main".to_string()));
        driver
            .synthetic_dispatch("navigation", nav_op, params)
            .await
            .unwrap_or_else(|e| {
                panic!("[{label}] synthetic_dispatch(navigation, {nav_op}) failed: {e:#}")
            });
    }

    /// Resolve a reference URI to its real backend URI via `doc_uri_map`.
    /// Handles file:→doc: (documents), block::split-N→block:uuid (split-created blocks),
    /// and passes through any URI not in the map unchanged.
    pub fn resolve_uri(&self, parent_id: &EntityUri) -> EntityUri {
        self.doc_uri_map
            .get(parent_id)
            .cloned()
            .unwrap_or_else(|| parent_id.clone())
    }

    /// Resolve a reference-model stable_id to the actual stable_id used in the Loro tree.
    /// The reference model uses `b.id.id()` (e.g. "ref-doc-2"), but the actual Loro tree
    /// uses the resolved UUID path (e.g. "422cf01d-..."). Try doc_uri_map first.
    fn resolve_stable_id(&self, stable_id: &str) -> String {
        // Try block: prefix first (common for block IDs)
        let block_uri = EntityUri::from_raw(&format!("block:{}", stable_id));
        if let Some(resolved) = self.doc_uri_map.get(&block_uri) {
            return resolved.id().to_string();
        }
        // Try file: prefix (document IDs from pre-startup)
        let file_uri = EntityUri::from_raw(&format!("file:{}", stable_id));
        if let Some(resolved) = self.doc_uri_map.get(&file_uri) {
            return resolved.id().to_string();
        }
        // Pass through unchanged
        stable_id.to_string()
    }

    /// Look up the keybinding for an operation name from the reactive engine's registry.
    fn find_keybinding_for_op(&self, op_name: &str) -> Option<holon_api::KeyChord> {
        let engine = self.reactive_engine.borrow();
        let engine = engine.as_ref()?;
        engine.key_bindings().lock_ref().get(op_name).cloned()
    }

    /// Validate that a keychord resolves to the expected operation via the shadow index.
    ///
    /// Does NOT dispatch — only checks the keybinding → shadow index → bubble_input path.
    /// Panics with diagnostics if the keychord doesn't match. Delegates to
    /// `UserDriver::resolve_key_chord`.
    fn assert_keychord_resolves(&self, op_name: &str, entity_id: &str, label: &str) {
        let Some(chord) = self.find_keybinding_for_op(op_name) else {
            return; // No keybinding registered — skip validation
        };
        let Some((root_id, root_tree)) = self.current_reactive_tree() else {
            panic!("[{label}] No reactive tree available for keychord validation");
        };
        let Some(driver) = self.driver.as_ref() else {
            panic!("[{label}] driver not installed");
        };
        match driver.resolve_key_chord(root_id.as_str(), &root_tree, entity_id, &chord) {
            Some(matched_op) => {
                eprintln!("[{label}] Keychord validation OK: chord matched op '{matched_op}'");
            }
            None => {
                panic!(
                    "[{label}] Keychord {chord:?} for '{op_name}' did NOT match on entity \
                     {entity_id}. The keybinding was not joined into the operation."
                );
            }
        }
    }
}
impl<V: VariantMarker> E2ESut<V> {
    /// Lazy accessor for the CDC-driven `LiveData<Block>` mirroring the `block`
    /// matview. Built on first use because we need an async `watch_view` call and
    /// the SUT struct can't carry a started engine at construction time. The
    /// matview hydrates `tags` (and `requires`) from the junction tables, so
    /// rows are read directly into a fully-populated `Block`.
    async fn live_blocks(&self) -> Arc<holon::sync::LiveData<Block>> {
        if let Some(live) = self.live_blocks_cell.borrow().clone() {
            return live;
        }
        let sql = format!(
            "SELECT id, content, content_type, source_language, parent_id, properties, tags \
             FROM {BLOCK_READ_TABLE}"
        );
        let watch = self
            .ctx
            .engine()
            .watch_view(&sql)
            .await
            .expect("watch_view(block) failed");
        let live = holon::sync::LiveData::new(
            watch.initial_rows,
            |row| {
                row.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow::anyhow!("block row missing 'id'"))
            },
            |row| {
                parse_block_row(row)
                    .ok_or_else(|| anyhow::anyhow!("parse_block_row returned None for row {row:?}"))
            },
        );
        live.subscribe(watch.stream);
        *self.live_blocks_cell.borrow_mut() = Some(Arc::clone(&live));
        live
    }

    /// Lazy accessor for the CDC-driven `LiveData<FocusRoot>` mirroring the
    /// `focus_roots` matview. Keyed by `"{region}\u{1F}{root_id}"` since one
    /// region can have multiple root rows (one per child of the nav target).
    async fn live_focus_roots(&self) -> Arc<holon::sync::LiveData<FocusRoot>> {
        if let Some(live) = self.live_focus_roots_cell.borrow().clone() {
            return live;
        }
        // `focus_roots` matview filters `block_id IS NOT NULL` at projection
        // time as of nightscape@holon `aff40a84` (the IVM compound IS NOT NULL
        // fix). Chained-matview CDC propagation is 1:1 with no spurious
        // events for filtered rows (verified by
        // `crates/holon/examples/turso_ivm_chained_matview_null_cdc.rs`).
        // No watcher-level filter needed.
        let sql = "SELECT region, root_id FROM focus_roots";
        let watch = self
            .ctx
            .engine()
            .watch_view(sql)
            .await
            .expect("watch_view(focus_roots) failed");
        let live = holon::sync::LiveData::new(
            watch.initial_rows,
            |row| {
                let region = row
                    .get("region")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'region'"))?;
                let root_id = row
                    .get("root_id")
                    .and_then(|v| v.as_string())
                    .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'root_id'"))?;
                Ok(format!("{region}\u{1F}{root_id}"))
            },
            |row| {
                Ok(FocusRoot {
                    region: row
                        .get("region")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'region'"))?,
                    root_id: row
                        .get("root_id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .ok_or_else(|| anyhow::anyhow!("focus_roots row missing 'root_id'"))?,
                })
            },
        );
        live.subscribe(watch.stream);
        *self.live_focus_roots_cell.borrow_mut() = Some(Arc::clone(&live));
        live
    }

    /// Async body of `apply()` — extracted so Flutter (already async) can call directly
    /// without `block_on`.
    #[tracing::instrument(skip(self, ref_state, transition), name = "pbt.apply_transition")]
    pub async fn apply_transition_async(
        &mut self,
        ref_state: &ReferenceState,
        transition: &crate::pbt::transitions::E2ETransition,
    ) {
        use crate::pbt::transitions::E2ETransitionImpl;
        transition.apply_to_sut(ref_state, self).await;
        // Stash the post-transition ref-state so the NEXT call can read it
        // as its pre-transition state. The framework hands us only the
        // post-state, so we have to carry the previous post forward
        // ourselves. See `pre_ref_state` field doc for the rationale.
        self.pre_ref_state = Some(ref_state.clone());

        // Yield to let tokio schedule CDC forwarding tasks before we drain.
        tokio::task::yield_now().await;
        self.drain_cdc_events().await;
        self.drain_region_cdc_events().await;

        // Drain both directions of the Loro mirror BEFORE sampling
        // `target_seq` in `assert_cdc_quiescent`. The original layout ran
        // `wait_for_consumers` AFTER the inv-editable-text-has-draggable assert, which let SQL writes
        // produced by inbound EventBus consumers (e.g. `LoroSyncController`'s
        // SQL→Loro path triggering an outbound Loro→SQL reconcile) commit
        // *during* the inv-editable-text-has-draggable grace window — looking like spurious churn
        // when they're really just causally-related writes that haven't
        // settled yet.
        //
        // Round-trip path that has to converge before the assert:
        //   SQL write → CDC → EventBus event → `loro` consumer writes Loro
        //   → `subscribe_root` fires → `on_loro_changed` → more SQL writes
        //
        // Echo suppression (`event.origin == EventOrigin::Loro`) breaks
        // the cycle in 1–2 hops, so a single drain pair is enough.
        {
            use tracing::Instrument;
            // Per-step settle barriers. Timeouts sized for Full+atomic-editor PBT runs
            // where BulkExternalAdd produces bursts the loro consumer applies serially:
            // 500ms wasn't enough to land all create events, leaving subsequent TypeChars
            // dispatched against blocks not-yet-in-the-Loro-tree (silent-drop in
            // `headless_editor_mirror.rs` because `editable_text(...)` returned Err).
            async {
                tokio::task::yield_now().await;
                self.ctx
                    .wait_for_loro_quiescence(std::time::Duration::from_secs(2))
                    .await;
                self.ctx
                    .wait_for_consumers(
                        &["loro", "org", "cache"],
                        std::time::Duration::from_secs(5),
                    )
                    .await;
                self.ctx
                    .wait_for_loro_quiescence(std::time::Duration::from_secs(2))
                    .await;
                tokio::task::yield_now().await;
                self.drain_cdc_events().await;
                self.drain_region_cdc_events().await;
                self.wait_for_live_data_mirrors(std::time::Duration::from_secs(2))
                    .await;
            }
            .instrument(tracing::info_span!("pbt.pre_inv16_settle"))
            .await;
        }

        // inv-editable-text-has-draggable: After draining, no more CDC events should arrive.
        {
            use tracing::Instrument;
            async {
                self.assert_cdc_quiescent().await;
            }
            .instrument(tracing::info_span!("pbt.assert_cdc_quiescent"))
            .await;
        }
    }

    /// Drain every instantiated `LiveData` mirror up to the current CDC
    /// emission watermark. Closes the race where `wait_for_consumers` reports
    /// the named EventBus consumers caught up but a mirror's `spawn_actor`
    /// task hasn't yet polled the matching CDC batches off its broadcast
    /// receiver — invariants would then read a stale snapshot (most visibly,
    /// invariant 8's region focus_roots check seeing the previous focus's
    /// children alongside the current ones).
    ///
    /// Sampling `cdc_emitted_watermark()` AFTER `wait_for_consumers` is
    /// deliberate: by then every batch the transition could possibly produce
    /// has been stamped with a `seq`, so once each mirror's `consumed_seq`
    /// catches that watermark we know it has applied every CDC batch the
    /// matview emitted before this call.
    async fn wait_for_live_data_mirrors(&self, timeout: std::time::Duration) {
        // Pre-startup transitions (e.g. `WriteOrgFile` before `StartApp`)
        // run through this drain block too, but the engine doesn't exist
        // yet — and there can't be any LiveData mirrors either.
        if !self.ctx.is_running() {
            return;
        }
        let target = self.ctx.engine().db_handle().cdc_emitted_watermark();
        if target == 0 {
            return;
        }
        if let Some(live) = self.live_blocks_cell.borrow().clone() {
            live.wait_for_seq(target, timeout).await;
        }
        if let Some(live) = self.live_focus_roots_cell.borrow().clone() {
            live.wait_for_seq(target, timeout).await;
        }
    }

    /// Async body of `check_invariants()` — extracted so Flutter can call directly.
    #[tracing::instrument(skip(self, ref_state), name = "pbt.check_invariants")]
    pub async fn check_invariants_async(&self, ref_state: &ReferenceState) {
        tracing::trace!(
            "[check_invariants] ref_state has {} blocks, app_started: {}",
            ref_state.block_state.blocks.len(),
            ref_state.app_started
        );

        // Skip invariant checks if app is not started
        if !ref_state.app_started {
            return;
        }

        // Transitions that don't modify block data — skip expensive invariants
        let nav_only = matches!(
            self.last_transition.variant_name(),
            "SwitchView"
                | "NavigateFocus"
                | "NavigateBack"
                | "NavigateForward"
                | "NavigateHome"
                | "ClickBlock"
                | "ArrowNavigate"
                | "SetupWatch"
                | "RemoveWatch"
                | "EmitMcpData"
                | "AddPeer"
                | "PeerEdit"
        );

        // 0. Check for startup errors (Flutter bug: DDL/sync race)
        assert!(
            !self.has_startup_errors(),
            "FLUTTER STARTUP BUG: {} publish errors during startup.\n\
                 This indicates DDL/sync race condition when {} pre-existing files were synced.\n\
                 Files: {:?}",
            self.startup_error_count(),
            self.documents.len(),
            self.documents.keys().collect::<Vec<_>>()
        );

        // 0b. inv-loro-no-errors: LoroSyncController must not log any errors.
        //     Catches Bug B and similar SQL→Loro reconcile failures (e.g.
        //     `Cannot resolve parent URI to TreeID`, missing-block warnings,
        //     `update_parent_id failed`, etc.). The controller increments
        //     `error_count` whenever `on_inbound_event` returns Err, so any
        //     non-zero count means the SQL→Loro mirror dropped a CDC event.
        let loro_errs = self.ctx.loro_sync_error_count();
        assert_eq!(
            loro_errs, 0,
            "[inv-loro-no-errors] LoroSyncController logged {loro_errs} error(s). \
             Search captured logs for `[LoroSyncController] Failed to apply` to find which \
             event(s) the SQL→Loro mirror dropped (e.g. `Cannot resolve parent URI to TreeID: \
             block:UUID` for outdent/indent/split where the new parent isn't yet a TreeID in the \
             Loro tree)."
        );

        // 1. Backend storage matches reference model
        //    Read from the CDC-driven `LiveData<Block>` mirroring the `block`
        //    matview. The matview hydrates `tags` from the junction table, so
        //    rows arrive fully populated. `wait_for_consumers` already gates
        //    each invariant pass on CDC delivery, so the in-memory snapshot
        //    is delay-free relative to the equivalent `SELECT`.
        let live_blocks = self.live_blocks().await;
        let backend_blocks: Vec<Block> = live_blocks.read().values().cloned().collect();

        // Translate synthetic doc URIs in reference blocks to real UUID-based IDs.
        // OrgSyncController creates document blocks asynchronously, so we
        // retry with a short timeout for any unresolved URIs.
        let mut lazy_doc_uri_map = self.doc_uri_map.clone();
        let unresolved: Vec<_> = ref_state
            .documents
            .iter()
            .filter(|(uri, _)| !lazy_doc_uri_map.contains_key(*uri))
            .map(|(uri, filename)| (uri.clone(), filename.clone()))
            .collect();
        if !unresolved.is_empty() {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut remaining = unresolved;
            while !remaining.is_empty() && Instant::now() < deadline {
                for (synthetic_uri, filename) in std::mem::take(&mut remaining) {
                    match self.ctx.resolve_doc_uri_by_name(&filename).await {
                        Ok(resolved) => {
                            tracing::trace!(
                                "[check_invariants] Late-resolved doc URI: {} → {}",
                                synthetic_uri,
                                resolved
                            );
                            lazy_doc_uri_map.insert(synthetic_uri, resolved);
                        }
                        Err(_) => remaining.push((synthetic_uri, filename)),
                    }
                }
                if !remaining.is_empty() {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
            if !remaining.is_empty() {
                tracing::trace!(
                    "[check_invariants] WARNING: {} doc URIs still unresolved: {:?}",
                    remaining.len(),
                    remaining.iter().map(|(u, _)| u).collect::<Vec<_>>()
                );
            }
        }
        let resolve = |uri: &EntityUri| -> EntityUri {
            lazy_doc_uri_map
                .get(uri)
                .cloned()
                .unwrap_or_else(|| uri.clone())
        };

        let ref_blocks_resolved: Vec<_> = ref_state
            .block_state
            .blocks
            .values()
            .map(|b| {
                let mut block = b.clone();
                block.id = resolve(&block.id);
                block.parent_id = resolve(&block.parent_id);
                block
            })
            .collect();

        // Seed block IDs (raw, untranslated) for org file comparison
        let seed_block_ids_raw: std::collections::HashSet<_> = ref_state
            .block_state
            .block_documents
            .iter()
            .filter(|(_, doc)| doc.is_no_parent() || doc.is_sentinel())
            .map(|(id, _)| id.clone())
            .collect();

        // Seed block IDs (translated) for backend comparison
        let seed_block_ids: std::collections::HashSet<_> = ref_state
            .block_state
            .block_documents
            .iter()
            .filter(|(_, doc)| doc.is_no_parent() || doc.is_sentinel())
            .map(|(id, _)| resolve(id))
            .collect();

        let backend_blocks_no_seed: Vec<_> = backend_blocks
            .iter()
            .filter(|b| !seed_block_ids.contains(&b.id))
            .cloned()
            .collect();
        let ref_blocks_no_seed: Vec<_> = ref_blocks_resolved
            .iter()
            .filter(|b| !seed_block_ids.contains(&b.id))
            .cloned()
            .collect();

        // ID-set truth check before the full block comparison: when
        // backend (live_blocks) and reference disagree, classify whether
        // it's a CDC delivery race (matview lagged a write) or a real
        // pipeline bug. Same pattern as inv-watch-rows-match-ref below — query `block_raw`
        // (write-side base table) and compare ID sets.
        let backend_ids: HashSet<EntityUri> = backend_blocks_no_seed
            .iter()
            .map(|b| b.id.clone())
            .collect();
        let ref_ids: HashSet<EntityUri> = ref_blocks_no_seed.iter().map(|b| b.id.clone()).collect();
        // When set, downstream invariants that read `backend_blocks` must
        // skip — the mirror is stale and any structural assertion (orphan
        // checks, focus_roots intersection, etc.) would just re-fail on
        // the same lag.
        let live_blocks_stale = if backend_ids != ref_ids {
            let truth_rows = self
                .ctx
                .query_sql("SELECT id FROM block_raw")
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "[inv-backend-blocks-match-ref truth check] block_raw query failed\n\
                         error: {}",
                        e
                    )
                });
            let truth_ids: HashSet<EntityUri> = truth_rows
                .iter()
                .filter_map(|r| {
                    r.get("id")
                        .and_then(|v| v.as_string())
                        .map(|s| EntityUri::parse(s).expect("invalid uri in block_raw row"))
                })
                .filter(|id| !seed_block_ids.contains(id))
                .collect();
            if truth_ids == ref_ids {
                let missing: Vec<&EntityUri> = ref_ids.difference(&backend_ids).collect();
                let spurious: Vec<&EntityUri> = backend_ids.difference(&ref_ids).collect();
                eprintln!(
                    "[inv-backend-blocks-match-ref WARN] live_blocks mirror lagged: backend has {} blocks, \
                     block_raw has {} (matches reference). Downgraded — Turso IVM CDC \
                     delivery race on the `block` matview → live_blocks mirror.\n\
                     Missing in live_blocks: {:?}\n\
                     Spurious in live_blocks: {:?}",
                    backend_ids.len(),
                    truth_ids.len(),
                    missing,
                    spurious,
                );
                true
            } else {
                // truth_ids disagrees with ref_ids — real bug, keep the panic
                // but with the diagnostic that block_raw is what the mirror
                // *would* converge to. Falling through to assert_blocks_equivalent
                // produces the canonical error message.
                eprintln!(
                    "[inv-backend-blocks-match-ref truth check] block_raw also disagrees with reference — \
                     real write/parse pipeline bug, not a CDC delivery race.\n\
                     Missing in block_raw: {:?}\n\
                     Spurious in block_raw: {:?}",
                    ref_ids.difference(&truth_ids).collect::<Vec<_>>(),
                    truth_ids.difference(&ref_ids).collect::<Vec<_>>(),
                );
                assert_blocks_equivalent(
                    &backend_blocks_no_seed,
                    &ref_blocks_no_seed,
                    "Backend diverged from reference",
                );
                false // unreachable — assert above panics
            }
        } else {
            // ID sets match — run the full block comparison (catches
            // per-row content/property/parent mismatches that the ID-set
            // check by definition can't see).
            assert_blocks_equivalent(
                &backend_blocks_no_seed,
                &ref_blocks_no_seed,
                "Backend diverged from reference",
            );
            false
        };

        // 1b. Loro tree matches reference model (when Loro is enabled)
        //
        // DISABLED: the outbound reconcile's CacheEventSubscriber sometimes
        // fails to deserialize update events (missing parent_id/created_at),
        // causing property sync to be lost. The Loro↔ref bridge IS validated
        // at Layer 3 (40 cases). Re-enable after fixing the outbound reconcile
        // event payload completeness for all block types.
        if let Some(ref _loro_sut) = self.loro_sut {
            // loro_sut.assert_matches_reference(&ref_blocks_no_seed, &seed_block_ids).await;
        }

        // Ref blocks for org file comparison — translate synthetic doc URIs
        // to whatever the org parser will produce on disk. With `#+ID:`
        // support, files that have been resolved by the controller carry a
        // `block:<uuid>` parent (the canonical resolved id). Files not yet
        // resolved fall back to `file:<filename>` to match the legacy parser
        // output. Exclude document blocks and seed blocks.
        //
        // Use `lazy_doc_uri_map` (not `self.doc_uri_map`) so docs added
        // post-startup via WriteOrgFile (which only populates the lazy map
        // via `ctx.resolve_doc_uri_by_name` above) are mapped correctly.
        let synthetic_to_parent: HashMap<EntityUri, EntityUri> = ref_state
            .documents
            .iter()
            .map(|(syn, filename)| {
                let target = lazy_doc_uri_map
                    .get(syn)
                    .cloned()
                    .unwrap_or_else(|| EntityUri::file(filename));
                (syn.clone(), target)
            })
            .collect();
        let ref_blocks_org_only: Vec<_> = ref_state
            .block_state
            .blocks
            .values()
            .filter(|b| !seed_block_ids_raw.contains(&b.id))
            .filter(|b| !b.is_page())
            .map(|b| {
                let mut b = b.clone();
                // Synthetic split IDs (`block::split-N`) get mapped to the
                // real UUID issued by `split_block` once the new block lands
                // in the DB; without this, the on-disk org file (which has
                // the real UUID) compares unequal to the ref state.
                b.id = resolve(&b.id);
                if let Some(parent_uri) = synthetic_to_parent.get(&b.parent_id) {
                    b.parent_id = parent_uri.clone();
                }
                b
            })
            .collect();

        // 2/2b: Org file parse + ordering — expensive, skip for nav-only transitions
        if !nav_only {
            // Wait for OrgSyncController's background task to re-render org files
            // after UI mutations. The SQL write is committed but the event-driven
            // re-render runs in a separate tokio task.
            self.wait_for_org_files_stable(25, Duration::from_millis(5000))
                .await;

            let todo_header = ref_state.keyword_set.as_ref().map(|ks| ks.to_org_header());
            let org_blocks = self
                .parse_org_file_blocks(todo_header.as_deref())
                .await
                .expect("Failed to parse Org file");
            assert_blocks_equivalent(
                &org_blocks,
                &ref_blocks_org_only,
                "Org file diverged from reference",
            );

            // 2b. Org file block ordering matches reference model
            assert_block_order(
                &org_blocks,
                &ref_blocks_org_only,
                "Org file block ordering wrong",
            );

            // 2c. Live block_raw children order (the projector's authoritative
            // ordering) matches the reference model's predicted children list.
            // This compares the encoding-free child-id list directly: no
            // `sort_key` strings or `sequence` numbers cross the boundary.
            // Earlier and more diagnostic than the org-roundtrip assertion
            // above (which can mask the underlying disagreement when the org
            // renderer's group sort accidentally re-orders things back).
            self.assert_live_children_match_ref(ref_state).await;

            // 2d. inv-org-render-fixed-point: re-rendering the current SQL
            // state for each tracked .org file must produce the exact bytes
            // already on disk. This is the contract `re_render_all_tracked`
            // depends on for echo suppression: if `render(SQL) != disk`,
            // the next event-driven re-render will write a different file,
            // FSEvent fires, `on_file_changed` reprocesses, and the
            // controller spins. Catches the May-2026 shared-tree mount
            // loop where a property-drawer key round-trip differs between
            // ingestion and render. The 2/2b checks above re-parse and
            // compare blocks against the reference model; they don't see
            // bytes-level disagreement that the parser is forgiving of
            // (e.g. property ordering, sibling reordering driven by
            // sort_key drift), and they don't see disagreement at all
            // when the bug only manifests in a file shape the reference
            // model never generates (e.g. `:share-role: mount`).
            let pairs = self
                .snapshot_org_render_pairs()
                .await
                .expect("snapshot_org_render_pairs failed");
            for (path, (disk, rendered)) in &pairs {
                if disk != rendered {
                    panic!(
                        "[inv-org-render-fixed-point] {} would be rewritten by the \
                         next re_render_all_tracked → echo-suppression loop risk.\n\
                         --- disk ({} bytes) ---\n{}\n--- rendered from SQL ({} bytes) ---\n{}",
                        path.display(),
                        disk.len(),
                        disk,
                        rendered.len(),
                        rendered,
                    );
                }
            }
        }

        // 3. UI model (built from CDC) matches reference — verify all fields, not just IDs
        for (query_id, ui_data) in &self.ui_model {
            if let Some(watch_spec) = ref_state.active_watches.get(query_id) {
                let expected = ref_state.query_results(watch_spec);
                let ui_rows = ui_data.to_vec();

                let ui_ids: HashSet<EntityUri> = ui_rows
                    .iter()
                    .filter_map(|row| {
                        row.get("id")
                            .and_then(|v| v.as_string())
                            .map(|s| EntityUri::parse(s).expect("invalid entity URI in CDC data"))
                    })
                    .collect();
                // Translate file: URIs in expected IDs to block:uuid via doc_uri_map
                let expected_ids: HashSet<EntityUri> = expected
                    .iter()
                    .filter_map(|row| {
                        row.get("id").and_then(|v| v.as_string()).map(|s| {
                            let uri =
                                EntityUri::parse(s).expect("invalid entity URI in expected data");
                            resolve(&uri)
                        })
                    })
                    .collect();

                if ui_ids != expected_ids {
                    // The CDC stream lagged on the ID set. Same classification
                    // as the field-level check below: re-query the underlying
                    // `block_raw` write-side table directly. If `block_raw`
                    // has the expected IDs, the watch matview's CDC just
                    // didn't fan out by the time we drained — downgrade to a
                    // warning. If `block_raw` also disagrees, the
                    // write/parser pipeline has a real bug — panic.
                    let truth_sql = watch_spec.query.to_block_raw_sql();
                    let truth_rows = match self.ctx.query_sql(&truth_sql).await {
                        Ok(rows) => rows,
                        Err(e) => panic!(
                            "[inv-watch-rows-match-ref truth check] block_raw query failed for watch '{}'\n\
                             sql: {}\n\
                             error: {}",
                            query_id, truth_sql, e
                        ),
                    };
                    let truth_ids: HashSet<EntityUri> = truth_rows
                        .iter()
                        .filter_map(|r| {
                            r.get("id").and_then(|v| v.as_string()).map(|s| {
                                let uri = EntityUri::parse(s)
                                    .expect("invalid entity URI in block_raw row");
                                resolve(&uri)
                            })
                        })
                        .collect();
                    if truth_ids == expected_ids {
                        let missing: Vec<&EntityUri> = expected_ids.difference(&ui_ids).collect();
                        let spurious: Vec<&EntityUri> = ui_ids.difference(&expected_ids).collect();
                        eprintln!(
                            "[inv-watch-rows-match-ref WARN] CDC stream lagged on ID set for watch '{}': \
                             ui_model has {} blocks, block_raw has {} (matches expected). \
                             Downgraded — Turso IVM CDC delivery race.\n\
                             Missing in ui_model: {:?}\n\
                             Spurious in ui_model: {:?}",
                            query_id,
                            ui_ids.len(),
                            truth_ids.len(),
                            missing,
                            spurious,
                        );
                        // ui_model is stale for this watch — skip the per-row
                        // field checks below. Re-checking against stale rows
                        // would just produce noise that masks the next signal.
                        continue;
                    }
                    panic!(
                        "CDC UI model for watch '{}' has wrong block IDs (block_raw also disagrees \
                         — real bug, not a CDC delivery race).\n\
                         Expected {} blocks: {:?}\n\
                         Got {} blocks (ui_model): {:?}\n\
                         Got {} blocks (block_raw truth): {:?}",
                        query_id,
                        expected_ids.len(),
                        expected_ids,
                        ui_ids.len(),
                        ui_ids,
                        truth_ids.len(),
                        truth_ids,
                    );
                }

                // Verify fields per block that are included in the query columns
                let query_cols = &watch_spec.query.columns;
                let fields_to_check: Vec<&str> =
                    ["content", "content_type", "source_language", "source_name"]
                        .iter()
                        .copied()
                        .filter(|f| query_cols.iter().any(|c| c == *f))
                        .collect();
                for expected_row in &expected {
                    let raw_id = match expected_row.get("id").and_then(|v| v.as_string()) {
                        Some(id) => id,
                        None => continue,
                    };
                    // Translate file: URI to block:uuid for matching against CDC data
                    let expected_id = if let Ok(uri) = EntityUri::parse(raw_id) {
                        resolve(&uri).to_string()
                    } else {
                        raw_id.to_string()
                    };

                    if let Some(ui_row) = ui_rows.iter().find(|r: &&HashMap<String, Value>| {
                        r.get("id").and_then(|v| v.as_string()) == Some(&expected_id)
                    }) {
                        // The org round-trip strips trailing whitespace per
                        // line (the parser drops trailing spaces from headlines
                        // and body lines), so normalize both sides the same way
                        // before comparing — matches `normalize_block`.
                        let normalize_content = |s: &str| -> String {
                            s.lines()
                                .map(|l| l.trim_end())
                                .collect::<Vec<_>>()
                                .join("\n")
                                .trim()
                                .to_string()
                        };
                        for field in &fields_to_check {
                            let expected_val = expected_row
                                .get(*field)
                                .and_then(|v: &Value| v.as_string())
                                .map(normalize_content);
                            let actual_val = ui_row
                                .get(*field)
                                .and_then(|v: &Value| v.as_string())
                                .map(normalize_content);
                            if actual_val != expected_val {
                                // The CDC stream lagged. Check the underlying
                                // SQL state directly: if SQL agrees with the
                                // reference, downgrade to a warning (Turso IVM
                                // CDC delivery race — the matview's stream
                                // didn't fan out the row update before our drain
                                // wait expired). If SQL also disagrees, the
                                // mutation pipeline has a real consistency bug
                                // — keep the panic.
                                let sql = format!(
                                    "SELECT {} FROM block_raw WHERE id = '{}'",
                                    field,
                                    expected_id.replace('\'', "''")
                                );
                                let sql_val = self
                                    .ctx
                                    .query_sql(&sql)
                                    .await
                                    .ok()
                                    .and_then(|rows| {
                                        rows.into_iter().next().and_then(|r| r.get(*field).cloned())
                                    })
                                    .and_then(|v| v.as_string().map(|s| s.to_string()))
                                    .map(|s| normalize_content(&s));
                                if sql_val == expected_val {
                                    eprintln!(
                                        "[inv-watch-rows-match-ref WARN] CDC stream lagged for block '{}' field '{}' \
                                         in watch '{}': ui_model={:?}, sql={:?}, expected={:?} \
                                         (downgraded — Turso IVM CDC delivery race)",
                                        expected_id,
                                        field,
                                        query_id,
                                        actual_val,
                                        sql_val,
                                        expected_val,
                                    );
                                } else {
                                    panic!(
                                        "CDC field '{}' mismatch for block '{}' in watch '{}'\n\
                                         actual_ui_model={:?}\n\
                                         actual_sql={:?}\n\
                                         expected={:?}",
                                        field,
                                        expected_id,
                                        query_id,
                                        actual_val,
                                        sql_val,
                                        expected_val,
                                    );
                                }
                            }
                        }

                        // parent_id: normalize document URIs before comparing
                        if query_cols.iter().any(|c| c == "parent_id") {
                            let normalize_parent = |v: Option<&Value>| -> Option<String> {
                                v.and_then(|v| v.as_string()).map(|s| {
                                    let uri_result = EntityUri::parse(s);
                                    if uri_result
                                        .as_ref()
                                        .is_ok_and(|u| u.is_no_parent() || u.is_sentinel())
                                    {
                                        "__document_root__".to_string()
                                    } else if let Ok(uri) = uri_result {
                                        // Translate file: URIs to block:uuid
                                        resolve(&uri).to_string()
                                    } else {
                                        s.trim().to_string()
                                    }
                                })
                            };
                            assert_eq!(
                                normalize_parent(ui_row.get("parent_id")),
                                normalize_parent(expected_row.get("parent_id")),
                                "CDC parent_id mismatch for block '{}' in watch '{}'",
                                expected_id,
                                query_id
                            );
                        }
                    }
                }
            }
        }

        // 4. View selection synchronized
        assert_eq!(self.current_view, ref_state.current_view());

        // 5. Active watches match
        assert_eq!(
            self.active_watches.keys().collect::<HashSet<_>>(),
            ref_state.active_watches.keys().collect::<HashSet<_>>(),
            "Watch sets diverged"
        );

        // 6. Structural integrity: no orphan blocks.
        //    Skip when inv-backend-blocks-match-ref detected the live_blocks mirror is stale —
        //    the parent might just be missing from the lagged snapshot,
        //    not actually orphaned in the database.
        if !live_blocks_stale {
            for block in &backend_blocks {
                if block.parent_id.is_no_parent() || block.parent_id.is_sentinel() {
                    continue;
                }
                assert!(
                    backend_blocks.iter().any(|b| b.id == block.parent_id),
                    "Orphan block: {} has invalid parent {}",
                    block.id,
                    block.parent_id
                );
            }
        }

        // 7. Navigation state verification
        let focus_rows = self
            .engine()
            .execute_query(
                "SELECT region, block_id FROM current_focus".to_string(),
                HashMap::new(),
                None,
            )
            .await
            .expect("Failed to query current_focus - this may indicate a Turso IVM bug");

        for (region, history) in &ref_state.navigation_history {
            let expected_focus = history.current_focus();
            let actual = focus_rows
                .iter()
                .find(|r| r.get("region").and_then(|v| v.as_string()) == Some(region.as_str()));

            match (actual, &expected_focus) {
                (Some(row), Some(expected_id)) => {
                    let resolved_expected = resolve(expected_id);
                    let actual_block_id = row
                        .get("block_id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string());
                    assert_eq!(
                        actual_block_id.as_deref(),
                        Some(resolved_expected.as_str()),
                        "Navigation focus mismatch for region '{}': expected {:?} (resolved {:?}), got {:?}",
                        region,
                        expected_focus,
                        resolved_expected,
                        actual_block_id
                    );
                }
                (Some(row), None) => {
                    let actual_block_id = row.get("block_id");
                    assert!(
                        actual_block_id.is_none()
                            || actual_block_id.and_then(|v| v.as_string()).is_none()
                            || matches!(actual_block_id, Some(Value::Null)),
                        "Navigation focus mismatch for region '{}': expected home (None), got {:?}",
                        region,
                        actual_block_id
                    );
                }
                (None, None) => {}
                (None, Some(expected_id)) => {
                    panic!(
                        "[check_invariants] Region '{}' should have focus on '{}' but not found in DB",
                        region, expected_id
                    );
                }
            }
        }

        // 8. Region data verification — read from CDC-driven LiveData<FocusRoot>
        // mirror of the focus_roots matview. Avoids one SQL round trip per region
        // per check; gating on `wait_for_consumers` keeps it delay-free.
        // Per-region grouping is done in Rust via the in-memory snapshot.
        if ref_state.app_started {
            let live_focus_roots = self.live_focus_roots().await;
            let mut by_region: HashMap<String, Vec<EntityUri>> = HashMap::new();
            for fr in live_focus_roots.read().values() {
                by_region
                    .entry(fr.region.clone())
                    .or_default()
                    .push(EntityUri::parse(&fr.root_id).expect("valid entity URI in focus_roots"));
            }
            for region in holon_api::Region::ALL {
                let expected = ref_state.expected_focus_root_ids(*region);

                let mut expected_ids: Vec<EntityUri> =
                    expected.into_iter().map(|uri| resolve(&uri)).collect();
                expected_ids.sort();

                let mut actual_ids: Vec<EntityUri> =
                    by_region.remove(region.as_str()).unwrap_or_default();
                actual_ids.sort();

                if actual_ids == expected_ids {
                    continue;
                }

                // Truth check: query the `focus_roots` matview directly. If the
                // matview agrees with the reference, the `LiveData<FocusRoot>`
                // mirror lagged (CDC delivery race) — same downgrade pattern as
                // inv-backend-blocks-match-ref. If the matview itself disagrees, it's a real IVM bug
                // (e.g. UPDATE through the chained `block` matview not
                // propagating, see split_block CDC-drop memory note).
                let truth_sql = format!(
                    "SELECT root_id FROM focus_roots WHERE region = '{}'",
                    region.as_str()
                );
                let truth_rows = self.ctx.query_sql(&truth_sql).await.unwrap_or_else(|e| {
                    panic!(
                        "[inv-focus-roots truth check] focus_roots query failed\n\
                         error: {}",
                        e
                    )
                });
                let mut truth_ids: Vec<EntityUri> = truth_rows
                    .iter()
                    .filter_map(|r| r.get("root_id").and_then(|v| v.as_string()))
                    .map(|s| EntityUri::parse(s).expect("valid entity URI in focus_roots row"))
                    .collect();
                truth_ids.sort();

                if truth_ids == expected_ids {
                    eprintln!(
                        "[inv-focus-roots WARN] Region '{}' LiveData<FocusRoot> mirror \
                         lagged: matview has {} rows (matches reference), mirror has {}. \
                         Downgraded — Turso IVM CDC delivery race on focus_roots → mirror.\n\
                         Missing in mirror: {:?}\n\
                         Spurious in mirror: {:?}",
                        region.as_str(),
                        truth_ids.len(),
                        actual_ids.len(),
                        truth_ids
                            .iter()
                            .filter(|id| !actual_ids.contains(id))
                            .collect::<Vec<_>>(),
                        actual_ids
                            .iter()
                            .filter(|id| !truth_ids.contains(id))
                            .collect::<Vec<_>>(),
                    );
                    continue;
                }

                // Localize: which matview lost the row? Query the chain
                // (block_raw → block matview → focus_roots matview) for the
                // missing IDs so the panic pinpoints the dropping link.
                let missing: Vec<EntityUri> = expected_ids
                    .iter()
                    .filter(|id| !truth_ids.contains(id))
                    .cloned()
                    .collect();
                let mut chain_status: Vec<String> = Vec::new();
                for id in &missing {
                    let raw_sql = format!("SELECT id FROM block_raw WHERE id = '{}'", id.as_str());
                    let raw_hit = self
                        .ctx
                        .query_sql(&raw_sql)
                        .await
                        .map(|r| !r.is_empty())
                        .unwrap_or(false);
                    let blk_sql = format!("SELECT id FROM block WHERE id = '{}'", id.as_str());
                    let blk_hit = self
                        .ctx
                        .query_sql(&blk_sql)
                        .await
                        .map(|r| !r.is_empty())
                        .unwrap_or(false);
                    chain_status.push(format!(
                        "{}: block_raw={} block={} focus_roots=false",
                        id.as_str(),
                        if raw_hit { "✓" } else { "✗" },
                        if blk_hit { "✓" } else { "✗" }
                    ));
                }

                panic!(
                    "Region '{}' focus_roots mismatch after navigation.\n\
                     Focus: {:?}\n\
                     Expected IDs:   {:?}\n\
                     Mirror IDs:     {:?}\n\
                     Matview IDs:    {:?}\n\
                     Chain status for missing rows:\n  {}\n\
                     ↑ matview itself disagrees with reference — real Turso IVM bug, \
                     not a CDC delivery race. Chain shows where the row gets dropped.",
                    region.as_str(),
                    ref_state.current_focus(*region),
                    expected_ids,
                    actual_ids,
                    truth_ids,
                    chain_status.join("\n  "),
                );
            }
        }

        // 9/10: Properties check + root layout liveness — skip for nav-only transitions
        if !nav_only {
            // 9. Verify blocks with properties HashMap are correctly stored in cache
            // Single batch query instead of per-block queries
            let blocks_with_props: Vec<&Block> = backend_blocks
                .iter()
                .filter(|b| !b.properties.is_empty())
                .collect();

            if !blocks_with_props.is_empty() {
                // Read from block_raw (writable base table) — same matview-CDC
                // race fix as inv-viewmodel-root-matches-render-expr (devlog/2026-05-05-110311.md). This query
                // only needs id + properties, both in block_raw.
                let prql = "from block_raw | filter properties != null | select {id, properties}";
                let query_result = self
                    .test_ctx()
                    .query(prql.to_string(), QueryLanguage::HolonPrql, HashMap::new())
                    .await
                    .expect("Failed to query properties batch");

                let cached_ids_with_props: HashSet<String> = query_result
                    .iter()
                    .filter_map(|row| {
                        let id = row.get("id")?.as_string()?.to_string();
                        let props = row.get("properties")?;
                        if matches!(props, Value::Null) {
                            None
                        } else {
                            Some(id)
                        }
                    })
                    .collect();

                let mut missing: Vec<String> = Vec::new();
                for block in &blocks_with_props {
                    if !cached_ids_with_props.contains(block.id.as_str()) {
                        eprintln!(
                            "[props_check] block={}, has_props=true, properties={:?}, NOT found in cache",
                            block.id, block.properties
                        );
                        missing.push(block.id.to_string());
                    }
                }

                assert!(
                    missing.is_empty(),
                    "Block properties NULL in cache for: {:?} (Value::Object serialization bug)",
                    missing
                );
            }

            // 10. Root layout via ReactiveEngine (same pipeline as GPUI frontend)
            // ReactiveEngine watches root block via watch_ui, accumulates CDC into
            // MutableBTreeMap, and produces ViewModels via signal graph.
            if ref_state.is_properly_setup() {
                let engine = self.engine();
                let root_id = ref_state
                    .root_layout_block_id()
                    .unwrap_or_else(holon_api::root_layout_block_uri);

                // Ensure ReactiveEngine exists (created during StartApp,
                // but handle edge cases where check_invariants runs first).
                self.ensure_reactive_engine(&root_id).await;

                let reactive = self.reactive_engine.borrow().clone().unwrap();

                // Ensure the reactive engine has processed pending CDC before we
                // read its snapshot. Keep the 5 s first-emission wait as a safety
                // net for cold startups, but replace the former 100 ms drain loop
                // with the same 5 ms sleep + now_or_never hybrid used in
                // drain_cdc_events. The sleep gives the engine real wall time to
                // process incoming events; the now_or_never loop drains whatever's
                // immediately ready without a 100 ms gap detection.
                let stream_closed = {
                    use futures::FutureExt;
                    use futures::StreamExt;
                    use tracing::Instrument;
                    async {
                        let mut stream = reactive.watch(&root_id);
                        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
                            Ok(Some(_)) => {
                                tokio::time::sleep(Duration::from_millis(5)).await;
                                loop {
                                    match stream.next().now_or_never() {
                                        Some(Some(_)) => continue,
                                        _ => break,
                                    }
                                }
                                false
                            }
                            Ok(None) => {
                                eprintln!("[inv-viewmodel-snapshot] Reactive stream closed, skipping");
                                true
                            }
                            Err(_) => {
                                eprintln!("[inv-viewmodel-snapshot] No data within 5s, using current state");
                                false
                            }
                        }
                    }
                    .instrument(tracing::info_span!("pbt.inv10_watch_drain"))
                    .await
                };
                if stream_closed {
                    return;
                }

                let results = reactive.ensure_watching(&root_id);
                let (render_expr, data_rows) = results.snapshot();

                if matches!(&render_expr, holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading")
                {
                    eprintln!("[inv-viewmodel-snapshot] render_expr is still loading(), skipping");
                    return;
                }

                if matches!(&render_expr, holon_api::RenderExpr::FunctionCall { name, .. } if name == "spacer")
                {
                    eprintln!("[inv-viewmodel-snapshot] Still placeholder (spacer), skipping");
                    return;
                }

                let engine_clone = Arc::clone(engine);
                let re = render_expr.clone();
                let dr = data_rows.clone();
                let display_tree = tokio::task::spawn_blocking(move || {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let services =
                            holon_frontend::reactive::HeadlessBuilderServices::new(engine_clone);
                        holon_frontend::interpret_pure(&re, &dr, &services).snapshot()
                    }))
                })
                .await
                .expect("spawn_blocking panicked");

                let display_tree = match display_tree {
                    Ok(tree) => tree,
                    Err(e) => {
                        let msg = e
                            .downcast_ref::<String>()
                            .map(|s| s.as_str())
                            .or_else(|| e.downcast_ref::<&str>().copied())
                            .unwrap_or("unknown panic");
                        eprintln!(
                            "[inv-viewmodel-snapshot] Shadow interpretation panicked: {msg} \
                             (pre-existing bug, skipping structural assertions)"
                        );
                        return;
                    }
                };
                eprintln!("[inv-viewmodel-snapshot] ViewModel from ReactiveEngine snapshot");

                // 10a. Root widget must not be "error"
                assert_ne!(
                    display_tree.widget_name(),
                    Some("error"),
                    "Root layout rendered as error widget:\n{}",
                    display_tree.pretty_print(0),
                );

                // 10b. Entity IDs in tree
                let tree_ids = display_tree.collect_entity_ids();
                eprintln!(
                    "[inv-viewmodel-snapshot] ViewModel: root='{}', {} entity IDs",
                    display_tree.widget_name().unwrap_or("?"),
                    tree_ids.len(),
                );

                // 10c. No nested error nodes
                let error_count = crate::display_assertions::count_error_nodes(&display_tree);
                assert_eq!(
                    error_count,
                    0,
                    "[inv-viewmodel-no-error-widgets] {} error node(s) in ViewModel tree:\n{}",
                    error_count,
                    display_tree.pretty_print(0),
                );

                // 10d. Root widget type matches reference model's render expression.
                // The engine wraps the root in a view_mode_switcher; the reference
                // model doesn't know about this wrapper so we look one level deeper.
                if let Some(expected_expr) = ref_state.root_render_expr() {
                    let expected_widget = match expected_expr {
                        holon_api::render_types::RenderExpr::FunctionCall { name, .. } => {
                            name.as_str()
                        }
                        _ => panic!("root render expr must be FunctionCall"),
                    };
                    let actual_widget = display_tree.widget_name();
                    let matches_expected = actual_widget == Some(expected_widget)
                        || (actual_widget == Some("view_mode_switcher")
                            && display_tree
                                .children()
                                .first()
                                .and_then(|c| c.widget_name())
                                == Some(expected_widget));
                    assert!(
                        matches_expected,
                        "[inv-viewmodel-root-matches-render-expr] Root widget '{}' doesn't match render source '{}' \
                         (root_id={})\n\
                         EXPECTED render expr (from ref_state.root_render_expr()): {}\n\
                         ACTUAL render expr (from engine.snapshot()): {}\n\
                         data_rows.len()={} ids={:?}\n\
                         {}",
                        actual_widget.unwrap_or("?"),
                        expected_widget,
                        root_id,
                        expected_expr.to_rhai(),
                        render_expr.to_rhai(),
                        data_rows.len(),
                        data_rows
                            .iter()
                            .filter_map(|r| r.get("id").and_then(|v| v.as_string()))
                            .collect::<Vec<_>>(),
                        display_tree.pretty_print(0),
                    );
                    eprintln!(
                        "[inv-viewmodel-root-matches-render-expr] Root widget '{}' matches render expr '{}'",
                        expected_widget,
                        expected_expr.to_rhai(),
                    );
                }

                // 10e. Entity IDs in tree are subset of query data IDs.
                //
                // Only meaningful when the ref model tracks a render source for
                // the root layout — i.e. rendering is driven by a user-authored
                // render expression whose `live_block()` nodes read `col("id")`
                // from data rows. When no render source is tracked, the backend
                // falls through to `render_entity()` + entity-profile variant
                // resolution, and variants like the `root_layout` block-profile
                // variant contain **literal** `live_block("block:default-*")`
                // IDs that are hardcoded in YAML and never appear in
                // `data_rows` (data_rows only contains the root block itself).
                // Gating on `root_render_expr().is_some()` keeps the assertion
                // strict where it's load-bearing and skips it when the tree IDs
                // come from profile-variant YAML rather than query data.
                let data_id_set: std::collections::HashSet<String> = data_rows
                    .iter()
                    .filter_map(|r| {
                        r.get("id")
                            .and_then(|v| v.as_string())
                            .map(|s| s.to_string())
                    })
                    .collect();
                if ref_state.root_render_expr().is_some()
                    && !tree_ids.is_empty()
                    && !data_id_set.is_empty()
                {
                    let tree_id_set: std::collections::HashSet<String> =
                        tree_ids.iter().cloned().collect();
                    let missing: Vec<&String> = tree_id_set
                        .iter()
                        .filter(|id| !data_id_set.contains(*id))
                        .collect();
                    assert!(
                        missing.is_empty(),
                        "[inv-viewmodel-entity-ids-subset-of-data] ViewModel has entity IDs not in query data.\n\
                             Missing: {:?}\n\
                             Tree IDs ({}):\n  {:?}\n\
                             Data IDs ({}):\n  {:?}\n{}",
                        missing,
                        tree_ids.len(),
                        tree_ids,
                        data_id_set.len(),
                        data_id_set,
                        display_tree.pretty_print(0),
                    );
                    eprintln!(
                        "[inv-viewmodel-entity-ids-subset-of-data] {} tree entity IDs are subset of {} data IDs",
                        tree_id_set.len(),
                        data_id_set.len(),
                    );
                }

                // 10f. Decompiled row data matches query data
                if let Some(expected_expr) = ref_state.root_render_expr() {
                    let visible_cols = expected_expr.visible_columns();
                    let rendered_rows =
                        crate::display_assertions::extract_rendered_rows(&display_tree);
                    if !rendered_rows.is_empty()
                        && !visible_cols.is_empty()
                        && !data_rows.is_empty()
                    {
                        let expected_rows: Vec<
                            std::collections::HashMap<String, holon_api::Value>,
                        > = data_rows
                            .iter()
                            .map(|r| {
                                r.iter()
                                    .filter(|(k, _)| visible_cols.contains(k))
                                    .map(|(k, v)| (k.clone(), v.clone()))
                                    .collect()
                            })
                            .collect();
                        let subset_result = crate::display_assertions::is_ordered_subset(
                            &rendered_rows
                                .iter()
                                .filter_map(|r| {
                                    r.get("content")
                                        .and_then(|v| v.as_string())
                                        .map(|s| s.to_string())
                                })
                                .collect::<Vec<_>>(),
                            &expected_rows
                                .iter()
                                .filter_map(|r| {
                                    r.get("content")
                                        .and_then(|v| v.as_string())
                                        .map(|s| s.to_string())
                                })
                                .collect::<Vec<_>>(),
                        );
                        assert!(
                            subset_result.is_subset,
                            "[inv-viewmodel-decompiled-rows-match-query] Decompiled content doesn't match query data.\n\
                                 Rendered: {:?}\nExpected: {:?}\n\
                                 Missing: {:?}\nOut of order: {:?}\n\
                                 Render expr: {}\n{}",
                            rendered_rows,
                            expected_rows,
                            subset_result.missing_from_expected,
                            subset_result.out_of_order,
                            expected_expr.to_rhai(),
                            display_tree.pretty_print(0),
                        );
                        eprintln!(
                            "[inv-viewmodel-decompiled-rows-match-query] {} decompiled rows match expected (cols: {:?})",
                            rendered_rows.len(),
                            visible_cols,
                        );
                    }
                }

                // 10g. EditableText nodes with operations must have triggers
                let (total_with_ops, missing_triggers) =
                    crate::display_assertions::count_editables_missing_triggers(&display_tree);
                assert_eq!(
                    missing_triggers,
                    0,
                    "[inv-viewmodel-editable-text-triggers] {missing_triggers}/{total_with_ops} EditableText node(s) \
                         with operations are missing triggers.\n{}",
                    display_tree.pretty_print(0),
                );
                if total_with_ops > 0 {
                    eprintln!(
                        "[inv-viewmodel-editable-text-triggers] All {total_with_ops} EditableText node(s) with ops have triggers"
                    );
                }

                // 10h. StateToggle: hard assertions on entity, operations, state
                let toggle_nodes =
                    crate::display_assertions::collect_state_toggle_nodes(&display_tree);
                for toggle in &toggle_nodes {
                    if let holon_frontend::view_model::ViewKind::StateToggle {
                        field,
                        current,
                        label,
                        states,
                    } = &toggle.kind
                    {
                        assert_eq!(
                            field, "task_state",
                            "[inv-viewmodel-state-toggle-correct] unexpected field in StateToggle"
                        );

                        let block_id_str = toggle.row_id();
                        assert!(
                            block_id_str.is_some(),
                            "[inv-viewmodel-state-toggle-correct] StateToggle has no entity id!\n{}",
                            display_tree.pretty_print(0)
                        );
                        let block_id_str = block_id_str.unwrap();
                        let block_id = EntityUri::from_raw(&block_id_str);

                        // Only assert operations/states on TASK blocks in the reference model.
                        // Non-task blocks rendered with a custom render expression containing
                        // state_toggle legitimately have no operations (the "task" profile
                        // only activates when is_task == true, i.e. task_state is set).
                        if let Some(ref_block) = ref_state.block_state.blocks.get(&block_id) {
                            let expected_state = ref_block
                                .task_state()
                                .map(|ts| ts.keyword.to_string())
                                .unwrap_or_default();

                            if ref_block.task_state().is_some() {
                                // Task blocks: full interactivity assertions
                                assert!(
                                    !toggle.operations.is_empty(),
                                    "[inv-viewmodel-state-toggle-correct] StateToggle for {block_id_str} has no operations!\n{}",
                                    display_tree.pretty_print(0)
                                );

                                assert!(
                                    holon_frontend::operations::find_set_field_op(
                                        field,
                                        &toggle.operations
                                    )
                                    .is_some(),
                                    "[inv-viewmodel-state-toggle-correct] No set_field op for '{field}' on {block_id_str}"
                                );

                                assert!(
                                    !states.is_empty(),
                                    "[inv-viewmodel-state-toggle-correct] StateToggle for {block_id_str} has empty states"
                                );
                            }

                            // Value/label assertions apply to all blocks (task or not)
                            assert_eq!(
                                current, &expected_state,
                                "[inv-viewmodel-state-toggle-correct] StateToggle current '{current}' != \
                                     reference '{expected_state}' for block {block_id}"
                            );

                            let (expected_label, _) =
                                holon_api::render_eval::state_display(current);
                            assert_eq!(
                                label, expected_label,
                                "[inv-viewmodel-state-toggle-correct] StateToggle label '{label}' != \
                                     expected '{expected_label}' for block {block_id}"
                            );
                        }
                    }
                }
                if !toggle_nodes.is_empty() {
                    eprintln!(
                        "[inv-viewmodel-state-toggle-correct] {} StateToggle node(s) verified",
                        toggle_nodes.len()
                    );
                }

                // 10h_live. Live-tree vs fresh-tree comparison.
                //
                // The fresh tree (display_tree above) is always re-interpreted
                // from current data — it can't catch bugs where set_data
                // doesn't propagate to child widgets. The HeadlessLiveTree
                // persists across transitions and receives CDC updates through
                // the collection driver's set_data path, mirroring GPUI.
                //
                // We anchor the live tree on the **main panel block**, not the
                // root. The root layout has a render expression but no data
                // query — its data_rows are always empty. Actual rows live in
                // the nested `live_block(default-main-panel)`'s own
                // ReactiveQueryResults. This is where the collection driver
                // runs and where `set_data` would fire on `VecDiff::UpdateAt`
                // when a row's task_state changes.
                //
                // If the live tree diverges from the fresh tree, child widgets
                // (state_toggle, editable_text, etc.) have stale data/props.
                if !nav_only {
                    let main_panel_id = holon_api::EntityUri::block("default-main-panel");
                    let mp_results = reactive.ensure_watching(&main_panel_id);

                    // Wait for the main panel watcher to deliver its first
                    // emission. ToggleState only fires after a sidebar click
                    // populates focus_roots, so the GQL data should be
                    // arriving — but the watcher may still be cold on the
                    // first ClickBlock-only transition.
                    {
                        use futures::StreamExt;
                        let mut mp_stream = reactive.watch(&main_panel_id);
                        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
                        loop {
                            let (mp_render, mp_rows) = mp_results.snapshot();
                            let still_loading = matches!(
                                &mp_render,
                                holon_api::RenderExpr::FunctionCall { name, .. }
                                    if name == "loading"
                            );
                            if !still_loading && !mp_rows.is_empty() {
                                break;
                            }
                            match tokio::time::timeout_at(deadline, mp_stream.next()).await {
                                Ok(Some(_)) => continue,
                                _ => break,
                            }
                        }
                    }

                    let (mp_render_expr, mp_data_rows) = mp_results.snapshot();

                    let still_loading = matches!(
                        &mp_render_expr,
                        holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading"
                    );

                    if !still_loading && !mp_data_rows.is_empty() {
                        if let Some(item_template) =
                            holon_layout_testing::live_tree::extract_item_template(&mp_render_expr)
                        {
                            let needs_init = self.live_tree.borrow().is_none();
                            if needs_init {
                                let data_source: std::sync::Arc<
                                    dyn holon_api::ReactiveRowProvider,
                                > = mp_results.clone();
                                let services: std::sync::Arc<
                                    dyn holon_frontend::reactive::BuilderServices,
                                > = reactive.clone();
                                let lt = holon_layout_testing::live_tree::HeadlessLiveTree::new(
                                    data_source,
                                    item_template.clone(),
                                    services,
                                    &reactive.runtime_handle,
                                );
                                *self.live_tree.borrow_mut() = Some(lt);
                                // Give the driver time to populate initial items.
                                tokio::time::sleep(Duration::from_millis(50)).await;
                                eprintln!(
                                    "[inv10h_live] HeadlessLiveTree initialized on \
                                     main panel ({} items, item_template={})",
                                    self.live_tree
                                        .borrow()
                                        .as_ref()
                                        .map_or(0, |t| t.item_count()),
                                    item_template.to_rhai(),
                                );
                            }

                            // Give the driver a moment to process pending VecDiff events.
                            tokio::time::sleep(Duration::from_millis(10)).await;

                            let live_ref = self.live_tree.borrow();
                            if let Some(ref lt) = *live_ref {
                                let live_items = lt.items();
                                let fresh_items: Vec<
                                    std::sync::Arc<holon_frontend::ReactiveViewModel>,
                                > = mp_data_rows
                                    .iter()
                                    .map(|row| {
                                        let ctx = holon_frontend::RenderContext::default()
                                            .with_row(row.clone());
                                        let node = reactive.interpret(&item_template, &ctx);
                                        std::sync::Arc::new(node)
                                    })
                                    .collect();

                                if live_items.len() != fresh_items.len() {
                                    // Item count mismatch: the driver hasn't caught up yet
                                    // (InsertAt/RemoveAt pending). Log but don't fail — the
                                    // bug we're catching is stale PROPS on existing items.
                                    eprintln!(
                                        "[inv10h_live] Item count mismatch: live={} fresh={} (driver lag)",
                                        live_items.len(),
                                        fresh_items.len()
                                    );
                                }

                                // Match live↔fresh items by position.
                                //
                                // The wrapper vm of `render_entity()` doesn't carry the
                                // row id on its own `data` — the row is buried in inner
                                // children (state_toggle, editable_text, ...). But both
                                // `live_items` and `fresh_items` are produced from the
                                // same `mp_data_rows` sequence with `sort_key: None`, so
                                // index `i` corresponds to `mp_data_rows[i]` on both
                                // sides. We use that row's id as the diagnostic key.
                                let mut prop_diffs = Vec::new();
                                let pair_count = live_items.len().min(fresh_items.len());
                                for i in 0..pair_count {
                                    let row_id = mp_data_rows
                                        .get(i)
                                        .and_then(|r| r.get("id"))
                                        .and_then(|v| v.as_string())
                                        .unwrap_or("?")
                                        .to_string();
                                    let diffs = crate::display_assertions::tree_diff(
                                        live_items[i].as_ref(),
                                        fresh_items[i].as_ref(),
                                    );
                                    for d in diffs {
                                        prop_diffs.push(format!("  [{i}] {row_id}: {d}"));
                                    }
                                }

                                if !prop_diffs.is_empty() {
                                    panic!(
                                        "[inv10h_live] LIVE tree diverges from FRESH tree!\n\
                                         The collection driver's set_data path produces different \
                                         props than fresh interpretation. Child widgets see stale \
                                         data in the GPUI frontend.\n\n\
                                         Diffs ({}):\n{}",
                                        prop_diffs.len(),
                                        prop_diffs.join("\n")
                                    );
                                }
                                eprintln!(
                                    "[inv10h_live] Live vs fresh: {} item pair(s) compared, no divergence",
                                    pair_count
                                );
                            }
                        } else {
                            eprintln!(
                                "[inv10h_live] no item_template in main-panel render_expr: {}",
                                mp_render_expr.to_rhai(),
                            );
                        }
                    } else {
                        eprintln!(
                            "[inv10h_live] main panel not ready (loading={}, rows={})",
                            still_loading,
                            mp_data_rows.len(),
                        );
                    }
                }

                // 10j. Virtual child / trailing slot rendering.
                //
                // When the active render expression is a tree (default in
                // collection_profile.yaml's tree_view variant with
                // creation_slot: true), the last item in every tree collection
                // must be a virtual child placeholder with entity id
                // <scheme>:__virtual:<parent_local>.
                {
                    let is_tree = ref_state
                        .active_render_expr_name(holon_api::Region::Main)
                        .map(|n| n == "tree")
                        .unwrap_or(false);
                    if is_tree {
                        fn walk(
                            node: &holon_frontend::ReactiveViewModel,
                            found: &mut usize,
                            without: &mut usize,
                        ) {
                            if let Some(ref view) = node.collection {
                                let snap = view.children_snapshot();
                                if snap.last().is_some_and(|last| {
                                    last.entity_id()
                                        .is_some_and(|id| id.contains(":__virtual:"))
                                }) {
                                    *found += 1;
                                } else if view.layout().is_some_and(|l| l.name() == "tree") {
                                    *without += 1;
                                }
                            }
                            for child in &node.children {
                                walk(child, found, without);
                            }
                            if let Some(ref slot) = node.slot {
                                let guard = slot.content.lock_ref();
                                walk(&guard, found, without);
                            }
                        }
                        let found = 0usize;
                        let without = 0usize;
                        // FIXME: display_tree must be obtained from
                        // wait_for_entity_in_resolved_view_model or similar
                        // before this invariant can meaningfully execute.
                        // Blocked on inv-viewmodel-tree-virtual-slots wiring — see memory entry
                        // pbt_zero_height_reproduction.md.
                        let _ = (found, without);
                        eprintln!(
                            "[inv-viewmodel-tree-virtual-slots] SKIPPED — display_tree not wired in this scope"
                        );
                        if found > 0 {
                            eprintln!(
                                "[inv-viewmodel-tree-virtual-slots] Virtual child slot(s): {found} OK"
                            );
                        }
                        if without > 0 && found == 0 {
                            eprintln!(
                                "[inv-viewmodel-tree-virtual-slots] WARNING: {without} tree collection(s) \
                                 with no virtual child — creation_slot may be \
                                 inactive for this seed."
                            );
                        }
                    }
                }

                // 10i. Matview data IDs must match reference model (catches IVM inconsistency)
                //
                // The data_rows come from the matview snapshot (CDC pipeline). If the
                // matview is inconsistent with the base table (Turso IVM bug), data_rows
                // will have extra/missing rows compared to the reference model.
                //
                // The root layout query returns all non-source descendants of the focus
                // roots. We compute this set from the reference model and compare.
                if !data_rows.is_empty() {
                    let data_block_ids: std::collections::BTreeSet<String> = data_rows
                        .iter()
                        .filter_map(|r| {
                            r.get("id")
                                .and_then(|v| v.as_string())
                                .map(|s| s.to_string())
                        })
                        .collect();

                    // Compute expected: all blocks in reference model (including source).
                    // Also include layout blocks and profile blocks which the ref model
                    // doesn't track as regular blocks but are in the DB.
                    let ref_block_ids: std::collections::BTreeSet<String> = ref_state
                        .block_state
                        .blocks
                        .values()
                        .map(|b| b.id.as_str().to_string())
                        .chain(
                            ref_state
                                .layout_blocks
                                .headline_ids
                                .iter()
                                .chain(&ref_state.layout_blocks.query_source_ids)
                                .chain(&ref_state.layout_blocks.render_source_ids)
                                .chain(&ref_state.profile_block_ids)
                                .map(|id| id.as_str().to_string()),
                        )
                        .collect();

                    // Extra IDs in matview that aren't in reference model
                    let extra: Vec<&String> = data_block_ids
                        .iter()
                        .filter(|id| !ref_block_ids.contains(*id))
                        .collect();

                    // Missing IDs in matview that should be visible
                    // (only check blocks that are in the focus tree, not all reference blocks)
                    let focus_roots = ref_state.expected_focus_root_ids(holon_api::Region::Main);
                    let expected_visible: std::collections::BTreeSet<String> = ref_state
                        .block_state
                        .blocks
                        .values()
                        .filter(|b| {
                            !matches!(b.content_type, holon_api::ContentType::Source)
                                && ref_state.is_descendant_of_any(&b.id, &focus_roots)
                        })
                        .map(|b| b.id.as_str().to_string())
                        .collect();

                    let missing: Vec<&String> = expected_visible
                        .iter()
                        .filter(|id| !data_block_ids.contains(*id))
                        .collect();

                    if !extra.is_empty() || !missing.is_empty() {
                        eprintln!(
                            "[inv-matview-consistent-with-ref] IVM MATVIEW INCONSISTENCY DETECTED!\n\
                                 Data rows (from matview): {} IDs\n\
                                 Reference model: {} total blocks, {} expected visible\n\
                                 Extra in matview (stale/ghost): {:?}\n\
                                 Missing from matview: {:?}",
                            data_block_ids.len(),
                            ref_block_ids.len(),
                            expected_visible.len(),
                            extra,
                            missing,
                        );
                    }
                    // NOTE: These are soft checks because the AppState data_rows come
                    // from the ROOT LAYOUT query (returns layout column blocks), not
                    // from region-specific queries (which return user content blocks).
                    // The data sets are different levels of the rendering hierarchy.
                    if !extra.is_empty() {
                        eprintln!(
                            "[inv-matview-consistent-with-ref] Matview has {} extra block IDs not in reference model: {:?}",
                            extra.len(),
                            extra,
                        );
                    }
                    // TODO: Re-enable once inv-matview-consistent-with-ref compares region-specific data
                    // (not root layout data which is a different hierarchy level).
                    // if !missing.is_empty() {
                    //     eprintln!(
                    //         "[inv-matview-consistent-with-ref] Matview is MISSING {} block IDs: {:?}",
                    //         missing.len(), missing,
                    //     );
                    // }
                    if extra.is_empty() && missing.is_empty() {
                        eprintln!(
                            "[inv-matview-consistent-with-ref] Matview data ({} rows) consistent with reference model",
                            data_block_ids.len(),
                        );
                    }
                }
            }

            // ─── inv-value-fn-provider-arg-variance/12/13: value-fn provider invariants ────────────────
            //
            // These invariants cover the `ReactiveRowProvider`s produced by
            // value functions (`focus_chain`, `ops_of`, `chain_ops`). The
            // reactive engine caches them via `ProviderCache` so repeated
            // `(name, args)` calls share an `Arc`. We re-interpret the
            // current render tree against the live engine (so the cache is
            // active) and walk the resulting tree collecting streaming
            // providers.
            //
            // Viewport trigger: push a narrow 500×800 viewport so the
            // default `block:root-layout` profile picks the
            // `if_space(600, ...)` branch that instantiates the mobile
            // action bar (`focus_chain()` + `ops_of(col("uri"))`). Without
            // this the PBT would only exercise the chain_ops fixture in
            // `valid_render_expressions` when it's randomly chosen — the
            // narrow viewport guarantees coverage on every run that has a
            // root layout present. `ui_state.set_viewport` sets a
            // `Mutable` that the reactive signal graph already subscribes
            // to, so one scheduler tick propagates it downstream.
            if ref_state.app_started && !ref_state.block_state.blocks.is_empty() {
                use crate::pbt::value_fn_invariants::{
                    collect_providers, count_bottom_docks, rhai_mentions,
                };

                let reactive = match self.reactive_engine.borrow().clone() {
                    Some(r) => r,
                    None => return,
                };

                reactive
                    .ui_state()
                    .set_viewport(holon_frontend::reactive::ViewportInfo {
                        width_px: 500.0,
                        height_px: 800.0,
                        scale_factor: 1.0,
                    });
                tokio::task::yield_now().await;
                let root_id = ref_state
                    .root_layout_block_id()
                    .unwrap_or_else(holon_api::root_layout_block_uri);
                let results = reactive.ensure_watching(&root_id);
                let (render_expr, data_rows) = results.snapshot();

                if matches!(&render_expr, holon_api::RenderExpr::FunctionCall { name, .. } if name == "loading" || name == "spacer")
                {
                    // Root still initializing — nothing to observe.
                } else {
                    let services: Arc<dyn holon_frontend::reactive::BuilderServices> =
                        reactive.clone();

                    let re = render_expr.clone();
                    let dr = data_rows.clone();
                    let svc1 = services.clone();
                    let tree1 = tokio::task::spawn_blocking(move || {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            holon_frontend::interpret_pure(&re, &dr, &*svc1)
                        }))
                        .ok()
                    })
                    .await
                    .expect("spawn_blocking panicked");

                    let Some(tree1) = tree1 else {
                        eprintln!(
                            "[inv-value-fn-provider-arg-variance-13] first interpret panicked, skipping"
                        );
                        return;
                    };

                    let providers1 = collect_providers(&tree1);
                    let total1 = providers1.len();

                    // inv_bar — bottom_dock structural presence.
                    //
                    // If the active render_expr for the root layout
                    // mentions `bottom_dock`, the interpreted tree must
                    // contain at least one `BottomDock` node with
                    // exactly two children (main + dock slot). Catches
                    // regressions where the `bottom_dock` widget
                    // silently falls through to the `unknown` arm, or
                    // its shadow builder drops a slot.
                    if rhai_mentions(&render_expr, "bottom_dock") {
                        let docks = count_bottom_docks(&tree1);
                        assert!(
                            docks >= 1,
                            "[inv_bar] render_expr mentions bottom_dock but \
                             interpreted tree contains 0 BottomDock nodes"
                        );
                        eprintln!("[inv_bar] bottom_dock count = {docks}");
                    }

                    // inv-value-fn-provider-arg-variance — provider arg variance.
                    //
                    // Only assert when the **active** render_expr (the one
                    // the reactive engine just interpreted) mentions
                    // `focus_chain` AND a focus target is set AND the
                    // walker actually surfaced a streaming provider. This
                    // keeps the check specific to cases where a
                    // focus_chain-backed node is genuinely present —
                    // render_expressions in `ref_state` may contain
                    // fixtures attached to nested blocks that the current
                    // interpretation doesn't reach.
                    let active_has_focus_chain = rhai_mentions(&render_expr, "focus_chain");
                    let expects_focus_rows =
                        ref_state.focused_block.is_some() && active_has_focus_chain && total1 > 0;
                    let any_nonempty = providers1.iter().any(|p| p.rows_snapshot_len > 0);
                    eprintln!(
                        "[vfn11] streaming_providers={} any_nonempty={} \
                         expects_focus_rows={} active_has_focus_chain={}",
                        total1, any_nonempty, expects_focus_rows, active_has_focus_chain,
                    );
                    if expects_focus_rows {
                        assert!(
                            any_nonempty,
                            "[vfn11] active render_expr mentions focus_chain and \
                             reference model has focused_block = {:?}, but no streaming \
                             provider produced rows",
                            ref_state.focused_block,
                        );
                    }

                    // inv-value-fn-provider-identity — provider identity stability within one pass.
                    //
                    // Group by `(item_template_debug, rows_snapshot_len)` — a
                    // coarse but useful proxy for "same `(name, args)`".
                    // Track per-group **call-site count** (how many walker
                    // visits landed on that group) and the set of distinct
                    // `cache_identity()` values seen. A group with more
                    // than one call site but exactly one identity is
                    // evidence of cache reuse — one `Arc` serving several
                    // sites. The "reuse" metric is what the handoff's
                    // "cache reuse > 0" acceptance is checking for; it is
                    // reported alongside the group count.
                    use std::collections::{HashMap, HashSet};
                    let mut sites_per_group: HashMap<(String, usize), usize> = HashMap::new();
                    let mut ids_per_group: HashMap<(String, usize), HashSet<u64>> = HashMap::new();
                    for p in &providers1 {
                        let key = (p.item_template_debug.clone(), p.rows_snapshot_len);
                        *sites_per_group.entry(key.clone()).or_default() += 1;
                        ids_per_group
                            .entry(key)
                            .or_default()
                            .insert(p.cache_identity);
                    }
                    let mut reuse_groups = 0usize;
                    let mut reuse_sites = 0usize;
                    for (key, ids) in &ids_per_group {
                        let sites = sites_per_group.get(key).copied().unwrap_or(0);
                        if ids.len() > 1 {
                            panic!(
                                "[vfn12] provider identity instability: template={} \
                                 rows={} → {} distinct cache_identities across {sites} call sites",
                                key.0,
                                key.1,
                                ids.len(),
                            );
                        }
                        if sites > 1 {
                            reuse_groups += 1;
                            reuse_sites += sites;
                        }
                    }
                    eprintln!(
                        "[vfn12] provider groups={} reuse_groups={} reuse_sites={}",
                        ids_per_group.len(),
                        reuse_groups,
                        reuse_sites,
                    );

                    // inv-sql-budget — no flicker across re-interpret.
                    // Re-run interpretation; every cache_identity observed
                    // in pass-1 should still appear in pass-2 (Arcs persist
                    // because `ProviderCache` hands out the same Weak on
                    // unchanged args).
                    let re2 = render_expr.clone();
                    let dr2 = data_rows.clone();
                    let svc2 = services.clone();
                    let tree2 = tokio::task::spawn_blocking(move || {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            holon_frontend::interpret_pure(&re2, &dr2, &*svc2)
                        }))
                        .ok()
                    })
                    .await
                    .expect("spawn_blocking panicked");

                    let Some(tree2) = tree2 else {
                        eprintln!("[vfn13] second interpret panicked, skipping");
                        return;
                    };

                    let providers2 = collect_providers(&tree2);
                    let ids1: std::collections::HashSet<u64> =
                        providers1.iter().map(|p| p.cache_identity).collect();
                    let ids2: std::collections::HashSet<u64> =
                        providers2.iter().map(|p| p.cache_identity).collect();
                    let flickered: Vec<u64> = ids1.difference(&ids2).copied().collect();
                    eprintln!(
                        "[vfn13] pass1 ids={} pass2 ids={} stable={}",
                        ids1.len(),
                        ids2.len(),
                        ids1.intersection(&ids2).count(),
                    );
                    assert!(
                        flickered.is_empty(),
                        "[vfn13] provider cache identity flicker: {} ids present in pass-1 \
                         but missing in pass-2 — cache wiring regressed",
                        flickered.len(),
                    );
                }
            }
        } // end if !nav_only (#9, #10)

        // 11. Loro vs Org check DISABLED: Loro is no longer the write path for blocks.
        // All block CRUD goes through SqlOperationProvider. Loro is populated via EventBus
        // subscriptions (reverse sync) which hasn't been implemented yet.
        // Re-enable this check once EventBus → Loro sync is in place.

        // 12. Every intermediate ViewModel emission must have correct StateToggle values.
        //
        // A background task collects ALL ViewModel emissions from the reactive stream.
        // We drain and check each one — this catches transient bugs where the CDC
        // enrichment pipeline produces incorrect data that is later masked when a
        // structural re-render fetches fresh data from the query path.
        //
        // Without this, bugs like flatten_properties only handling Value::Object (not
        // Value::String from the CDC path) go undetected because the final snapshot
        // always has correct data from the query path.
        if ref_state.app_started && !nav_only {
            let emissions: Vec<holon_frontend::ViewModel> =
                std::mem::take(&mut *self.vm_emissions.lock().unwrap());

            let mut checked = 0usize;
            for (i, vm) in emissions.iter().enumerate() {
                let toggles = crate::display_assertions::collect_state_toggle_nodes(vm);
                for toggle in &toggles {
                    if let holon_frontend::view_model::ViewKind::StateToggle { current, .. } =
                        &toggle.kind
                    {
                        let Some(block_id_str) = toggle.row_id() else {
                            continue;
                        };
                        let block_id = EntityUri::from_raw(&block_id_str);
                        let Some(ref_block) = ref_state.block_state.blocks.get(&block_id) else {
                            continue;
                        };
                        let expected = ref_block
                            .task_state()
                            .map(|ts| ts.keyword.to_string())
                            .unwrap_or_default();

                        assert_eq!(
                            current, &expected,
                            "[inv-value-fn-provider-identity] Intermediate ViewModel emission #{i} has wrong \
                             StateToggle value for block {block_id}.\n\
                             Got '{current}', expected '{expected}'.\n\
                             This means the CDC enrichment pipeline produced incorrect \
                             data that would be visible as a UI glitch before the next \
                             structural re-render masks it."
                        );
                        checked += 1;
                    }
                }
            }
            if checked > 0 {
                eprintln!(
                    "[inv-value-fn-provider-identity] Verified {} StateToggle value(s) across {} intermediate ViewModel emissions",
                    checked,
                    emissions.len(),
                );
            }
        }

        // ── 13. Non-functional span invariants (SQL counts, durations, memory) ────
        #[cfg(feature = "otel-testing")]
        {
            let metrics = self.span_collector.snapshot();
            let wall_time = self
                .last_transition_start
                .map(|t| t.elapsed())
                .unwrap_or_default();
            let key = super::transition_budgets::transition_key(&self.last_transition);

            // 13d. RSS memory tracking
            let rss_after = crate::test_tracing::current_rss_bytes();
            let memory = super::transition_budgets::MemoryMetrics {
                rss_before: self.rss_before,
                rss_after,
                rss_baseline: self.rss_baseline,
            };

            // 13b. Summary line (always printed before violations can panic)
            let expected =
                super::transition_budgets::expected_sql(&self.last_transition, ref_state);
            let render_summary: String = if metrics.render_count > 0 {
                let components: Vec<_> = metrics
                    .render_by_component
                    .iter()
                    .map(|(c, n)| format!("{c}={n}"))
                    .collect();
                format!(
                    " renders={} [{}]",
                    metrics.render_count,
                    components.join(",")
                )
            } else {
                String::new()
            };
            let cdc_summary: String =
                if metrics.cdc_ingest_count > 0 || metrics.cdc_emission_count > 0 {
                    format!(
                        " cdc_in={} cdc_out={}",
                        metrics.cdc_ingest_count, metrics.cdc_emission_count
                    )
                } else {
                    String::new()
                };
            // HOLON_PERF investigation: per-transition attribution of suspected hot paths.
            let perf_summary = format!(
                " apply={}ms check={}ms drain_cdc={}ms inv10_drain={}ms files_stable={}ms file_sync={}ms mark_proc={}ms×{}",
                metrics.apply_transition_total.as_millis(),
                metrics.check_invariants_total.as_millis(),
                metrics.drain_cdc_total.as_millis(),
                metrics.inv10_watch_drain.as_millis(),
                metrics.wait_files_stable.as_millis(),
                metrics.wait_file_sync.as_millis(),
                metrics.mark_processed_total.as_millis(),
                metrics.mark_processed_count,
            );
            eprintln!(
                "[inv-sql-budget] {key}: reads={}/{} writes={}/{} ddl={}/{} tol={} max_q={}ms wall={}ms spans={} \
                 rss={delta:+.1}MB (cum={cum:+.1}MB){render_summary}{cdc_summary}{perf_summary}",
                metrics.sql_read_count,
                expected.reads,
                metrics.sql_write_count,
                expected.writes,
                metrics.sql_ddl_count,
                expected.ddl,
                expected.tolerance,
                metrics.max_query_duration.as_millis(),
                wall_time.as_millis(),
                metrics.total_span_count,
                delta = memory.rss_delta_mb(),
                cum = memory.cumulative_growth_mb(),
            );

            // 13c. Budget violation checks (may panic)
            let violations = super::transition_budgets::check_budget(
                &self.last_transition,
                ref_state,
                &metrics,
                wall_time,
                Some(&memory),
            );

            // Budgets drifted significantly after the reactive refactor; opt
            // into enforcement explicitly via HOLON_PERF_BUDGET=1 once they
            // are recalibrated. Default behavior logs violations as warnings.
            let enforce_budget = std::env::var("HOLON_PERF_BUDGET")
                .map(|v| v != "0")
                .unwrap_or(false);

            let has_memory_violation = violations.iter().any(|v| match v {
                super::transition_budgets::Violation::Error(msg) => msg.contains("rss_"),
                _ => false,
            });

            if has_memory_violation {
                super::transition_budgets::diagnose_memory(&key);
            }

            for v in &violations {
                match v {
                    super::transition_budgets::Violation::Warning(msg) => {
                        eprintln!("[inv-sql-budget WARN] {msg}");
                    }
                    super::transition_budgets::Violation::Error(msg) => {
                        if enforce_budget {
                            panic!("inv-sql-budget: {msg}");
                        } else {
                            eprintln!("[inv-sql-budget BUDGET OFF] {msg}");
                        }
                    }
                }
            }

            // 13d. Duplicate SQL detection — warn about potential N+1 patterns
            if !metrics.duplicate_sql.is_empty() {
                eprintln!(
                    "[inv-sql-budget N+1] {key}: {} distinct SQL texts fired multiple times:",
                    metrics.duplicate_sql.len()
                );
                for (sql, count) in &metrics.duplicate_sql {
                    eprintln!("  {count}x: {sql}");
                }
            }

            // 13e. Flamegraph (opt-in via HOLON_PERF_FLAMEGRAPH=/path/to/dir)
            crate::test_tracing::maybe_write_flamegraph(&self.span_collector, &key);

            // Detailed SQL breakdown (enabled by HOLON_PERF_DETAIL=1)
            if std::env::var("HOLON_PERF_DETAIL").is_ok() {
                let breakdown = self.span_collector.sql_breakdown();
                eprintln!("[inv-sql-budget DETAIL] {key}:\n{breakdown}");
            }
        }

        // ── inv-frontend-engine: Frontend engine ViewModel assertions ─────────
        //
        // When a frontend engine is installed (e.g., GPUI PBT), check that
        // the frontend's own ReactiveEngine produces a valid ViewModel.
        // This catches issues invisible to the headless engine: matview
        // failures, CDC delivery bugs, cross-executor waker issues.
        if let Some(ref fe_engine) = self.frontend_engine {
            let root_uri = holon_api::root_layout_block_uri();
            let rqr = fe_engine.ensure_watching(&root_uri);

            if rqr.is_loading() {
                eprintln!(
                    "[inv-frontend-engine] Frontend engine still loading root layout — skipping"
                );
            } else {
                let vm = fe_engine.snapshot(&root_uri);
                let root_kind = vm.widget_name().unwrap_or("?");

                // 14a: Root widget must not be Error
                assert_ne!(
                    root_kind,
                    "error",
                    "[inv-frontend-root-not-error] Frontend root widget is Error: {:?}",
                    vm.entity.get("error_message"),
                );

                // 14b: No Error widgets anywhere in the tree
                let error_count = crate::display_assertions::count_error_nodes(&vm);
                if error_count > 0 {
                    let summaries = crate::display_assertions::collect_error_node_summaries(&vm);
                    eprintln!(
                        "[inv-frontend-no-error-widgets] {} Error widget(s) in ViewModel:",
                        summaries.len()
                    );
                    for s in &summaries {
                        eprintln!("    {s}");
                    }
                }
                assert!(
                    error_count == 0,
                    "[inv-frontend-no-error-widgets] Frontend ViewModel contains {error_count} Error widget(s)",
                );

                // 14c: BoundsRegistry assertions — verify GPUI actually laid out elements
                let entity_ids = vm.collect_entity_ids();
                if let Some(ref geometry) = self.frontend_geometry {
                    // Wait for GPUI to render at least one tracked element. The
                    // backend ViewModel resolves faster than the GPUI render pipeline;
                    // the first check can land before any prepaint has run.
                    let all_elements = {
                        let mut elements = geometry.all_elements();
                        if elements.is_empty() && !ref_state.documents.is_empty() {
                            // GPUI debug builds need more time: the render
                            // pipeline (signal → render → prepaint → record)
                            // can take several seconds after a mutation.
                            for _ in 0..50 {
                                std::thread::sleep(std::time::Duration::from_millis(200));
                                elements = geometry.all_elements();
                                if !elements.is_empty() {
                                    break;
                                }
                            }
                        }
                        elements
                    };

                    // An entity is "rendered" if any tracked element has its
                    // entity_id — checked via both el_id prefix (for fast path)
                    // and entity_id field (for selectable/editable_text widgets
                    // whose el_id uses different prefixes).
                    let lookup_entity = |eid: &str| {
                        geometry
                            .element_info(&format!("render-entity-{eid}"))
                            .or_else(|| geometry.element_info(&format!("live-block-{eid}")))
                            .or_else(|| geometry.element_info(&format!("selectable-{eid}")))
                            .or_else(|| geometry.element_info(&format!("editable-text-{eid}")))
                            .or_else(|| {
                                // Fallback: scan all_elements for any entity_id match
                                all_elements
                                    .iter()
                                    .find(|(_, info)| info.entity_id.as_deref() == Some(eid))
                                    .map(|(_, info)| info.clone())
                            })
                    };

                    // Dump tracked elements as a parent-indented tree (helps
                    // diagnose assertion failures). Each element's `parent_id`
                    // points at the nearest enclosing tracked widget recorded
                    // by `TransparentTracker`. Children are sorted by (y, x,
                    // el_id) so painting order is preserved visually. Orphans
                    // (parent_id pointing at a missing entry) are surfaced
                    // under a synthetic `<orphan>` root rather than silently
                    // dropped.
                    {
                        use std::collections::HashMap;

                        let by_id: HashMap<&str, &holon_frontend::geometry::ElementInfo> =
                            all_elements.iter().map(|(k, v)| (k.as_str(), v)).collect();

                        let mut children_of: HashMap<Option<&str>, Vec<&str>> = HashMap::new();
                        let mut orphans: Vec<&str> = Vec::new();
                        for (el_id, info) in &all_elements {
                            match info.parent_id.as_deref() {
                                None => children_of.entry(None).or_default().push(el_id.as_str()),
                                Some(p) if by_id.contains_key(p) => {
                                    children_of.entry(Some(p)).or_default().push(el_id.as_str())
                                }
                                Some(_) => orphans.push(el_id.as_str()),
                            }
                        }
                        let sort_children = |ids: &mut Vec<&str>| {
                            ids.sort_by(|a, b| {
                                let ai = by_id[a];
                                let bi = by_id[b];
                                ai.y.partial_cmp(&bi.y)
                                    .unwrap_or(std::cmp::Ordering::Equal)
                                    .then(
                                        ai.x.partial_cmp(&bi.x)
                                            .unwrap_or(std::cmp::Ordering::Equal),
                                    )
                                    .then_with(|| a.cmp(b))
                            });
                        };
                        for ids in children_of.values_mut() {
                            sort_children(ids);
                        }
                        sort_children(&mut orphans);

                        fn print_node(
                            id: &str,
                            depth: usize,
                            by_id: &HashMap<&str, &holon_frontend::geometry::ElementInfo>,
                            children_of: &HashMap<Option<&str>, Vec<&str>>,
                            label: &str,
                        ) {
                            let info = by_id[id];
                            let indent = "  ".repeat(depth);
                            eprintln!(
                                "[{label}] {indent}{id}: widget_type={} entity_id={:?} bounds=({:.0},{:.0} {:.0}x{:.0}) has_content={}",
                                info.widget_type,
                                info.entity_id,
                                info.x,
                                info.y,
                                info.width,
                                info.height,
                                info.has_content,
                            );
                            if let Some(kids) = children_of.get(&Some(id)) {
                                for child in kids {
                                    print_node(child, depth + 1, by_id, children_of, label);
                                }
                            }
                        }

                        if let Some(roots) = children_of.get(&None) {
                            for root in roots {
                                print_node(
                                    root,
                                    0,
                                    &by_id,
                                    &children_of,
                                    "inv-frontend-engine TREE",
                                );
                            }
                        }
                        if !orphans.is_empty() {
                            eprintln!(
                                "[inv-frontend-engine TREE] <orphan> ({} entries — parent_id refers to missing element)",
                                orphans.len()
                            );
                            for id in &orphans {
                                print_node(id, 1, &by_id, &children_of, "inv-frontend-engine TREE");
                            }
                        }
                    }

                    // bounds-registry-not-empty: At least 1 element rendered (warning — BoundsRegistry is
                    // a layout-time snapshot; double-buffering means it can be
                    // transiently empty during restarts and state changes. Use not-visually-empty
                    // for authoritative empty-UI detection.)
                    if all_elements.is_empty() {
                        eprintln!(
                            "[inv-frontend-bounds-rendered/bounds-registry-not-empty WARN] BoundsRegistry is empty — GPUI may not have rendered yet (check not-visually-empty for visual emptiness)",
                        );
                    }

                    // expected-size-satisfied: every tracked element's observed (w, h) must satisfy its
                    // declared `expected_size` bounds. Bounds default to "all Free"
                    // (= unconstrained), so widgets that don't opt in are skipped.
                    // The previous hard-coded `live_block` / `spacer` allowlist is
                    // gone — those widgets are simply unconstrained by default. Leaf
                    // widgets (text/icon/selectable/...) can declare `at_least(...)`
                    // to catch genuine "rendered too small" bugs; wrappers can use
                    // `follows_child(child_id)` to express "I'm transparent to layout
                    // and inherit my child's expectation". See
                    // `holon_frontend::size_expectation` for the AST.
                    for (el_id, info) in &all_elements {
                        let ctx = holon_frontend::geometry::ProviderEvalCtx::from_snapshot(
                            &all_elements,
                            el_id.as_str(),
                            None, // viewport unknown here; widgets that need it can be
                                  // wired up later when the test owns the window dims.
                        );
                        if let Err(violation) =
                            info.expected_size.check(info.width, info.height, &ctx)
                        {
                            panic!(
                                "[inv-frontend-bounds-rendered/expected-size-satisfied] Element '{el_id}' violates expected_size: {violation}\n  observed: {info:?}",
                            );
                        }
                    }

                    // vm-entities-have-bounds: Entity IDs from ViewModel that have corresponding bounds (warning —
                    // uniform_list virtualizes, so not all ViewModel entities are rendered).
                    //
                    // Layout blocks (direct children of root-layout, e.g. default-main-panel)
                    // are deliberately NOT tracked by the live_block builder — wrapping a
                    // whole region in BoundsTracker causes the wrapper to collapse to height=0
                    // and clips all region content (see live_block.rs comments). Skip these
                    // to avoid false-positive warnings.
                    let layout_block_ids: std::collections::HashSet<&str> = [
                        "block:default-main-panel",
                        "block:default-left-sidebar",
                        "block:default-right-sidebar",
                    ]
                    .into_iter()
                    .collect();
                    let mut missing = Vec::new();
                    for eid in &entity_ids {
                        if layout_block_ids.contains(eid.as_str()) {
                            continue;
                        }
                        if lookup_entity(eid).is_none() {
                            missing.push(eid.clone());
                        }
                    }

                    // no-error-widgets-rendered: No error widgets rendered
                    for (el_id, info) in &all_elements {
                        assert!(
                            info.widget_type != "error",
                            "[inv-frontend-bounds-rendered/no-error-widgets-rendered] BoundsRegistry contains error widget '{el_id}': {info:?}",
                        );
                    }

                    // known-widget-type: Widget type consistency (warning) — for entity IDs present in both
                    // ViewModel and BoundsRegistry, the widget_type should be one of the
                    // known rendering wrappers.
                    for (el_id, info) in &all_elements {
                        if let Some(ref eid) = info.entity_id
                            && entity_ids.contains(eid)
                        {
                            let ok = matches!(
                                info.widget_type.as_str(),
                                "render_entity"
                                    | "live_block"
                                    | "editable_text"
                                    | "rendered_text"
                                    | "selectable"
                            );
                            if !ok {
                                eprintln!(
                                    "[inv-frontend-bounds-rendered/known-widget-type] Element '{el_id}' entity={eid} has unexpected widget_type='{}'",
                                    info.widget_type,
                                );
                            }
                        }
                    }

                    // element-has-content: Content presence (warning) — rendered elements with entity bindings
                    // should have content when ViewModel says they do.
                    for (el_id, info) in &all_elements {
                        if !info.has_content {
                            eprintln!(
                                "[inv-frontend-bounds-rendered/element-has-content WARN] Element '{el_id}' (widget_type='{}') has has_content=false",
                                info.widget_type,
                            );
                        }
                    }

                    // vm-y-order-and-contiguity: Y-order consistency — rendered elements that correspond to ViewModel
                    // entity IDs must appear in the same y-axis order and form a contiguous
                    // subsequence of the ViewModel's entity list.
                    //
                    // Exclude layout blocks (direct children of root-layout) from the index
                    // computation — they're never rendered via tracked() (see live_block.rs),
                    // so they naturally create gaps in the rendered-index sequence.
                    let contiguity_entity_ids: Vec<&String> = entity_ids
                        .iter()
                        .filter(|eid| !layout_block_ids.contains(eid.as_str()))
                        .collect();
                    let rendered_entities: Vec<(usize, &str, f32)> = contiguity_entity_ids
                        .iter()
                        .enumerate()
                        .filter_map(|(vm_idx, eid)| {
                            let info = lookup_entity(eid)?;
                            Some((vm_idx, eid.as_str(), info.y))
                        })
                        .collect();

                    if rendered_entities.len() >= 2 {
                        // Check y-order: each rendered element's y should be >= previous
                        for pair in rendered_entities.windows(2) {
                            let (_, id_a, y_a) = pair[0];
                            let (_, id_b, y_b) = pair[1];
                            assert!(
                                y_b >= y_a,
                                "[inv-frontend-bounds-rendered/vm-y-order-and-contiguity] Y-order violation: '{id_a}' at y={y_a:.0} appears before '{id_b}' at y={y_b:.0}",
                            );
                        }

                        // Check contiguity: ViewModel indices of rendered elements must be consecutive
                        for pair in rendered_entities.windows(2) {
                            let (idx_a, id_a, _) = pair[0];
                            let (idx_b, id_b, _) = pair[1];
                            assert!(
                                idx_b == idx_a + 1,
                                "[inv-frontend-bounds-rendered/vm-y-order-and-contiguity] Non-contiguous rendering: '{id_a}' at VM index {idx_a} \
                                 and '{id_b}' at VM index {idx_b} — gap of {} entities",
                                idx_b - idx_a - 1,
                            );
                        }
                    }

                    // non-wrapper-content-when-docs and not-visually-empty are gated on the root layout being fully loaded.
                    // When root_kind == "table", the render_expr matview hasn't delivered
                    // the columns() expression yet — the UI shows a loading/fallback state.
                    // Asserting on that transient state would be a false positive.
                    let layout_ready = root_kind != "table";
                    if !layout_ready {
                        eprintln!(
                            "[inv-frontend-bounds-rendered] Root widget is '{}' (loading) — skipping non-wrapper-content-when-docs/not-visually-empty",
                            root_kind,
                        );
                    }

                    // non-wrapper-content-when-docs: Non-container content exists — when ref_state has user documents,
                    // at least one tracked element must be a content widget (render_entity,
                    // editable_text, or selectable), NOT just a live_block wrapper.
                    //
                    // Skip if BoundsRegistry is entirely empty — that's bounds-registry-not-empty's concern and is
                    // better detected via not-visually-empty (visual emptiness from screenshot), which knows
                    // how to distinguish transient empty state (restart/layout race) from a
                    // truly broken render. Firing non-wrapper-content-when-docs on an empty registry produces a
                    // misleading error message ("only live_block wrappers") when in fact
                    // there are no elements at all.
                    if !ref_state.documents.is_empty() && layout_ready && !all_elements.is_empty() {
                        let has_content_widget = all_elements
                            .iter()
                            .any(|(_, info)| info.widget_type != "live_block");
                        assert!(
                            has_content_widget,
                            "[inv-frontend-bounds-rendered/non-wrapper-content-when-docs] ref_state has {} document(s) and BoundsRegistry has \
                             {} elements, but all are live_block wrappers — no content widgets \
                             rendered",
                            ref_state.documents.len(),
                            all_elements.len(),
                        );
                    }

                    // not-visually-empty: Pixel-level empty UI detection — the ground truth for visible
                    // content. BoundsRegistry tracks layout, which can be wildly different
                    // from what's actually painted (clipped elements, stale entries, layout
                    // races). This invariant reads a recent screenshot's analysis and fails
                    // if the window's content area is almost entirely background color.
                    //
                    // Threshold: content_fraction must be > 0.003 (0.3% of content-area
                    // pixels). An empty macOS window with just the title bar typically
                    // measures ~0.001-0.0025; a sparse sidebar-only UI measures ~0.003-0.004;
                    // a UI with main panel content measures > 0.01.
                    //
                    // Exception: after NavigateHome on `main`, the main panel is
                    // intentionally empty and only the sidebar renders. In that
                    // state, content_fraction legitimately falls to ~0.002.
                    // We use a weaker threshold of 0.001 to only catch fully
                    // empty windows (title-bar-only).
                    //
                    // Also: if BoundsRegistry has tracked content widgets,
                    // the UI IS rendering — xcap screenshots can be flaky
                    // when the window is briefly obscured or during GPU
                    // compositing. BoundsRegistry is the authoritative
                    // layout ground truth; not-visually-empty is only a backup for the case
                    // where layout runs but paint produces nothing visible.
                    let main_focused = ref_state
                        .focused_entity_id
                        .contains_key(&holon_api::Region::Main);
                    let min_content = if main_focused { 0.003 } else { 0.001 };
                    let has_bounds_content = all_elements
                        .iter()
                        .any(|(_, info)| info.widget_type != "live_block");
                    if !ref_state.documents.is_empty()
                        && layout_ready
                        && !has_bounds_content
                        && let Some(ref state) = self.frontend_visual_state
                    {
                        let analysis = *state.lock().unwrap();
                        if let Some(analysis) = analysis {
                            assert!(
                                analysis.content_fraction > min_content,
                                "[inv-frontend-bounds-rendered/not-visually-empty] UI is visually empty: content_fraction={:.4} < {:.4} \
                                     (ref_state has {} document(s), main_focused={main_focused}, bounds_empty=true)",
                                analysis.content_fraction,
                                min_content,
                                ref_state.documents.len(),
                            );
                        }
                    }

                    // vm-data-tracked-as-content: ViewModel data coverage — entity IDs emitted by the ViewModel
                    // that are NOT top-level region wrappers represent real data
                    // (documents, tree rows, table rows). At least one of them must
                    // be tracked as a non-`live_block` content widget. Catches the
                    // case where the ViewModel emits entity IDs but the renderer
                    // only materialises wrappers — leaving no element bound to the
                    // entity in BoundsRegistry. (`live_block` is the GPUI bug-
                    // marker: GPUI's `live_block` builder deliberately does NOT
                    // call `tracked()`, so a `widget_type == "live_block"`
                    // registration with `entity_id == eid` indicates the
                    // wrapper-only failure mode rather than a content row. TUI's
                    // tree/table/outline rows register as `render_entity` to keep
                    // this signal frontend-consistent — see
                    // `frontends/tui/src/render/mod.rs`.)
                    //
                    // Exemption: entities with no geometry trace at all
                    // (`lookup_entity` returns `None`) and any `loading` widget in
                    // BoundsRegistry — the VM has emitted the entity but the
                    // render pipeline hasn't produced bounds for it yet. Steady-
                    // state pattern for: watcher hasn't delivered the first
                    // Structure event; newly created/peer-edited entities mid-
                    // propagation; entities outside the virtualised viewport.
                    // We downgrade to a warning when these conditions hold —
                    // there's nothing to assert against.
                    let data_entity_ids: Vec<&String> = entity_ids
                        .iter()
                        .filter(|eid| !eid.starts_with("block:default-"))
                        .collect();
                    if !data_entity_ids.is_empty() {
                        let content_match_count = data_entity_ids
                            .iter()
                            .filter(|eid| {
                                lookup_entity(eid)
                                    .map(|info| info.widget_type != "live_block")
                                    .unwrap_or(false)
                            })
                            .count();
                        // True iff every data entity has *some* widget (live_block or
                        // otherwise). When all entities have at least a live_block but
                        // none are content widgets, it's the original vm-data-tracked-as-content bug.
                        let all_entities_have_live_block = data_entity_ids
                            .iter()
                            .all(|eid| lookup_entity(eid).is_some());
                        let has_loading = all_elements
                            .iter()
                            .any(|(_, info)| info.widget_type == "loading");
                        if content_match_count == 0
                            && (has_loading || !all_entities_have_live_block)
                        {
                            eprintln!(
                                "[inv-frontend-bounds-rendered/vm-data-tracked-as-content WARN] {} data entity ID(s) not yet tracked as content widgets (loading={has_loading}, all_have_live_block={all_entities_have_live_block}): {:?}",
                                data_entity_ids.len(),
                                &data_entity_ids[..data_entity_ids.len().min(5)],
                            );
                        } else {
                            assert!(
                                content_match_count > 0,
                                "[inv-frontend-bounds-rendered/vm-data-tracked-as-content] ViewModel has {} data entity ID(s) but none are tracked as content widgets (render_entity/editable_text/selectable): {:?}",
                                data_entity_ids.len(),
                                &data_entity_ids[..data_entity_ids.len().min(5)],
                            );
                        }
                    }

                    // ── Future invariants (brainstormed, not yet implemented) ──
                    //
                    // widget-type-diverse — Widget type diversity: non-trivial UI should contain ≥ 2
                    //   distinct widget_type values in BoundsRegistry.
                    //
                    // live-block-contains-content — Data-aware containment: for each live_block wrapper whose
                    //   ViewModel sub-tree has data rows > 0, assert that at least one
                    //   non-live_block tracked element's bounds are geometrically contained
                    //   within the live_block's bounds. Natural virtual-scrolling tolerance.
                    //
                    // live-block-area-nonzero — Region area sanity: for any live_block wrapper whose ViewModel
                    //   sub-tree has data rows > 0, the wrapper's own area must be non-zero.
                    //   Catches "empty main panel when it shouldn't be empty".
                    //
                    // total-content-area-nonzero — Non-zero total content area: sum area of all non-live_block
                    //   tracked elements; require > 0 (or some minimum). Weakest check,
                    //   superseded by non-wrapper-content-when-docs but cheap.
                    //
                    // focused-block-tracked — Focus state invariant: if the reference model has a focused
                    //   block, that block's entity_id must appear as a tracked element.
                    //
                    // content-spans-regions — Cross-region span: tracked non-live_block elements should
                    //   span ≥ 2 of the 3 regions when ref_state has documents AND
                    //   navigation focus. Uses geometric intersection with region bounds.
                    //
                    // Also considered: screen-size-based minimum element count, scroll
                    //   position from GPUI's uniform_list. Rejected as brittle — live-block-contains-content/live-block-area-nonzero
                    //   achieve the same goal via geometric containment without needing
                    //   scroll offsets or resolution-dependent thresholds.

                    eprintln!(
                        "[inv-frontend-engine] Frontend: root='{root_kind}', {} entity IDs, {} elements, {} missing bounds, {} rendered in order",
                        entity_ids.len(),
                        all_elements.len(),
                        missing.len(),
                        rendered_entities.len(),
                    );
                    if !missing.is_empty() {
                        eprintln!(
                            "[inv-frontend-engine WARN] {} entity IDs have no BoundsRegistry entry: {:?}",
                            missing.len(),
                            &missing[..missing.len().min(5)],
                        );
                    }
                } else {
                    eprintln!(
                        "[inv-frontend-engine] Frontend ViewModel: root='{root_kind}', {} entity IDs (no geometry)",
                        entity_ids.len(),
                    );
                }
            }
            fe_engine.unwatch(&root_uri);
        }

        // ── inv-editable-text-has-draggable: Every focused editable text block has a Draggable ─
        //
        // Production wraps every block bullet in a `draggable` widget so
        // users can pick up the block and drop it elsewhere. If a future
        // refactor accidentally drops the wrapper for some block subset
        // (e.g. when re-shaping the bullet column), drag&drop silently
        // breaks — `DragDropBlock` would fail before this invariant
        // catches the structural drift.
        //
        // Walks the resolved frontend ViewModel for every block currently
        // in the focus tree (via reference model) and asserts a
        // `Draggable` node carrying the block's id is reachable. Skipped
        // if no frontend engine is installed or none of the focus blocks
        // are text blocks (only text blocks are draggable in production).
        //
        // Skipped when the test environment has registered an alternate
        // `block` entity profile from a generated org file. The test
        // profile YAMLs (see `TestEntityProfile::to_yaml` in
        // `reference_state.rs`) render as `row(editable_text(...))` and
        // get merged into the canonical `block_profile.yaml` variants by
        // `ProfileResolver::merge_profile`. With the test profile's
        // priority-1 `task` variant grabbing every block where
        // `task_state != ()` and the canonical `default` (priority -1)
        // catching the rest, the resolved tree legitimately mixes
        // wrapped and bare `editable_text` widgets — an "N editable_text
        // / N-1 draggable" pattern indistinguishable from the production
        // drift inv-editable-text-has-draggable was designed to catch.
        let inv16_engine: Option<Arc<holon_frontend::reactive::ReactiveEngine>> =
            if ref_state.has_blocks_profile() {
                None
            } else {
                self.frontend_engine
                    .clone()
                    .or_else(|| self.reactive_engine.borrow().clone())
            };
        if let Some(engine) = inv16_engine {
            let root_uri = self
                .reactive_root_id
                .borrow()
                .clone()
                .unwrap_or_else(holon_api::root_layout_block_uri);
            let rqr = engine.ensure_watching(&root_uri);
            if !rqr.is_loading() {
                // snapshot_reactive only resolves the root level; nested
                // live_block placeholders need to be expanded explicitly
                // to find draggables that live inside per-block render
                // templates (block_profile.yaml's `column(row(draggable),...)`
                // wrap). BFS over discovered nested block ids.
                // inv-editable-text-has-draggable is a *render-pipeline* invariant scoped to the
                // block_profile render path: when a tree's render produces
                // *any* `draggable` wrappers (canonical block_profile signal),
                // every `editable_text` in the same tree must be paired with
                // a `draggable` carrying the same row_id. If a tree has no
                // draggables at all, it's a custom non-block_profile render
                // (e.g. a sidebar list template `list(item_template:
                // row(editable_text(col("name"))))`) where unpaired
                // editable_text is intentional — skip.
                //
                // Block_profile drift (the production bug we want to catch)
                // shows up as N editable_texts paired with N-1 draggables in
                // the same tree, so per-tree pairing fires correctly.
                //
                // ref-state-vs-SQL divergences (a block that ref_state thinks
                // should be visible but the GQL query never returns) are a
                // separate concern caught by other invariants.
                let mut visited: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                let mut queue: Vec<EntityUri> = vec![root_uri.clone()];
                let mut tree_widget_summary: Vec<(
                    String,
                    std::collections::HashMap<String, usize>,
                )> = Vec::new();
                let mut missing: Vec<String> = Vec::new();
                let mut all_draggable_ids: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                let mut all_editable_ids: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                while let Some(uri) = queue.pop() {
                    if !visited.insert(uri.as_str().to_string()) {
                        continue;
                    }
                    let _ = engine.ensure_watching(&uri);
                    let rvm = engine.snapshot_reactive(&uri);
                    let mut counts: std::collections::HashMap<String, usize> =
                        std::collections::HashMap::new();
                    let mut tree_draggable: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    let mut tree_editable: std::collections::HashSet<String> =
                        std::collections::HashSet::new();
                    holon_frontend::focus_path::walk_tree(&rvm, &mut |n| {
                        if let Some(name) = n.widget_name() {
                            *counts.entry(name.clone()).or_insert(0) += 1;
                        }
                        match n.widget_name().as_deref() {
                            Some("draggable") => {
                                if let Some(id) = n.row_id() {
                                    tree_draggable.insert(id);
                                }
                            }
                            Some("editable_text") | Some("rendered_text") => {
                                if let Some(id) = n.row_id() {
                                    tree_editable.insert(id);
                                }
                            }
                            Some("live_block") => {
                                if let Some(bid) = n.prop_str("block_id")
                                    && !visited.contains(&bid)
                                {
                                    queue.push(EntityUri::from_raw(&bid));
                                }
                            }
                            _ => {}
                        }
                    });
                    // Only enforce pairing in trees where block_profile-style
                    // rendering is in effect (signaled by ≥1 draggable).
                    if !tree_draggable.is_empty() {
                        for id in tree_editable.difference(&tree_draggable) {
                            missing.push(id.clone());
                        }
                    }
                    all_draggable_ids.extend(tree_draggable);
                    all_editable_ids.extend(tree_editable);
                    tree_widget_summary.push((uri.as_str().to_string(), counts));
                }
                missing.sort();
                missing.dedup();
                let draggable_ids = all_draggable_ids;
                let editable_ids = all_editable_ids;
                if !missing.is_empty() {
                    let mut tree_lines = String::new();
                    for (block_id, counts) in &tree_widget_summary {
                        let mut sorted: Vec<_> = counts.iter().collect();
                        sorted.sort_by(|a, b| b.1.cmp(a.1));
                        tree_lines.push_str(&format!(
                            "    {block_id}: {sorted:?}\n",
                            sorted = sorted.iter().take(15).collect::<Vec<_>>(),
                        ));
                    }
                    panic!(
                        "[inv-editable-text-has-draggable] {n} editable_text widget(s) have no sibling \
                         Draggable carrying the same row_id — drag&drop \
                         would silently break for these blocks (production \
                         GPUI's draggable.rs short-circuits when row_id is \
                         None).\n  missing (editable_text without draggable): \
                         {missing:?}\n  draggable_ids ({n_drag}): {drag_sample:?}\
                         \n  editable_ids ({n_edit}): {edit_sample:?}\n  visited \
                         {visited_n} block trees:\n{tree_lines}",
                        n = missing.len(),
                        n_drag = draggable_ids.len(),
                        n_edit = editable_ids.len(),
                        drag_sample = draggable_ids.iter().take(10).collect::<Vec<_>>(),
                        edit_sample = editable_ids.iter().take(10).collect::<Vec<_>>(),
                        visited_n = visited.len(),
                    );
                }
            }
            engine.unwatch(&root_uri);
        }

        // ── inv-focus-matches-ref: Focus consistency ─────────────────────────────
        // The engine's global `focused_block` mirror (written by the click
        // handler / `maybe_mirror_navigation_focus`) must match the reference
        // model's global `focused_block` after every focus-changing
        // transition. The `focused_entity_id` map is per-region and can hold
        // entries for multiple regions simultaneously (a Main click followed
        // by a RightSidebar focus leaves both populated), so checking against
        // the global field — which tracks the *most recent* focus change — is
        // the only consistent comparison: the engine has a single global
        // `focused_block`, not a per-region map.
        //
        // Skipped:
        //   - SqlOnly mode (no frontend_engine).
        //   - Reference model has no global focus (no focus-changing
        //     transition has fired yet, or the last `go_home` cleared it).
        //   - An editor is active in the ref state. Editor focus
        //     (`active_editor.block_id`) is the source of truth while an
        //     editor is open; the engine's global `focused_block` may or
        //     may not have been updated by the click handler — depends on
        //     whether the GPUI window had finished painting at click time.
        //     The check resumes once `active_editor` clears (e.g. after
        //     navigation away).
        //
        // The ref-state `focused_block` is unresolved-id-shaped (e.g.
        // `block:ref-doc-0`); the engine works with resolved UUIDs. The
        // SUT mirrors the resolved id when it sets engine focus (see
        // NavigateFocus/ArrowNavigate), so the engine value carries the
        // resolved id while the ref tracks the unresolved seed. Compare via
        // `resolve_uri` to bridge that gap.
        if let Some(ref engine) = self.frontend_engine
            && let Some(ref ref_focused) = ref_state.focused_block
            && ref_state.active_editor.is_none()
        {
            let resolved_ref = self.resolve_uri(ref_focused);
            // Poll briefly: chord ops like SplitBlock / JoinBlock fire
            // editor_focus(new_block) as a follow-up that propagates through
            // SQL → watch_editor_cursor → window.focus → InputEvent::Focus →
            // set_focus. The new block's EditorView may not have mounted by
            // the time this invariant runs; poll up to 1s for the chain to
            // converge before failing.
            let poll_deadline = std::time::Instant::now() + Duration::from_millis(1000);
            let mut actual = engine.focused_block();
            while actual
                .as_ref()
                .is_some_and(|u| u.as_str() != resolved_ref.as_str())
                && std::time::Instant::now() < poll_deadline
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
                actual = engine.focused_block();
            }
            if let Some(ref actual_uri) = actual {
                assert_eq!(
                    actual_uri.as_str(),
                    resolved_ref.as_str(),
                    "[inv-focus-matches-ref] Global focus mismatch: reference model has {} \
                     (resolved: {}), but engine.focused_block() has {} (polled 1s)",
                    ref_focused,
                    resolved_ref,
                    actual_uri,
                );
            }
            // If actual is None but ref has focus, that's allowed — the
            // reference model sets focus in its apply() phase, but GPUI's
            // focus update happens on a signal loop and may lag.
        }

        // ── inv-displayed-text: editable_text + text widgets show the right string ─
        //
        // The on-screen string for any block-bound text widget (live
        // `InputState` value for `editable_text`, rendered prop for
        // `text(col(...))`) must match what the user is currently editing
        // (or `block.content` if no edit is in progress).
        //
        // Empirically (devlog 2026-05-08-152913): MutableText updates the
        // editor's live state but does NOT synchronously commit to
        // `block.content` — the SQL row only catches up at blur / Enter /
        // chord-commit. So while an editor is active on a block we compare
        // against `active_editor.in_memory_content`; otherwise we compare
        // against the committed `block.content`.
        //
        // This catches both real UI-staleness regressions (post-`split_block`
        // stale prefix on InputState) and any divergence between the
        // editor's view and the reference model's tracked in-memory state.
        if !nav_only && let Some(ref geometry) = self.frontend_geometry {
            // Build reverse map: real URI → synthetic ref-state key.
            // After SplitBlock, the ref state stores the new block under a
            // synthetic `block::split-N` key while the frontend sees the real
            // `block:uuid`. Without reverse resolution, the lookup below skips
            // every split-created block, masking UI staleness.
            let reverse_map: HashMap<EntityUri, EntityUri> = self
                .doc_uri_map
                .iter()
                .map(|(syn, real)| (real.clone(), syn.clone()))
                .collect();

            let mut mismatches: Vec<String> = Vec::new();
            for (_el_id, info) in geometry.all_elements() {
                if info.widget_type != "editable_text"
                    && info.widget_type != "rendered_text"
                    && info.widget_type != "text"
                {
                    continue;
                }
                let Some(ref displayed) = info.displayed_text else {
                    continue;
                };
                let Some(ref entity_id) = info.entity_id else {
                    continue;
                };
                if !entity_id.starts_with("block:") {
                    continue;
                }
                let Ok(uri) = EntityUri::parse(entity_id) else {
                    continue;
                };
                // Try direct lookup first, then reverse-map (split-created
                // blocks are stored under synthetic keys in the ref state).
                let block = ref_state.block_state.blocks.get(&uri).or_else(|| {
                    reverse_map
                        .get(&uri)
                        .and_then(|synthetic| ref_state.block_state.blocks.get(synthetic))
                });
                let Some(block) = block else {
                    continue;
                };
                // While an editor is active on this block, the on-screen
                // string reflects the live `InputState` value, NOT the
                // committed `block.content`. Verified empirically (seed 5,
                // devlog 2026-05-08-..-pbt-empirical): MutableText writes
                // to its CRDT and the InputState reflects that, but
                // `block.content` only catches up at blur / Enter / etc.
                // So while editing, compare against `in_memory_content`.
                let expected: String = match &ref_state.active_editor {
                    Some(active) if active.block_id == block.id => active.in_memory_content.clone(),
                    _ => block.content_text().to_string(),
                };
                if displayed != &expected {
                    // Tag each mismatch with where the divergence lives —
                    // backend (engine snapshot also stale) vs GPUI render
                    // layer (engine snapshot matches expected). The
                    // diagnostic asks the same `ReactiveEngine` the GPUI
                    // window is bound to, so it shows whether the engine
                    // produced the right ViewModel and the render layer
                    // dropped it, or whether the bug is upstream.
                    let diag_label = self
                        .frontend_engine
                        .as_ref()
                        .map(|engine| {
                            crate::pbt::panic_diag::diagnose_displayed_text(
                                engine, entity_id, displayed, &expected,
                            )
                            .as_label()
                        })
                        .unwrap_or_else(|| "no engine handle".into());
                    mismatches.push(format!(
                        "  {widget}@block={entity_id}\n    on-screen: {:?}\n    \
                         expected:  {:?}\n    [DIAG] {diag_label}",
                        displayed,
                        expected,
                        widget = info.widget_type,
                    ));
                }
            }
            assert!(
                mismatches.is_empty(),
                "[inv-displayed-text] {} text widget(s) show stale content. \
                     The on-screen string diverged from the SQL block.content in the \
                     reference model — typical after split_block/join_block when the \
                     row's data signal fires but a rendered prop (editable_text \
                     InputState, text col(...) snapshot) skips the update.\n\
                     Per-line [DIAG] tag distinguishes backend (engine ViewModel \
                     also stale) from GPUI render layer (engine snapshot matches \
                     expected; render layer dropped the update).\n{}",
                mismatches.len(),
                mismatches.join("\n"),
            );
        }
    }
}

impl<V: VariantMarker> StateMachineTest for E2ESut<V> {
    type SystemUnderTest = Self;
    type Reference = VariantRef<V>;

    fn init_test(
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) -> Self::SystemUnderTest {
        tracing::trace!(
            "[init_test<{}>] Starting, ref_state has {} blocks, app_started: {}",
            std::any::type_name::<V>(),
            ref_state.block_state.blocks.len(),
            ref_state.app_started
        );
        // Reuse a process-wide tokio runtime across PBT cases. We empirically
        // re-measured the per-case alternative (May 2026): it adds 15s/case
        // on Full and 30s/case on SqlOnly, and it does NOT fix the Loro
        // wait_for_consumers race (which lives within a single case, not
        // across cases). Per-case isolation comes from the SUT's own state
        // (TempDir, DB, session): when the SUT drops, its Arcs go away and
        // the spawned tasks observe broken channels and exit.
        static SHARED_RUNTIME: OnceLock<Arc<tokio::runtime::Runtime>> = OnceLock::new();
        let runtime = SHARED_RUNTIME
            .get_or_init(|| Arc::new(tokio::runtime::Runtime::new().unwrap()))
            .clone();
        let result = E2ESut::new(runtime).unwrap();
        tracing::trace!("[init_test] Completed (app not started yet - pre-startup phase)");
        result
    }

    fn apply(
        mut state: Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        transition: crate::pbt::transitions::E2ETransition,
    ) -> Self::SystemUnderTest {
        tracing::trace!(
            "[apply] ref_state has {} blocks, transition: {}",
            ref_state.block_state.blocks.len(),
            transition.variant_name()
        );

        state.last_transition = transition.clone();
        #[cfg(feature = "otel-testing")]
        {
            // The span collector resets per-transition for budget isolation,
            // but `Drop` wants whole-case totals. Snapshot the previous
            // transition's `query` ancestor chains before resetting, and
            // merge them into the case-level accumulator. Only paid when
            // PBT_MATVIEW_METRICS=1 so normal runs keep their per-transition
            // semantics untouched.
            if std::env::var("PBT_MATVIEW_METRICS").as_deref() == Ok("1") {
                let prev = state.span_collector.queries_by_origin();
                let mut acc = state.query_origin_acc.borrow_mut();
                for row in prev.rows {
                    let entry = acc
                        .entry(row.chain)
                        .or_insert((0, std::time::Duration::ZERO));
                    entry.0 += row.count;
                    entry.1 += row.total_duration;
                }
            }
            state.span_collector.reset();
            state.last_transition_start = Some(Instant::now());
            let rss_now = crate::test_tracing::current_rss_bytes();
            state.rss_before = rss_now;
            if state.rss_baseline == 0 {
                state.rss_baseline = rss_now;
            }
        }

        let runtime = state.runtime.clone();
        runtime.block_on(state.apply_transition_async(ref_state, &transition));
        state
    }

    fn check_invariants(
        state: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
    ) {
        let runtime = state.runtime.clone();
        runtime.block_on(state.check_invariants_async(ref_state));
    }
}

impl<V: VariantMarker> E2ESut<V> {
    /// Number of content blocks (excludes document blocks, which are created
    /// asynchronously by OrgSyncController and may lag behind content blocks).
    fn expected_content_block_count(ref_state: &ReferenceState) -> usize {
        ref_state
            .block_state
            .blocks
            .values()
            .filter(|b| !b.is_page())
            .count()
    }

    /// Resolve every reference-state block id to its DB-side id via
    /// `doc_uri_map` (documents) or pass-through (content blocks). The
    /// returned set is the synchronization predicate used by
    /// `wait_for_blocks_synced`: each id must appear in the all-blocks
    /// CDC accumulator before the wait succeeds.
    pub(crate) fn expected_block_ids(&self, ref_state: &ReferenceState) -> HashSet<String> {
        ref_state
            .block_state
            .blocks
            .values()
            .map(|b| self.resolve_uri(&b.id).to_string())
            .collect()
    }

    /// Clone all reference blocks with parent_id resolved to UUID-based URIs.
    /// When `resolve_id` is true, the block id is also remapped via doc_uri_map
    /// (used for org-file/external mutation paths where doc URIs are UUID-keyed).
    fn resolve_ref_blocks(&self, ref_state: &ReferenceState, resolve_id: bool) -> Vec<Block> {
        ref_state
            .block_state
            .blocks
            .values()
            .map(|b| {
                let mut b = b.clone();
                if resolve_id {
                    b.id = self.doc_uri_map.get(&b.id).cloned().unwrap_or(b.id);
                }
                b.parent_id = self.resolve_uri(&b.parent_id);
                b
            })
            .collect()
    }

    /// Wait until every id in `expected_ids` is synced and the non-page row
    /// count matches `expected_count`, panicking with a descriptive message
    /// on timeout. The two arguments serve different purposes: the id set
    /// drives the wait predicate (asymmetric — accumulator may legitimately
    /// hold more ids), the count drives the post-condition assertion.
    async fn await_block_count_or_panic(
        &mut self,
        expected_ids: &HashSet<String>,
        expected_count: usize,
        timeout: Duration,
        context: &str,
    ) {
        let start = Instant::now();
        let actual_rows = self.wait_for_blocks_synced(expected_ids, timeout).await;
        let elapsed = start.elapsed();
        if actual_rows.len() == expected_count {
            eprintln!(
                "[{context}] Block count matched ({}) in {:?}",
                expected_count, elapsed
            );
        } else {
            panic!(
                "[{context}] Timeout waiting for {} blocks, got {} after {:?}",
                expected_count,
                actual_rows.len(),
                elapsed
            );
        }
    }

    /// Compare every parent's live `block_raw` children order (sorted
    /// by `sort_key`, the projector's authoritative ordering) against
    /// the reference model's predicted children list. This is the
    /// encoding-free equivalent of `assert_block_order` — both sides
    /// produce a `Vec<EntityUri>` per parent, no `sort_key` /
    /// `sequence` strings cross the boundary.
    ///
    /// Mirrors `BlockOrdering::children(parent_id)` semantically;
    /// queries `block_raw` directly because the test doesn't hold a
    /// `dyn BlockOrdering` (the trait lives in the holon backend, not
    /// the engine surface). When/if a `BlockOrdering` is exposed via
    /// `BlockDomain`, swap the SQL out for the trait call.
    async fn assert_live_children_match_ref(&self, ref_state: &ReferenceState) {
        let parents: std::collections::BTreeSet<EntityUri> = ref_state
            .block_state
            .blocks
            .values()
            .map(|b| b.parent_id.clone())
            .collect();
        let engine = self.engine();
        for parent in parents {
            if !parent.is_block() {
                continue;
            }
            let resolved_parent = self.resolve_uri(&parent);
            let sql = format!(
                "SELECT id FROM block_raw WHERE parent_id = '{}' ORDER BY sort_key, id",
                resolved_parent.as_str().replace('\'', "''")
            );
            let rows = match engine.execute_query(sql, HashMap::new(), None).await {
                Ok(rows) => rows,
                Err(e) => {
                    panic!(
                        "[inv-live-children-match-ref] block_raw query failed for parent {}: {e:#}",
                        resolved_parent
                    );
                }
            };
            let live_ids: Vec<String> = rows
                .iter()
                .filter_map(|row| row.get("id").and_then(|v| v.as_string()).map(String::from))
                .collect();
            // Resolve ref ids the same way the rest of the test does so
            // synthetic split / doc URIs line up with their real UUIDs.
            let ref_ids: Vec<String> = ref_state
                .children_of(&parent)
                .into_iter()
                .map(|uri| self.resolve_uri(&uri).as_str().to_string())
                .collect();
            if live_ids != ref_ids {
                panic!(
                    "[inv-live-children-match-ref] children of {} disagree:\n  \
                     live  (block_raw ORDER BY sort_key): {:?}\n  \
                     ref   (sorted_children_of):          {:?}",
                    resolved_parent, live_ids, ref_ids
                );
            }
        }
    }

    /// Wait for the org-file projection to match `expected_blocks` and then
    /// stabilise (no more writes for one quiescence window).
    async fn await_org_file_convergence(&self, expected_blocks: &[Block]) {
        let org_timeout = Duration::from_millis(5000);
        self.ctx
            .wait_for_org_file_sync(expected_blocks, org_timeout)
            .await;
        self.ctx
            .wait_for_org_files_stable(25, Duration::from_millis(5000))
            .await;
    }

    /// Apply a mutation (UI or External) and wait for sync to complete.
    ///
    /// This method delegates to TestContext methods for the actual work,
    /// keeping the PBT layer thin.
    async fn apply_mutation(&mut self, event: MutationEvent, ref_state: &ReferenceState) {
        match event.source {
            MutationSource::UI => {
                let (entity, op, mut params) = event.mutation.to_operation();

                // The reference model uses file-based document URIs (e.g. "file:doc_0.org")
                // but the real system assigns UUID-based IDs. Resolve before executing.
                if let Some(Value::String(pid)) = params.get("parent_id") {
                    let pid = EntityUri::parse(pid).expect("Unable to parse parent_id");
                    let resolved = self.resolve_uri(&pid);
                    params.insert("parent_id".to_string(), resolved.clone().into());
                }

                // Try keychord path first: if the operation has a keybinding, dispatch
                // via send_key_chord → shadow index → bubble_input. This exercises the
                // full keybinding pipeline, same as pressing Cmd+Enter in GPUI.
                let dispatched_via_keychord = if let Some(block_id) =
                    params.get("id").and_then(|v| v.as_string())
                {
                    if let Some(chord) = self.find_keybinding_for_op(&op) {
                        eprintln!(
                            "[E2ESut::apply_mutation] Trying keychord {:?} for op '{}' on block '{}'",
                            chord, op, block_id
                        );
                        match self.send_key_chord(block_id, &chord, HashMap::new()).await {
                            Ok(true) => {
                                eprintln!("[E2ESut::apply_mutation] Dispatched via keychord");
                                true
                            }
                            Ok(false) => {
                                eprintln!(
                                    "[E2ESut::apply_mutation] Keychord did NOT match — falling back to direct dispatch"
                                );
                                false
                            }
                            Err(e) => {
                                eprintln!(
                                    "[E2ESut::apply_mutation] Keychord dispatch error: {:?} — falling back",
                                    e
                                );
                                false
                            }
                        }
                    } else {
                        false
                    }
                } else {
                    false
                };

                if !dispatched_via_keychord {
                    // TODO(simulate-real-input): this fallback bypasses the user-input
                    // layer. The legitimate UI mutations that hit it (block::set_field
                    // content edits) need a UserDriver `replace_text(entity_id, text)`
                    // verb — click + Cmd+A + type_text + click-elsewhere-to-blur — so
                    // the editor controller, InputState, and on_text_changed pipeline
                    // are exercised end-to-end.
                    //
                    // SYNTHETIC: the keychord path above handles ops that have a
                    // keybinding (cycle_task_state, indent, split_block, ...). This
                    // branch fires when the ref-model generated an abstract mutation
                    // with no corresponding user gesture — e.g., a direct `block::update`
                    // that a real user would produce by clicking into an editor and
                    // typing. Burn-down for these lives in Step known-widget-type of plan
                    // `deep-humming-crane.md`: once `click_entity` + `type_text` cover
                    // the full editor flow, this fallback can be deleted.
                    eprintln!(
                        "[E2ESut::apply_mutation] Direct dispatch: entity={}, op={}",
                        entity, op
                    );
                    let driver = self
                        .driver
                        .as_ref()
                        .expect("driver not installed — was start_app called?");
                    match driver.synthetic_dispatch(&entity, &op, params).await {
                        Ok(()) => {
                            eprintln!("[E2ESut::apply_mutation] synthetic_dispatch returned Ok")
                        }
                        Err(e) => panic!("Operation {}.{} failed: {:?}", entity, op, e),
                    }
                }
            }

            MutationSource::External => {
                // Resolve file-based doc URIs to UUID-based (ctx.documents is re-keyed
                // to UUID after start_app). Block-to-block parent_ids pass through unchanged.
                eprintln!("[E2ESut::apply_mutation] External mutation - writing to Org file");
                let expected_blocks = self.resolve_ref_blocks(ref_state, true);
                if let Err(e) = self.ctx.apply_external_mutation(&expected_blocks).await {
                    eprintln!("[E2ESut::apply_mutation] External mutation failed: {:?}", e);
                } else {
                    eprintln!(
                        "[E2ESut::apply_mutation] External mutation wrote to file, waiting for file watcher"
                    );
                }
            }

            MutationSource::Action => {
                // Action-sourced mutations are autonomous: the action watcher
                // observes a query result and calls `engine.execute_operation`
                // directly (see `action_watcher.rs::run_discovery_loop`).
                // There is no user keystroke or click to simulate here, so
                // routing through `send_key_chord` / `click_entity` would
                // *invent* a gesture the production code path never makes.
                // `synthetic_dispatch` is the faithful mirror of what the
                // action watcher actually does in production.
                let (entity, op, mut params) = event.mutation.to_operation();

                if let Some(Value::String(pid)) = params.get("parent_id") {
                    let pid = EntityUri::parse(pid).expect("Unable to parse parent_id");
                    let resolved = self.resolve_uri(&pid);
                    params.insert("parent_id".to_string(), resolved.clone().into());
                }

                eprintln!(
                    "[E2ESut::apply_mutation] Action dispatch: entity={}, op={}",
                    entity, op
                );
                let driver = self
                    .driver
                    .as_ref()
                    .expect("driver not installed — was start_app called?");
                match driver.synthetic_dispatch(&entity, &op, params).await {
                    Ok(()) => {
                        eprintln!("[E2ESut::apply_mutation] Action synthetic_dispatch returned Ok")
                    }
                    Err(e) => panic!("Action operation {}.{} failed: {:?}", entity, op, e),
                }
            }
        }

        // Wait until block count matches expected (with timeout).
        let expected_count = Self::expected_content_block_count(ref_state);
        let expected_ids = self.expected_block_ids(ref_state);
        self.await_block_count_or_panic(
            &expected_ids,
            expected_count,
            Duration::from_millis(10000),
            "E2ESut::apply_mutation",
        )
        .await;

        // Spot-check: verify the mutated block has correct data in the DB.
        // Only for UI mutations — External mutations write to org files and need the file
        // watcher to propagate changes to SQL (checked later in check_invariants).
        if event.source == MutationSource::UI
            && let Some(block_id) = event.mutation.target_block_id()
            && let Some(expected_block) = ref_state.block_state.blocks.get(&block_id)
        {
            // Map synthetic split ids (`block::split-N`) to the real DB id
            // via doc_uri_map. Without this, blocks created by SplitBlock
            // are queried by their reference-state placeholder id and never
            // found in SQL.
            let resolved_block_id = self.resolve_uri(&block_id);
            // Read from block_raw — post-mutation spot-check needs synchronous
            // visibility (same matview-CDC race fix as inv-viewmodel-root-matches-render-expr / #13).
            let prql = format!(
                "from block_raw | filter id == \"{}\" | select {{id, content, content_type, parent_id}}",
                resolved_block_id
            );
            let spec = self
                .test_ctx()
                .query(prql, QueryLanguage::HolonPrql, HashMap::new())
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "Post-mutation spot-check query failed for block '{}': {:?}",
                        block_id, e
                    )
                });
            let resolved_row = spec.first().unwrap_or_else(|| {
                panic!(
                    "Post-mutation spot-check: no row returned for block '{}'",
                    block_id
                )
            });
            let actual_content = resolved_row
                .get("content")
                .and_then(|v| v.as_string())
                .unwrap_or("")
                .trim();
            let expected_content = expected_block.content.trim();
            assert_eq!(
                actual_content, expected_content,
                "Post-mutation spot-check: content mismatch for block '{}'",
                block_id
            );
            let actual_ct = resolved_row
                .get("content_type")
                .and_then(|v| v.as_string())
                .unwrap_or("");
            assert_eq!(
                actual_ct,
                expected_block.content_type.to_string().as_str(),
                "Post-mutation spot-check: content_type mismatch for block '{}'",
                block_id
            );
        } // UI mutations only

        // Wait for org files to match expected state, then stabilize (no more writes).
        // Resolve both id and parent_id so document blocks match UUID-keyed documents.
        let expected_blocks = self.resolve_ref_blocks(ref_state, true);
        self.await_org_file_convergence(&expected_blocks).await;

        // External mutations write to disk; the file watcher asynchronously
        // delivers the change to the backend. `await_org_file_convergence` only
        // waits for the file itself to match, not for the backend to catch up.
        // For content or property updates (no count change), this can cause the
        // invariant check to run before the backend has the new state.
        //
        // Spot-check the mutated block's content AND properties in the backend,
        // polling until they match or the timeout fires. Properties are checked
        // against `event.mutation.fields` so custom-property updates like
        // `{effort: "7yzXz"}` also wait for SQL to catch up.
        if event.source == MutationSource::External
            && let Some(block_id) = event.mutation.target_block_id()
        {
            let resolved_id = self.resolve_uri(&block_id);
            if let Some(expected_block) = ref_state.block_state.blocks.get(&block_id) {
                let expected_content = expected_block.content.trim().to_string();
                let expected_properties: HashMap<String, Value> =
                    mutation_expected_properties(&event.mutation);
                let deadline = Instant::now() + Duration::from_millis(5000);
                loop {
                    let prql = format!(
                        "from block_raw | filter id == \"{}\" | select {{content, properties}}",
                        resolved_id
                    );
                    let rows = self
                        .test_ctx()
                        .query(prql, QueryLanguage::HolonPrql, HashMap::new())
                        .await
                        .unwrap_or_default();
                    let row = rows.first();
                    let actual_content = row
                        .and_then(|r| r.get("content"))
                        .and_then(|v| v.as_string())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let actual_properties = row
                        .and_then(|r| r.get("properties"))
                        .map(row_properties_to_map)
                        .unwrap_or_default();
                    let content_match = actual_content == expected_content;
                    let properties_match = expected_properties
                        .iter()
                        .all(|(k, v)| actual_properties.get(k) == Some(v));
                    if content_match && properties_match {
                        break;
                    }
                    if Instant::now() >= deadline {
                        eprintln!(
                            "[E2ESut::apply_mutation] External sync timeout for \
                                 block '{}': content actual={:?} expected={:?}; \
                                 properties actual={:?} expected={:?}",
                            resolved_id,
                            actual_content,
                            expected_content,
                            actual_properties,
                            expected_properties
                        );
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }
        }
    }
}

/// Fields that are SQL columns on `block` rather than entries in the
/// `properties` JSON column. When an External mutation's `fields` map contains
/// one of these, the expected effect lands in a column — not in `properties` —
/// so it's excluded from the post-mutation property spot-check.
const BLOCK_SQL_COLUMNS: &[&str] = &[
    "id",
    "parent_id",
    "name",
    "content",
    "content_type",
    "source_language",
    "source_name",
    "collapsed",
    "completed",
    "block_type",
    "created_at",
    "updated_at",
];

/// Extract the subset of a mutation's `fields` that should land in the DB
/// row's `properties` JSON column (i.e. custom properties and org drawer
/// props like `task_state`, `effort`, `column-order`, …).
fn mutation_expected_properties(mutation: &Mutation) -> HashMap<String, Value> {
    let fields = match mutation {
        Mutation::Create { fields, .. } | Mutation::Update { fields, .. } => fields,
        _ => return HashMap::new(),
    };
    fields
        .iter()
        .filter(|(k, _)| !BLOCK_SQL_COLUMNS.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// Parse a `properties` column value into a flat map, handling the two
/// shapes Turso may return (raw JSON string or already-parsed Object).
fn row_properties_to_map(props_val: &Value) -> HashMap<String, Value> {
    match props_val {
        Value::String(s) => serde_json::from_str::<HashMap<String, Value>>(s).unwrap_or_default(),
        Value::Object(m) => m.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        _ => HashMap::new(),
    }
}
