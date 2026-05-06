//! GPUI `UserDriver` — dispatches UI mutations via the MCP
//! `interaction_tx` channel, which the GPUI window drains into real
//! `PlatformInput` events.
//!
//! This driver never touches the host cursor and works regardless of
//! whether the window is visible, minimized, or on another Space. It is
//! the driver that MCP tools inject into — see `setup_interaction_pump`
//! in this crate's `lib.rs`.
//!
//! `*_entity` methods look up the element's screen position via the
//! injected `GeometryProvider` (backed by `BoundsRegistry`) and delegate
//! to the corresponding coordinate-based variant. **Bounds-missing is a
//! hard error**: the `BoundsRegistry` double-buffers staged → committed
//! per render pass, so interacting with a just-created element before
//! the next `begin_pass` legitimately returns `None`. Tests must call
//! `holon_integration_tests::polling::wait_for_element_bounds` after
//! structural mutations; drivers fail loud instead of synthesizing
//! dispatches that bypass the input path.
//!
//! Current constraint: `element_center` only resolves `block:`-style
//! entity ids (it prepends `render-entity-`). Non-entity user-verb targets
//! are unsupported by this driver.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::channel::mpsc::Sender;
use holon_api::{KeyChord, Value};
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive::{BuilderServices, ReactiveEngine};
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::user_driver::UserDriver;
use holon_mcp::server::{InteractionCommand, InteractionEvent, InteractionResponse};

/// Channel-based `UserDriver` for GPUI. Sends `InteractionCommand`s on
/// the shared `interaction_tx` channel; the GPUI interaction pump drains
/// them on the main thread and dispatches real `PlatformInput` events
/// against the window.
pub struct GpuiUserDriver {
    tx: Sender<InteractionCommand>,
    geometry: Arc<dyn GeometryProvider>,
    engine: Arc<ReactiveEngine>,
}

impl GpuiUserDriver {
    pub fn new(
        tx: Sender<InteractionCommand>,
        geometry: Arc<dyn GeometryProvider>,
        engine: Arc<ReactiveEngine>,
    ) -> Self {
        Self {
            tx,
            geometry,
            engine,
        }
    }

    /// Send an `InteractionEvent` on the channel and await the pump's
    /// oneshot response.
    async fn dispatch_event(&self, event: InteractionEvent) -> Result<InteractionResponse> {
        let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
        self.tx
            .clone()
            .try_send(InteractionCommand {
                event,
                response_tx: resp_tx,
            })
            .map_err(|e| anyhow::anyhow!("interaction channel send failed: {e}"))?;
        resp_rx
            .await
            .context("GPUI interaction pump dropped the response channel")
    }

    /// Look up an element's window-space center from the `GeometryProvider`.
    ///
    /// Current constraint: only resolves `block:`-prefixed entity ids. The
    /// debug_assert guards against non-block entity ids silently resolving
    /// to `None` (which would look identical to an un-rendered block).
    ///
    /// Lookup chain: `render-entity-{id}` → `selectable-{id}` → entity_id
    /// scan. The default `index.org` sidebar wraps each row in
    /// `selectable(row(...))` directly, with no outer `render_entity()`,
    /// so sidebar rows register under `selectable-{id}`. Without that
    /// second alias, `click_entity` on a sidebar item would always miss.
    fn element_center(&self, entity_id: &str) -> Option<(f32, f32)> {
        debug_assert!(
            entity_id.starts_with("block:") || !entity_id.contains(':'),
            "GpuiUserDriver only supports block-scoped entity ids; got {entity_id:?}"
        );
        for el_id in [
            format!("render-entity-{entity_id}"),
            format!("selectable-{entity_id}"),
        ] {
            if let Some(info) = self.geometry.element_info(&el_id) {
                return Some(info.center());
            }
        }
        self.geometry
            .find_by_entity_id(entity_id)
            .map(|info| info.center())
    }

    /// Fail-loud bounds lookup used by the user-verb methods. Returns an
    /// error with enough context for test authors to understand whether
    /// the element was never rendered or simply hasn't been promoted from
    /// the staged buffer yet.
    fn require_element_center(&self, entity_id: &str, verb: &str) -> Result<(f32, f32)> {
        self.element_center(entity_id).with_context(|| {
            format!(
                "GpuiUserDriver::{verb}: no bounds recorded for entity {entity_id:?} — \
                 element not rendered, or BoundsRegistry hasn't promoted staged → committed \
                 since it was added. Tests should call \
                 `holon_integration_tests::polling::wait_for_element_bounds` before \
                 driving input on a freshly-rendered element."
            )
        })
    }
}

#[async_trait]
impl UserDriver for GpuiUserDriver {
    async fn synthetic_dispatch(
        &self,
        entity: &str,
        op: &str,
        params: HashMap<String, Value>,
    ) -> Result<()> {
        // Inline-dispatch into the reactive engine. No channel dispatch
        // because this is the synthetic path; when callers want the real
        // click pipeline they go through `click_entity` / `send_key_chord`.
        let intent = OperationIntent::new(entity.into(), op.into(), params);
        self.engine.dispatch_intent_sync(intent).await
    }

    /// Focus the target via a real mouse click dispatched on the
    /// interaction channel. Fails loud when geometry isn't available —
    /// see module-level doc for the rationale and the
    /// `wait_for_element_bounds` remedy. The `region` arg matches the
    /// trait signature; the GPUI driver synthesizes a real mouse event,
    /// so the region is implicit in the click coordinates and the arg is
    /// unused here.
    #[tracing::instrument(skip(self), name = "GpuiUserDriver.click_entity", fields(%entity_id))]
    async fn click_entity(&self, entity_id: &str, _: &str) -> Result<()> {
        let (cx, cy) = self.require_element_center(entity_id, "click_entity")?;
        self.dispatch_event(InteractionEvent::MouseClick {
            position: (cx, cy),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(())
    }

    /// Focus the target via click, then dispatch each character of
    /// `text` as a keystroke through the interaction channel. Mirrors
    /// MCP's `type_text` tool so both paths exercise the same pipeline.
    /// Fails loud when bounds aren't available.
    async fn type_text(&self, entity_id: &str, text: &str) -> Result<()> {
        let (cx, cy) = self.require_element_center(entity_id, "type_text")?;

        self.dispatch_event(InteractionEvent::MouseClick {
            position: (cx, cy),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;

        for ch in text.chars() {
            self.dispatch_event(InteractionEvent::KeyDown {
                keystroke: ch.to_string(),
                modifiers: Vec::new(),
            })
            .await?;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(())
    }

    /// Focus the entity via click, then press the chord keys through
    /// the interaction channel. Modifier keys are pressed, the regular
    /// keys are clicked, and modifiers are released in reverse.
    ///
    /// `extra_params["position"]` (when present) is treated as a desired
    /// cursor byte offset that a real user would have set up before
    /// pressing the chord (today: `split_block`). We click to focus, then
    /// emit a `home` keystroke and `position` `right` keystrokes through
    /// the real `PlatformInput` pipeline so the focused `InputState`'s
    /// cursor lands at the requested offset. The production chord handler
    /// (`EditorView`'s capture-phase `Enter`) then reads that cursor byte
    /// itself — there is no server-side injection.
    ///
    /// Caveat: `right` advances by a grapheme boundary in `InputState`,
    /// not by raw bytes. ASCII content means byte == grapheme; non-ASCII
    /// content can land the cursor a few bytes off. Tests that need
    /// byte-exact placement on multi-byte content must either generate
    /// ASCII or read back the actual cursor byte after positioning.
    ///
    /// Bounds-missing is a hard error. See module-level doc.
    async fn send_key_chord(
        &self,
        _: &str,
        _: &ReactiveViewModel,
        entity_id: &str,
        chord: &KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool> {
        let (cx, cy) = self.require_element_center(entity_id, "send_key_chord")?;
        self.dispatch_event(InteractionEvent::MouseClick {
            position: (cx, cy),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Position the cursor with real input when the caller specified a
        // byte offset. Click lands the cursor somewhere in the line; we
        // re-anchor with `home` and then advance with `right`. This goes
        // through `PlatformInput` and `InputState::move_to_*`, so any
        // bug in cursor handling surfaces here just like in production.
        if let Some(Value::Integer(target)) = extra_params.get("position") {
            let target = (*target).max(0) as usize;
            self.dispatch_event(InteractionEvent::KeyDown {
                keystroke: "home".into(),
                modifiers: Vec::new(),
            })
            .await?;
            for _ in 0..target {
                self.dispatch_event(InteractionEvent::KeyDown {
                    keystroke: "right".into(),
                    modifiers: Vec::new(),
                })
                .await?;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let (modifiers, regulars): (Vec<_>, Vec<_>) =
            chord.0.iter().cloned().partition(is_modifier);

        let mod_names: Vec<String> = modifiers
            .iter()
            .filter_map(|k| modifier_name(k).map(str::to_string))
            .collect();

        for key in &regulars {
            let Some(name) = keystroke_name(key) else {
                continue;
            };
            self.dispatch_event(InteractionEvent::KeyDown {
                keystroke: name,
                modifiers: mod_names.clone(),
            })
            .await?;
        }

        tokio::time::sleep(Duration::from_millis(30)).await;
        Ok(true)
    }

    /// Turn the scroll wheel at a window coordinate via the interaction
    /// channel. `dx` / `dy` are line-based deltas (positive `dy` = down).
    async fn scroll_at(&self, x: f32, y: f32, dx: f32, dy: f32) -> Result<()> {
        self.dispatch_event(InteractionEvent::ScrollWheel {
            position: (x, y),
            delta: (dx, dy),
            modifiers: Vec::new(),
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;
        Ok(())
    }

    /// Scroll over an entity — looks up its window-space center via the
    /// `GeometryProvider` and delegates to `scroll_at`. Fails loud when
    /// bounds aren't available; MCP clients now receive an error
    /// instead of a silent no-op (observable behavior change).
    async fn scroll_entity(&self, entity_id: &str, dx: f32, dy: f32) -> Result<()> {
        let (cx, cy) = self.require_element_center(entity_id, "scroll_entity")?;
        self.scroll_at(cx, cy, dx, dy).await
    }

    /// Drag the source block onto the target via real pointer events:
    /// `MouseDown(source)` → several `MouseMove(…, pressed=Left)` past
    /// GPUI's drag threshold → `MouseUp(target)`. The window's input
    /// pump turns each into a `PlatformInput`; GPUI populates
    /// `cx.active_drag` from the draggable's `on_drag` closure on the
    /// first qualifying move, and the drop_zone's `on_drop` closure
    /// fires on `MouseUp` over the target. Fails loud when either
    /// element's bounds aren't available — see module-level doc.
    async fn drop_entity(&self, _: &str, source_id: &str, target_id: &str) -> Result<bool> {
        let (sx, sy) = self.require_element_center(source_id, "drop_entity (source)")?;
        let (tx, ty) = self.require_element_center(target_id, "drop_entity (target)")?;

        self.dispatch_event(InteractionEvent::MouseDown {
            position: (sx, sy),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(20)).await;

        // Step the cursor toward the target in increments. Each step must
        // exceed GPUI's drag threshold (~5 logical px) for the drag state
        // to engage, so we use 5 small steps with `pressed_button=Left`.
        let steps = 5;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            let mx = sx + (tx - sx) * t;
            let my = sy + (ty - sy) * t;
            self.dispatch_event(InteractionEvent::MouseMove {
                position: (mx, my),
                pressed_button: Some("left".into()),
                modifiers: Vec::new(),
            })
            .await?;
            tokio::time::sleep(Duration::from_millis(15)).await;
        }

        self.dispatch_event(InteractionEvent::MouseUp {
            position: (tx, ty),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;
        tokio::time::sleep(Duration::from_millis(30)).await;
        Ok(true)
    }

    async fn send_raw_keystroke(&self, keystroke: &str, modifiers: &[&str]) -> Result<()> {
        let response = self
            .dispatch_event(InteractionEvent::KeyDown {
                keystroke: keystroke.to_string(),
                modifiers: modifiers.iter().map(|s| s.to_string()).collect(),
            })
            .await?;
        // GPUI has no leader-mode tracker, so chord prefixes like " "+"h"
        // (leader+h → go_home) leave `dispatch_keystroke` returning false.
        // Surfacing that as Err is what lets `send_leader_chord` fall back
        // to `synthetic_dispatch` — the alternative was masking the no-op
        // and shipping a stale navigation_history through PBT invariants.
        if !response.handled {
            anyhow::bail!(
                "GPUI keystroke not consumed: keystroke={keystroke:?} \
                 modifiers={modifiers:?}{detail}",
                detail = match &response.detail {
                    Some(d) => format!(" (detail: {d})"),
                    None => String::new(),
                },
            );
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
        Ok(())
    }

    /// Tree-aware click — for the screen driver this is just `click_entity`.
    /// The bound click handler reads `click_intent` itself at the rendered
    /// widget, so dispatching the right intent is the click handler's job,
    /// not the test driver's. Returning `false` reflects that the driver
    /// can't synchronously prove which intent fired.
    async fn click_entity_with_tree(
        &self,
        _: &str,
        _: &ReactiveViewModel,
        entity_id: &str,
        region: &str,
    ) -> Result<bool> {
        self.click_entity(entity_id, region).await?;
        Ok(false)
    }

    // ── Observation verbs ──────────────────────────────────────────────
    //
    // Screen-mode answers via the BoundsRegistry. For "what's on screen
    // right now" (visible, in-region, displayed text) this is the only
    // honest source: the registry records what the prepaint pipeline
    // actually placed, post-layout, post-cull. For "what could the user
    // reach" (reachable set, click intent) we read the engine's
    // ReactiveViewModel snapshot — same source the production click /
    // list renders consult.

    fn is_widget_visible(&self, entity_id: &str) -> bool {
        self.geometry
            .find_by_entity_id(entity_id)
            .filter(|info| info.area() > 0.0)
            .is_some()
    }

    fn is_in_region(&self, entity_id: &str, region: holon_api::Region) -> bool {
        self.entities_in_region(region)
            .iter()
            .any(|uri| uri.as_str() == entity_id)
    }

    /// Walk the BoundsRegistry parent-id chain from every tracked element
    /// up to a `live_block` widget bound to the panel's URI. Elements that
    /// terminate there are in-region. Mirrors
    /// `crates/holon-integration-tests/src/pbt/live_geometry.rs::rendered_entity_ids_in_panel`
    /// but reads from this driver's own `GeometryProvider` so it doesn't
    /// require the PBT-only static.
    fn entities_in_region(&self, region: holon_api::Region) -> Vec<holon_api::EntityUri> {
        let panel_id = region_panel_block_id(region);
        let elements: HashMap<String, holon_frontend::geometry::ElementInfo> =
            self.geometry.all_elements().into_iter().collect();

        let panel_ids: std::collections::HashSet<String> = elements
            .iter()
            .filter(|(_, info)| {
                info.widget_type == "live_block" && info.entity_id.as_deref() == Some(panel_id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        if panel_ids.is_empty() {
            return Vec::new();
        }

        let mut result: std::collections::HashSet<String> = std::collections::HashSet::new();
        for info in elements.values() {
            let Some(eid) = info.entity_id.as_deref() else {
                continue;
            };
            if eid == panel_id {
                continue;
            }
            let mut cursor = info.parent_id.clone();
            let mut depth = 0;
            while let Some(p) = cursor {
                if panel_ids.contains(&p) {
                    result.insert(eid.to_string());
                    break;
                }
                depth += 1;
                if depth > 100 {
                    break;
                }
                cursor = elements.get(&p).and_then(|i| i.parent_id.clone());
            }
        }
        // Drop unparseable ids: BoundsRegistry holds anything the builder
        // tagged (e.g. raw row ids without a scheme), but the trait
        // contract is `Vec<EntityUri>`. Items that aren't valid URIs
        // wouldn't be addressable anyway.
        result
            .into_iter()
            // ALLOW(filter_map_ok): drop non-URI entity ids — see comment above
            .filter_map(|id| holon_api::EntityUri::parse(&id).ok()) // ALLOW(ok): same
            .collect()
    }

    /// The full data set the user could reach by scrolling — read from
    /// the engine's snapshot of the panel block, walking the resulting
    /// `ReactiveViewModel` until we hit its `collection` (the
    /// `ReactiveView` backing the `list(...)`), then enumerating its
    /// items. This is the same data source the panel's `ReactiveShell`
    /// itself iterates, so the answer is consistent with what
    /// `ListState::scroll_to_reveal_item(ix)` can address.
    fn reachable_entities_in_region(&self, region: holon_api::Region) -> Vec<holon_api::EntityUri> {
        let panel_uri = holon_api::EntityUri::from_raw(region_panel_block_id(region));
        let root = self.engine.snapshot_reactive(&panel_uri);
        let mut out = Vec::new();
        collect_collection_item_ids(&root, &mut out);
        out
    }

    /// Scroll the panel's virtualized list (sidebar list, primarily) so
    /// the named `entity_id` enters the viewport. Block-mode panels have
    /// all rendered entities in BoundsRegistry regardless of viewport (no
    /// virtualization), so this RPC is effectively only meaningful for
    /// `gpui::list(...)`-backed shells.
    ///
    /// Returns `Ok(())` when the scroll handler succeeded OR when the
    /// entity wasn't in any virtualized list (caller — `wait_for_entity_bounds`
    /// — keeps polling and lets the timeout be the authoritative failure
    /// signal). Returns `Err` only when the channel itself is broken.
    async fn scroll_to_entity(&self, entity_id: &str) -> Result<()> {
        let _ = self
            .dispatch_event(InteractionEvent::ScrollEntityIntoView {
                entity_id: entity_id.to_string(),
            })
            .await?;
        Ok(())
    }

    /// What `selectable.on_mouse_down` would dispatch at click time — read
    /// the entity's `ReactiveViewModel` from the engine and return its
    /// bound `click_intent()`. The click handler reads the same VM at click
    /// time, so observing the intent ahead of dispatch is equivalent to
    /// observing what the user's click would do.
    fn click_intent_of(&self, entity_id: &str) -> Option<OperationIntent> {
        let root_uri = holon_api::root_layout_block_uri();
        let resolved = self.engine.snapshot_resolved(&root_uri);
        holon_frontend::focus_path::find_click_intent_in_view_model(&resolved, entity_id)
    }

    /// The text actually rendered for `entity_id` on screen — read from
    /// the `BoundsRegistry`'s recorded `displayed_text`, which the
    /// `text` / `editable_text` builders capture from the rendered widget.
    fn displayed_text(&self, entity_id: &str) -> Option<String> {
        self.geometry
            .find_by_entity_id(entity_id)
            .and_then(|info| info.displayed_text)
    }
}

fn region_panel_block_id(region: holon_api::Region) -> &'static str {
    match region {
        holon_api::Region::LeftSidebar => "block:default-left-sidebar",
        holon_api::Region::Main => "block:default-main-panel",
        holon_api::Region::RightSidebar => "block:default-right-sidebar",
    }
}

/// Walk a `ReactiveViewModel` until we find its `collection`, then collect
/// the items' `entity_id`s. Used by `reachable_entities_in_region` to
/// enumerate the data behind a panel's `list(...)` widget.
fn collect_collection_item_ids(
    node: &holon_frontend::reactive_view_model::ReactiveViewModel,
    out: &mut Vec<holon_api::EntityUri>,
) {
    if let Some(ref view) = node.collection {
        for item in view.children_snapshot() {
            if let Some(eid) = item.entity_id() {
                if let Ok(uri) = holon_api::EntityUri::parse(&eid) {
                    out.push(uri);
                }
            }
        }
        return;
    }
    for child in &node.children {
        collect_collection_item_ids(child, out);
    }
    if let Some(ref slot) = node.slot {
        let guard = slot.content.lock_ref();
        collect_collection_item_ids(&guard, out);
    }
}

fn is_modifier(k: &holon_api::Key) -> bool {
    use holon_api::Key;
    matches!(k, Key::Cmd | Key::Ctrl | Key::Alt | Key::Shift)
}

fn modifier_name(k: &holon_api::Key) -> Option<&'static str> {
    use holon_api::Key;
    Some(match k {
        Key::Cmd => "cmd",
        Key::Ctrl => "ctrl",
        Key::Alt => "alt",
        Key::Shift => "shift",
        _ => return None,
    })
}

fn keystroke_name(k: &holon_api::Key) -> Option<String> {
    use holon_api::Key;
    Some(match k {
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
        Key::Char(c) => c.to_string(),
        Key::F(n) => format!("f{n}"),
        Key::Cmd | Key::Ctrl | Key::Alt | Key::Shift => return None,
    })
}
