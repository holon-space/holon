//! `McpUserDriver` — the out-of-process rung of the driver ladder (§8.11
//! in `docs/Testing/PbtCompositionDesign.md`).
//!
//! Drives a REAL running Holon app (iOS simulator, desktop GPUI, …)
//! through its embedded MCP server over streamable HTTP. The MCP tool
//! handlers (`frontends/mcp/src/tools.rs`) terminate in the exact same
//! `debug.user_driver` (`GpuiUserDriver`) / `debug.input_router` that the
//! in-process ladder rungs use, so a gesture sent through this driver
//! traverses the production input pipeline of the live app:
//!
//! | `UserDriver` verb                | MCP tool                                    |
//! |----------------------------------|---------------------------------------------|
//! | `synthetic_dispatch`             | `execute_operation`                         |
//! | `click_entity`                   | `click { entity_id, region }`               |
//! | `send_key_chord`                 | `send_key_chord { entity_id, keys }`        |
//! | `send_raw_keystroke`             | `type_text { text, modifiers }`             |
//! | `insert_text`                    | `insert_text { text }`                      |
//! | `scroll_at` / `scroll_entity`    | `scroll { x, y / entity_id, dx, dy }`       |
//! | `displayed_text` etc. (observe)  | `describe_ui { block_id, format: "json" }`  |
//!
//! # Observation model
//!
//! `UserDriver`'s observation verbs are synchronous point-in-time reads,
//! but every MCP read is an async network round-trip. The driver bridges
//! that with an explicit snapshot cache: call
//! [`McpUserDriver::refresh_ui`] (async, fetches `describe_ui` as JSON and
//! deserializes the server's `ViewModel`) and the sync verbs read the
//! cached tree. Reading before any refresh panics — a stale-or-missing
//! snapshot must never silently answer "not visible".
//!
//! # Honestly unsupported verbs (fail loud, never fake)
//!
//! - `is_in_region` / `entities_in_region` / `reachable_entities_in_region`
//!   — `describe_ui` renders one block tree with no region/panel geometry,
//!   so region membership is not derivable. These panic with instructions.
//! - `click_intent_of` — the serialized `ViewModel` does not carry the
//!   resolved click intent (that lives in the reactive tree in-process).
//!   Panics.
//! - `scroll_to_entity` — needs viewport geometry the MCP surface doesn't
//!   expose; bails. Use `scroll_entity` / `scroll_at` with explicit deltas.
//! - `drop_entity` — no MCP drag tool; bails.
//! - `send_key_chord` with non-empty `extra_params` — the MCP tool has no
//!   extra-params channel; bails rather than dropping them.
//!
//! # Remaining work to promote this rung to the full generated PBT loop
//!
//! 1. **Per-case reset**: proptest cases need a fresh SUT. Either an MCP
//!    `reset_vault` op on the server, or `simctl terminate/launch` around
//!    each case (slow; batch via `--test-threads=1` + case budget).
//! 2. **Wiring actor**: add an `McpUserDriver` variant to the composed
//!    SUT's `any_valid_wiring()` axis so the keystone PBT can draw it,
//!    gated on a live-server env var (absent ⇒ the wiring is not drawn,
//!    not silently skipped).
//! 3. **Cap-narrowing**: invariants that read in-process handles (Loro
//!    docs, Turso conns) can't hold them across a process boundary.
//!    Follow the E3 pattern: narrow the wiring's cap set to what the MCP
//!    surface honestly provides (`execute_query` / `execute_raw_sql` /
//!    `inspect_loro_blocks` / `diff_loro_sql`) and let the runner skip —
//!    disclosed, per cap — invariants whose caps the wiring lacks.
//! 4. **Region-aware observation**: extend `describe_ui` (or add a
//!    sibling tool) to report panel/region membership so the three
//!    region verbs above stop panicking.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{Context, Result, anyhow, bail};
use rmcp::RoleClient;
use rmcp::model::CallToolRequestParam;
use rmcp::service::Peer;

use holon_api::{EntityUri, Key, KeyChord, Value};
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::user_driver::UserDriver;
use holon_frontend::view_model::{ViewKind, ViewModel};
use holon_mcp_client::{McpRunningService, connect_mcp};

/// Port the embedded MCP server listens on when `MCP_SERVER_PORT` is unset.
/// The iOS simulator shares the host loopback, so the same address reaches
/// a sim-hosted app and a desktop app alike.
pub const DEFAULT_MCP_PORT: u16 = 8521;

/// Base URL of the app's MCP endpoint, from `MCP_SERVER_PORT` (default
/// [`DEFAULT_MCP_PORT`]).
pub fn mcp_base_url_from_env() -> Result<String> {
    let port = match std::env::var("MCP_SERVER_PORT") {
        Ok(raw) => raw
            .parse::<u16>()
            .with_context(|| format!("MCP_SERVER_PORT is not a valid port: {raw:?}"))?,
        Err(std::env::VarError::NotPresent) => DEFAULT_MCP_PORT,
        Err(e) => return Err(anyhow!("MCP_SERVER_PORT is not readable: {e}")),
    };
    Ok(format!("http://127.0.0.1:{port}/mcp"))
}

/// Drives a live Holon app through its embedded MCP server. See module docs
/// for the verb→tool mapping and the honesty contract.
pub struct McpUserDriver {
    peer: Peer<RoleClient>,
    /// Keeps the streamable-HTTP session alive; dropping it closes the
    /// connection.
    _service: McpRunningService,
    /// Last `describe_ui` snapshot (see module docs, "Observation model").
    ui_snapshot: Mutex<Option<ViewModel>>,
}

impl McpUserDriver {
    /// Connect to `base_url` (e.g. `http://127.0.0.1:8521/mcp`). Performs
    /// the MCP initialize handshake.
    pub async fn connect(base_url: &str) -> Result<Self> {
        let (peer, service) = connect_mcp(base_url, None).await.with_context(|| {
            format!("MCP handshake with {base_url} failed — is the app running and serving MCP?")
        })?;
        Ok(Self {
            peer,
            _service: service,
            ui_snapshot: Mutex::new(None),
        })
    }

    /// Connect using `MCP_SERVER_PORT` (default [`DEFAULT_MCP_PORT`]).
    pub async fn connect_from_env() -> Result<Self> {
        Self::connect(&mcp_base_url_from_env()?).await
    }

    /// Call an MCP tool and return the concatenated text content.
    /// Fails loud on transport errors and on `is_error` results.
    pub async fn call_tool_text(&self, name: &str, args: serde_json::Value) -> Result<String> {
        let serde_json::Value::Object(arguments) = args else {
            bail!("MCP tool arguments must be a JSON object, got: {args}");
        };
        let result = self
            .peer
            .call_tool(CallToolRequestParam {
                name: std::borrow::Cow::Owned(name.to_string()),
                arguments: Some(arguments),
            })
            .await
            .with_context(|| format!("MCP call_tool '{name}' failed"))?;
        let text: String = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("\n");
        if result.is_error == Some(true) {
            bail!("MCP tool '{name}' returned error: {text}");
        }
        Ok(text)
    }

    /// Call an MCP tool and parse its text content as JSON.
    pub async fn call_tool_json(
        &self,
        name: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let text = self.call_tool_text(name, args).await?;
        serde_json::from_str(&text)
            .with_context(|| format!("MCP tool '{name}' returned non-JSON text: {text}"))
    }

    /// Fetch `describe_ui` for `root_block_id` as JSON, deserialize the
    /// server's `ViewModel`, cache it for the sync observation verbs, and
    /// return it.
    pub async fn refresh_ui(&self, root_block_id: &EntityUri) -> Result<ViewModel> {
        let text = self
            .call_tool_text(
                "describe_ui",
                serde_json::json!({ "block_id": root_block_id.to_string(), "format": "json" }),
            )
            .await?;
        let tree: ViewModel = serde_json::from_str(&text).with_context(|| {
            format!("describe_ui({root_block_id}) JSON did not deserialize as ViewModel: {text}")
        })?;
        *self.ui_snapshot.lock().expect("ui_snapshot poisoned") = Some(tree.clone());
        Ok(tree)
    }

    /// Run raw SQL on the live app and return the rows JSON. Read side of
    /// the smoke rung's assertions (and, later, the full loop's caps).
    pub async fn execute_raw_sql(&self, sql: &str) -> Result<serde_json::Value> {
        self.call_tool_json("execute_raw_sql", serde_json::json!({ "sql": sql }))
            .await
    }

    fn with_snapshot<T>(&self, verb: &str, f: impl FnOnce(&ViewModel) -> T) -> T {
        let guard = self.ui_snapshot.lock().expect("ui_snapshot poisoned");
        let tree = guard.as_ref().unwrap_or_else(|| {
            panic!(
                "McpUserDriver::{verb} read before any UI snapshot was taken — \
                 call refresh_ui(root_block_id).await first (observation over MCP \
                 is an explicit async fetch, see module docs)"
            )
        });
        f(tree)
    }
}

/// Wire name for a [`Key`], matching `parse_key` in `frontends/mcp/src/tools.rs`.
fn key_wire_name(key: &Key) -> String {
    match key {
        Key::Cmd => "cmd".into(),
        Key::Ctrl => "ctrl".into(),
        Key::Alt => "alt".into(),
        Key::Shift => "shift".into(),
        Key::Up => "up".into(),
        Key::Down => "down".into(),
        Key::Left => "left".into(),
        Key::Right => "right".into(),
        Key::Home => "home".into(),
        Key::End => "end".into(),
        Key::PageUp => "pageup".into(),
        Key::PageDown => "pagedown".into(),
        Key::Tab => "tab".into(),
        Key::Enter => "enter".into(),
        Key::Backspace => "backspace".into(),
        Key::Delete => "delete".into(),
        Key::Escape => "escape".into(),
        Key::Space => "space".into(),
        // Server-side parse_key lowercases before matching, so send lowercase.
        Key::Char(c) => c.to_lowercase().collect(),
        Key::F(n) => format!("f{n}"),
    }
}

fn find_node<'a>(node: &'a ViewModel, target: &EntityUri) -> Option<&'a ViewModel> {
    if node.entity_id().as_ref() == Some(target) {
        return Some(node);
    }
    node.children().iter().find_map(|c| find_node(c, target))
}

fn first_text(node: &ViewModel) -> Option<String> {
    match &node.kind {
        ViewKind::EditableText { content, .. }
        | ViewKind::RenderedText { content, .. }
        | ViewKind::Text { content, .. } => Some(content.clone()),
        _ => node.children().iter().find_map(first_text),
    }
}

#[async_trait::async_trait]
impl UserDriver for McpUserDriver {
    async fn synthetic_dispatch(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()> {
        let json_params: serde_json::Map<String, serde_json::Value> = params
            .into_iter()
            .map(|(k, v)| (k, serde_json::Value::from(v)))
            .collect();
        self.call_tool_text(
            "execute_operation",
            serde_json::json!({
                "entity_name": entity,
                "operation": op,
                "params": json_params,
            }),
        )
        .await
        .with_context(|| format!("execute_operation {entity}.{op} over MCP failed"))?;
        Ok(())
    }

    /// The chord is resolved and dispatched by the LIVE app's own
    /// `input_router` — `root_tree` (the caller's local view of the tree)
    /// is intentionally unused, the server bubbles through its real tree.
    async fn send_key_chord(
        &self,
        _: &EntityUri,
        _: &ReactiveViewModel,
        entity_id: &EntityUri,
        chord: &KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool> {
        if !extra_params.is_empty() {
            bail!(
                "McpUserDriver::send_key_chord got extra_params {:?}, but the MCP \
                 send_key_chord tool has no extra-params channel — refusing to drop \
                 them silently. Extend frontends/mcp SendKeyChordParams if a \
                 transition needs this.",
                extra_params.keys().collect::<Vec<_>>()
            );
        }
        let keys: Vec<String> = chord.0.iter().map(key_wire_name).collect();
        let response = self
            .call_tool_json(
                "send_key_chord",
                serde_json::json!({ "entity_id": entity_id.to_string(), "keys": keys }),
            )
            .await?;
        if response.get("matched_operation").is_some() {
            return Ok(true);
        }
        match response.get("action").and_then(|a| a.as_str()) {
            Some("focus") | Some("handled") => Ok(true),
            Some("none") => Ok(false),
            other => bail!(
                "send_key_chord: unexpected MCP response shape (action={other:?}): {response}"
            ),
        }
    }

    async fn click_entity(&self, entity_id: &EntityUri, region: &str) -> Result<()> {
        self.call_tool_text(
            "click",
            serde_json::json!({ "entity_id": entity_id.to_string(), "region": region }),
        )
        .await
        .with_context(|| format!("click({entity_id}, {region}) over MCP failed"))?;
        Ok(())
    }

    /// Screen-driver semantics: the live app's click handler resolves the
    /// intent itself; we can't synchronously prove which intent fired, so
    /// return `false` (same contract as `GpuiUserDriver`).
    async fn click_entity_with_tree(
        &self,
        _: &EntityUri,
        _: &ReactiveViewModel,
        entity_id: &EntityUri,
        region: &str,
    ) -> Result<bool> {
        self.click_entity(entity_id, region).await?;
        Ok(false)
    }

    /// Routes through the MCP `type_text` tool, which forwards each
    /// keystroke as a real `InteractionEvent::KeyDown` into the live GPUI
    /// window — the production input pipeline, including chord resolution.
    async fn send_raw_keystroke(&self, keystroke: &str, modifiers: &[&str]) -> Result<()> {
        self.call_tool_text(
            "type_text",
            serde_json::json!({ "text": keystroke, "modifiers": modifiers }),
        )
        .await
        .with_context(|| format!("type_text({keystroke:?}, {modifiers:?}) over MCP failed"))?;
        Ok(())
    }

    /// Routes through the MCP `insert_text` tool, which injects an
    /// `InteractionEvent::InsertText` — the soft-keyboard `insertText:` path
    /// (bypasses the keymap, commits into the focused editor's input handler).
    /// Mirrors `send_raw_keystroke` but for the UIKit text-input rung.
    async fn insert_text(&self, text: &str) -> Result<()> {
        self.call_tool_text("insert_text", serde_json::json!({ "text": text }))
            .await
            .with_context(|| format!("insert_text({text:?}) over MCP failed"))?;
        Ok(())
    }

    /// `type_text` keystrokes reach the window's real input pipeline, which
    /// performs chord resolution before any editor sees the key.
    fn dispatches_chords_via_raw_keystroke(&self) -> bool {
        true
    }

    async fn scroll_at(&self, x: f32, y: f32, dx: f32, dy: f32) -> Result<()> {
        self.call_tool_text(
            "scroll",
            serde_json::json!({ "x": x, "y": y, "dx": dx, "dy": dy }),
        )
        .await
        .context("scroll(x, y) over MCP failed")?;
        Ok(())
    }

    async fn scroll_entity(&self, entity_id: &EntityUri, dx: f32, dy: f32) -> Result<()> {
        self.call_tool_text(
            "scroll",
            serde_json::json!({ "entity_id": entity_id.to_string(), "dx": dx, "dy": dy }),
        )
        .await
        .with_context(|| format!("scroll({entity_id}) over MCP failed"))?;
        Ok(())
    }

    async fn scroll_to_entity(&self, entity_id: &EntityUri) -> Result<()> {
        bail!(
            "McpUserDriver cannot scroll_to_entity({entity_id}): scrolling toward an \
             off-viewport target needs geometry the MCP surface doesn't expose. \
             Use scroll_entity/scroll_at with explicit deltas, or extend the MCP \
             scroll tool with a scroll-into-view mode."
        )
    }

    async fn drop_entity(
        &self,
        _: &EntityUri,
        source_id: &EntityUri,
        target_id: &EntityUri,
    ) -> Result<bool> {
        bail!(
            "McpUserDriver cannot drag {source_id} onto {target_id}: the MCP surface \
             has no drag/drop tool. Add one (routing through GpuiUserDriver::drop_entity) \
             before generating drag transitions for this rung."
        )
    }

    // ── Observation verbs — answered from the cached describe_ui snapshot
    //    (call `refresh_ui` first; see module docs) ────────────────────────

    fn is_widget_visible(&self, entity_id: &EntityUri) -> bool {
        self.with_snapshot("is_widget_visible", |tree| {
            find_node(tree, entity_id).is_some()
        })
    }

    fn displayed_text(&self, entity_id: &EntityUri) -> Option<String> {
        self.with_snapshot("displayed_text", |tree| {
            find_node(tree, entity_id).and_then(first_text)
        })
    }

    fn is_in_region(&self, entity_id: &EntityUri, region: holon_api::Region) -> bool {
        panic!(
            "McpUserDriver::is_in_region({entity_id}, {region:?}) is not honestly \
             answerable: describe_ui renders one block tree with no region/panel \
             attribution. Extend describe_ui with region info before using \
             region-aware observation on this rung (module docs, remaining work #4)."
        )
    }

    fn entities_in_region(&self, region: holon_api::Region) -> Vec<EntityUri> {
        panic!(
            "McpUserDriver::entities_in_region({region:?}) is not honestly answerable \
             over the current MCP surface (module docs, remaining work #4)."
        )
    }

    fn reachable_entities_in_region(&self, region: holon_api::Region) -> Vec<EntityUri> {
        panic!(
            "McpUserDriver::reachable_entities_in_region({region:?}) is not honestly \
             answerable over the current MCP surface (module docs, remaining work #4)."
        )
    }

    fn click_intent_of(&self, entity_id: &EntityUri) -> Option<OperationIntent> {
        panic!(
            "McpUserDriver::click_intent_of({entity_id}) is not honestly answerable: \
             the serialized describe_ui ViewModel does not carry resolved click \
             intents (they live in the app's reactive tree). Generators on this rung \
             must not condition on click intents until describe_ui exposes them."
        )
    }
}
