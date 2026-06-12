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
use holon_api::{EntityUri, KeyChord, Value};
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

    /// Dispatch a `KeyDown` and, when no handler consumed it, retry — waking
    /// on committed render passes — until `timeout`. Covers the gap where
    /// engine focus has already moved but the editor that will consume the
    /// key mounts on the next render pass. An unconsumed keystroke has no
    /// side effects, so the retry cannot double-apply. Fails loud at the
    /// deadline: a key the UI never consumes is a keybinding/focus bug.
    async fn key_down_until_handled(
        &self,
        keystroke: &str,
        modifiers: Vec<String>,
        timeout: Duration,
        context: &str,
    ) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let response = self
                .dispatch_event(InteractionEvent::KeyDown {
                    keystroke: keystroke.to_string(),
                    modifiers: modifiers.clone(),
                })
                .await?;
            if response.handled {
                if attempt > 1 {
                    // A keystroke that was re-dispatched after an unhandled
                    // report is the prime suspect for double-application bugs
                    // (e.g. a split that DID happen but reported unhandled).
                    // Make every such retry loud so failure logs can correlate.
                    eprintln!(
                        "[key_down_until_handled] {context}: {keystroke:?} consumed on \
                         attempt {attempt} (earlier dispatches reported unhandled)"
                    );
                }
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                anyhow::bail!(
                    "{context}: keystroke {keystroke:?} (modifiers {modifiers:?}) was never \
                     consumed within {timeout:?}{detail}",
                    detail = match &response.detail {
                        Some(d) => format!(" (detail: {d})"),
                        None => String::new(),
                    },
                );
            }
            // The retry exists for "editor mounts on the next render pass" —
            // but in a virtualized list the focused row can sit OUTSIDE the
            // viewport, where its editable variant never mounts no matter how
            // long we wait. Mirror a real user's viewport (which follows
            // focus) by scrolling the focused entity into view between
            // retries.
            if let Some(focused) = self.engine.focused_block() {
                let _ = self
                    .dispatch_event(InteractionEvent::ScrollEntityIntoView {
                        entity_id: focused.to_string(),
                    })
                    .await;
            }
            let _ = tokio::time::timeout(Duration::from_millis(50), self.geometry.changed()).await;
        }
    }

    /// Look up an element's window-space center from the `GeometryProvider`.
    ///
    /// Resolves block entity ids plus the non-block UI handles
    /// (`drawer_toggle::`, `vms_button::`, `expand_toggle::`) registered under
    /// their raw element id. The debug_assert guards against a non-block id
    /// silently resolving to `None` (which would look identical to an
    /// un-rendered block) — it fires only when nothing in the chain resolves.
    ///
    /// Lookup chain: `render-entity-{id}` → `selectable-{id}` → raw `{id}` →
    /// entity_id scan. The default `index.org` sidebar wraps each row in
    /// `selectable(row(...))` directly, with no outer `render_entity()`,
    /// so sidebar rows register under `selectable-{id}`. Without that
    /// second alias, `click_entity` on a sidebar item would always miss.
    fn element_center(&self, entity_id: &str) -> Option<(f32, f32)> {
        // Block entity ids resolve via their `render-entity-`/`selectable-`
        // aliases or an entity_id scan. Non-block UI handles
        // (`drawer_toggle::`, `vms_button::`, `expand_toggle::`) are
        // registered under their raw element id, so try that directly too.
        // Visible-area gate: recorded bounds are clipped to the content
        // mask, so a list-overdraw row just outside the viewport has a
        // degenerate rect whose center sits on the clip edge — on top of a
        // DIFFERENT row. Resolving it would click the wrong block
        // (2026-06-11); treat it as unresolved so callers re-wait/scroll.
        for el_id in [
            format!("render-entity-{entity_id}"),
            format!("selectable-{entity_id}"),
            entity_id.to_string(),
        ] {
            if let Some(info) = self.geometry.element_info(&el_id) {
                if info.has_visible_area() {
                    return Some(info.center());
                }
            }
        }
        let resolved = self
            .geometry
            .find_by_entity_id_visible(entity_id)
            .map(|info| info.center());
        // Guard intent (unchanged): a non-block id that resolves to nothing
        // looks identical to an un-rendered block. Block ids may legitimately
        // be absent (not yet rendered); a non-block id that resolves nowhere
        // means the caller drove a handle the driver can't map.
        debug_assert!(
            resolved.is_some() || entity_id.starts_with("block:") || !entity_id.contains(':'),
            "GpuiUserDriver could not resolve non-block entity id {entity_id:?}"
        );
        resolved
    }

    /// All BoundsRegistry elements whose recorded bounds contain `(px, py)`,
    /// ordered topmost-first by ascending area (smallest containing = most
    /// nested = visually on top in a normal layout).
    ///
    /// Returns up to `limit` (id, entity_id, widget_type, area) tuples.
    /// Used by `click_entity` to surface "the click coords are inside three
    /// overlapping elements; the topmost is X not the target Y" rather than
    /// letting the click silently activate the wrong handler.
    fn hit_test(
        &self,
        px: f32,
        py: f32,
        limit: usize,
    ) -> Vec<(String, Option<String>, String, f32)> {
        let mut hits: Vec<_> = self
            .geometry
            .all_elements()
            .into_iter()
            .filter(|(_, info)| {
                px >= info.x
                    && px <= info.x + info.width
                    && py >= info.y
                    && py <= info.y + info.height
            })
            .map(|(id, info)| {
                (
                    id,
                    info.entity_id.as_deref().map(str::to_string),
                    info.widget_type.to_string(),
                    info.area(),
                )
            })
            .collect();
        // Sort by area ascending (smallest = most-nested = visually on top).
        // Tie-break: prefer elements that carry an entity_id, so a wrapper
        // widget registered with the same bounds as its `entity_id`-bearing
        // child does not shadow the child in the hit-test. Without this,
        // `editable_text#N` (entity_id=None) and `editable-text-block:…-content`
        // (entity_id=Some) share identical bounds and the wrapper non-
        // deterministically wins, sending clicks to a handler that can't
        // identify the target block (PBT seed=42 step 11).
        hits.sort_by(|a, b| {
            a.3.partial_cmp(&b.3)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| match (a.1.is_some(), b.1.is_some()) {
                    (true, false) => std::cmp::Ordering::Less,
                    (false, true) => std::cmp::Ordering::Greater,
                    _ => std::cmp::Ordering::Equal,
                })
        });
        hits.truncate(limit);
        hits
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

    /// Click-target center, scoped to a sidebar `region` when the entity is
    /// rendered there. Falls back to the global `require_element_center` for
    /// the main panel, an unparseable region, or an entity not found inside
    /// the named sidebar panel (preserving prior behaviour for every other
    /// caller). See `click_entity` for why the scoping matters.
    fn require_click_center(
        &self,
        entity_id: &str,
        region: &str,
        verb: &str,
    ) -> Result<(f32, f32)> {
        if let Ok(r @ (holon_api::Region::LeftSidebar | holon_api::Region::RightSidebar)) =
            region.parse::<holon_api::Region>()
        {
            if let Some(center) = self.element_center_in_region(entity_id, r) {
                return Ok(center);
            }
        }
        self.require_element_center(entity_id, verb)
    }

    /// Center of the element carrying `entity_id` whose parent chain
    /// terminates at `region`'s panel `live_block`. Disambiguates a block
    /// rendered in several regions. Returns `None` if the region's panel or
    /// an in-region instance isn't currently tracked. Prefers the
    /// smallest-area match (most-nested = the actual clickable row).
    fn element_center_in_region(
        &self,
        entity_id: &str,
        region: holon_api::Region,
    ) -> Option<(f32, f32)> {
        let panel_id = region_panel_block_id(region);
        let elements: HashMap<String, holon_frontend::geometry::ElementInfo> =
            self.geometry.all_elements().into_iter().collect();

        let panel_ids: std::collections::HashSet<String> = elements
            .iter()
            .filter(|(_, info)| {
                info.widget_type.as_ref() == "live_block"
                    && info.entity_id.as_deref() == Some(panel_id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        if panel_ids.is_empty() {
            return None;
        }

        let mut best: Option<&holon_frontend::geometry::ElementInfo> = None;
        for info in elements.values() {
            // Same visible-area gate as `element_center`: a content-mask
            // clipped (degenerate) rect's center sits on a different row.
            if info.entity_id.as_deref() != Some(entity_id) || !info.has_visible_area() {
                continue;
            }
            let mut cursor = info.parent_id.clone();
            let mut depth = 0;
            let in_region = loop {
                let Some(p) = cursor else { break false };
                if panel_ids.contains(p.as_ref()) {
                    break true;
                }
                depth += 1;
                if depth > 100 {
                    break false;
                }
                cursor = elements.get(p.as_ref()).and_then(|i| i.parent_id.clone());
            };
            if in_region && best.is_none_or(|b| info.area() < b.area()) {
                best = Some(info);
            }
        }
        best.map(|info| info.center())
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
    /// `wait_for_element_bounds` remedy.
    ///
    /// For a sidebar `region`, resolution is scoped to that region's panel: a
    /// block rendered in BOTH a sidebar list and the main panel (e.g.
    /// `block:journals`) would otherwise resolve to the main-panel
    /// `render-entity-` element, whose click dispatches `navigation.editor_focus`
    /// instead of the sidebar row's `navigation.focus` — so the click silently
    /// places a cursor rather than navigating. Main clicks keep the global
    /// `render-entity-`-first resolution.
    #[tracing::instrument(skip(self), name = "GpuiUserDriver.click_entity", fields(%entity_id))]
    async fn click_entity(&self, entity_id: &EntityUri, region: &str) -> Result<()> {
        // The geometry/bounds registry is string-keyed (element ids, not
        // routing): render one stable string at the seam.
        let entity_id = entity_id.as_str();
        let (cx, cy) = self.require_click_center(entity_id, region, "click_entity")?;
        // Hit-test BEFORE dispatching so the log captures the layout the
        // synthetic MouseDown will see. The topmost (smallest containing)
        // element is logged first; up to 4 candidates total. If the topmost
        // entity_id doesn't match the requested entity, we have a bounds /
        // overlap / focus-trap problem — much more actionable than "focus
        // didn't change 1 s after the click."
        let candidates = self.hit_test(cx, cy, 4);
        if candidates.is_empty() {
            tracing::warn!(
                "[ui-event] click_entity({entity_id:?}) coords=({cx:.1},{cy:.1}) — \
                 hit-test found NO containing element in BoundsRegistry"
            );
        } else {
            let summary: Vec<String> = candidates
                .iter()
                .map(|(id, eid, w, a)| format!("{id} entity_id={eid:?} widget={w} area={a:.0}"))
                .collect();
            tracing::info!(
                "[ui-event] click_entity({entity_id:?}) coords=({cx:.1},{cy:.1}) topmost: {}",
                summary.join(" | ")
            );
            eprintln!(
                "[ui-event] click_entity({entity_id:?}) coords=({cx:.1},{cy:.1}) topmost: {}",
                summary.join(" | ")
            );
            let topmost_entity = candidates.first().and_then(|(_, eid, _, _)| eid.as_deref());
            if topmost_entity != Some(entity_id) {
                eprintln!(
                    "[ui-event] click_entity({entity_id:?}) WARN: topmost element entity_id \
                     is {:?}, not {entity_id:?} — click may activate the topmost handler instead",
                    topmost_entity
                );
            }
        }
        self.dispatch_event(InteractionEvent::MouseClick {
            position: (cx, cy),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;
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
        _: &EntityUri,
        _: &ReactiveViewModel,
        entity_id: &EntityUri,
        chord: &KeyChord,
        extra_params: HashMap<String, Value>,
    ) -> Result<bool> {
        let entity_id = entity_id.as_str();
        // A real user presses a chord into the editor they're already in —
        // no click. Click-to-focus only when the target does NOT already
        // hold editor focus: an unconditional click re-seats the caret at
        // the element center, diverging from the reference model, which
        // leaves the caret alone for the already-focused case (see
        // `transitions::model_chord_click_focus`).
        // Click-to-focus with re-click: a legitimate async commit (pending
        // text flushing on a previous editor's authority-leave) can
        // re-render and shift row bounds between bounds resolution and the
        // gpui hit-test, so a single click can land on a neighbor row.
        // Re-resolve fresh bounds and re-click until `focused_block` lands
        // on the target; fail loud at the deadline — keys pressed into the
        // wrong editor corrupt content invisibly.
        {
            use futures::StreamExt;
            use futures_signals::signal::SignalExt;
            let overall_deadline = tokio::time::Instant::now() + Duration::from_secs(4);
            loop {
                let already_focused = self
                    .engine
                    .ui_state()
                    .focused_block_mutable()
                    .get_cloned()
                    .as_ref()
                    .map(|u| u.as_str())
                    == Some(entity_id);
                if already_focused {
                    // A real user presses a chord into the editor they're
                    // already in — no click. An unconditional click would
                    // re-seat the caret at the element center, diverging from
                    // the reference model (see `model_chord_click_focus`).
                    break;
                }
                let (cx, cy) = self.require_element_center(entity_id, "send_key_chord")?;
                self.dispatch_event(InteractionEvent::MouseClick {
                    position: (cx, cy),
                    button: "left".into(),
                    modifiers: Vec::new(),
                })
                .await?;
                // Window focus follows `focused_block` via the spawned
                // binding — wait for this click's focus move to land.
                // (`to_stream()` emits the current value first.)
                let attempt_deadline = std::cmp::min(
                    tokio::time::Instant::now() + Duration::from_secs(1),
                    overall_deadline,
                );
                let mut focus_stream = self
                    .engine
                    .ui_state()
                    .focused_block_mutable()
                    .signal_cloned()
                    .to_stream();
                let landed = loop {
                    match tokio::time::timeout_at(attempt_deadline, focus_stream.next()).await {
                        Ok(Some(actual))
                            if actual.as_ref().map(|u| u.as_str()) == Some(entity_id) =>
                        {
                            break true;
                        }
                        Ok(Some(_)) => continue,
                        Ok(None) | Err(_) => break false,
                    }
                };
                if landed {
                    break;
                }
                if tokio::time::Instant::now() >= overall_deadline {
                    anyhow::bail!(
                        "send_key_chord: click on {entity_id:?} never moved focused_block \
                         to it within 4s (incl. re-click attempts) — refusing to press \
                         {chord:?} into the wrong editor"
                    );
                }
                eprintln!(
                    "[send_key_chord] click did not land focus on {entity_id:?} (re-render \
                     likely shifted bounds mid-click); re-clicking"
                );
            }
        }
        // Engine focus has moved, but the target's editor mounts and takes
        // WINDOW focus only on a following render pass — until then a key is
        // either dropped or consumed by the previously-focused editor, which
        // the `handled` flag can't distinguish. Wait for the target's
        // editable_text to report `focused == true` in a committed frame.
        {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
            loop {
                let window_focused = self.geometry.all_elements().iter().any(|(_, info)| {
                    info.entity_id.as_deref() == Some(entity_id)
                        && info.widget_type.as_ref() == "editable_text"
                        && info.focused == Some(true)
                });
                if window_focused {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    anyhow::bail!(
                        "send_key_chord: {entity_id:?}'s editable_text never took window focus \
                         within 2s of click-to-focus — refusing to press {chord:?} into whatever \
                         editor still holds it"
                    );
                }
                let _ =
                    tokio::time::timeout(Duration::from_millis(50), self.geometry.changed()).await;
            }
        }

        // Position the cursor with real input when the caller specified a
        // byte offset. Click lands the cursor somewhere in the line; we
        // re-anchor with `home` and then advance with `right`. This goes
        // through `PlatformInput` and `InputState::move_to_*`, so any
        // bug in cursor handling surfaces here just like in production.
        if let Some(Value::Integer(target)) = extra_params.get("position") {
            let target = (*target).max(0) as usize;
            self.key_down_until_handled(
                "home",
                Vec::new(),
                Duration::from_secs(2),
                "send_key_chord(position)",
            )
            .await?;
            for _ in 0..target {
                self.key_down_until_handled(
                    "right",
                    Vec::new(),
                    Duration::from_secs(2),
                    "send_key_chord(position)",
                )
                .await?;
            }
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
            self.key_down_until_handled(
                &name,
                mod_names.clone(),
                Duration::from_secs(2),
                "send_key_chord",
            )
            .await?;
        }

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
        Ok(())
    }

    /// Scroll over an entity — looks up its window-space center via the
    /// `GeometryProvider` and delegates to `scroll_at`. Fails loud when
    /// bounds aren't available; MCP clients now receive an error
    /// instead of a silent no-op (observable behavior change).
    async fn scroll_entity(&self, entity_id: &EntityUri, dx: f32, dy: f32) -> Result<()> {
        let (cx, cy) = self.require_element_center(entity_id.as_str(), "scroll_entity")?;
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
    async fn drop_entity(
        &self,
        _: &EntityUri,
        source_id: &EntityUri,
        target_id: &EntityUri,
    ) -> Result<bool> {
        let (sx, sy) = self.require_element_center(source_id.as_str(), "drop_entity (source)")?;
        let (tx, ty) = self.require_element_center(target_id.as_str(), "drop_entity (target)")?;

        self.dispatch_event(InteractionEvent::MouseDown {
            position: (sx, sy),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;

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
        }

        self.dispatch_event(InteractionEvent::MouseUp {
            position: (tx, ty),
            button: "left".into(),
            modifiers: Vec::new(),
        })
        .await?;
        Ok(true)
    }

    async fn send_raw_keystroke_until_handled(
        &self,
        keystroke: &str,
        modifiers: &[&str],
        timeout: Duration,
    ) -> Result<()> {
        self.key_down_until_handled(
            keystroke,
            modifiers.iter().map(|s| s.to_string()).collect(),
            timeout,
            "send_raw_keystroke_until_handled",
        )
        .await
    }

    async fn send_raw_keystroke(&self, keystroke: &str, modifiers: &[&str]) -> Result<()> {
        // In a virtualized list the focused row can sit outside the viewport:
        // its editor variant is unmounted, so the keystroke would be dropped
        // and we'd bail "not consumed". A real user's viewport follows focus —
        // mirror that: when the focused entity has no committed bounds,
        // reveal it and wait out the scroll → mount → paint cascade before
        // dispatching. Cheap when the row is visible (one registry lookup).
        if let Some(focused) = self.engine.focused_block() {
            if self.element_center(focused.as_str()).is_none() {
                let _ = self
                    .dispatch_event(InteractionEvent::ScrollEntityIntoView {
                        entity_id: focused.to_string(),
                    })
                    .await;
                for _ in 0..4 {
                    if self.element_center(focused.as_str()).is_some() {
                        break;
                    }
                    let _ =
                        tokio::time::timeout(Duration::from_millis(100), self.geometry.changed())
                            .await;
                }
            }
        }
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
        // Pace consecutive keystrokes by one committed frame. A real user's
        // keystrokes never outrun the compositor; without this, sub-frame
        // typing races the editor's focus-gated backend-echo handling and
        // drops characters (inv-displayed-text). Wakes on the commit notify;
        // the cap covers windows that aren't repainting.
        // `HOLON_PBT_SUPERHUMAN_INPUT=1` disables the pacing — a stress mode
        // that hunts the dropped-character editor-echo race as a prod bug
        // instead of pacing around it (expect inv-displayed-text failures
        // until that race is fixed; not part of the green gate).
        if !superhuman_input() {
            let _ = tokio::time::timeout(Duration::from_millis(50), self.geometry.changed()).await;
        }
        Ok(())
    }

    /// Tree-aware click — for the screen driver this is just `click_entity`.
    /// The bound click handler reads `click_intent` itself at the rendered
    /// widget, so dispatching the right intent is the click handler's job,
    /// not the test driver's. Returning `false` reflects that the driver
    /// can't synchronously prove which intent fired.
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

    // ── Observation verbs ──────────────────────────────────────────────
    //
    // Screen-mode answers via the BoundsRegistry. For "what's on screen
    // right now" (visible, in-region, displayed text) this is the only
    // honest source: the registry records what the prepaint pipeline
    // actually placed, post-layout, post-cull. For "what could the user
    // reach" (reachable set, click intent) we read the engine's
    // ReactiveViewModel snapshot — same source the production click /
    // list renders consult.

    fn is_widget_visible(&self, entity_id: &EntityUri) -> bool {
        self.geometry
            .find_by_entity_id(entity_id.as_str())
            .filter(|info| info.area() > 0.0)
            .is_some()
    }

    fn is_in_region(&self, entity_id: &EntityUri, region: holon_api::Region) -> bool {
        self.entities_in_region(region)
            .iter()
            .any(|uri| uri == entity_id)
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
                info.widget_type.as_ref() == "live_block"
                    && info.entity_id.as_deref() == Some(panel_id)
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
                if panel_ids.contains(p.as_ref()) {
                    result.insert(eid.to_string());
                    break;
                }
                depth += 1;
                if depth > 100 {
                    break;
                }
                cursor = elements.get(p.as_ref()).and_then(|i| i.parent_id.clone());
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
        let panel_uri =
            holon_api::EntityUri::parse(region_panel_block_id(region)).expect("static panel id");
        let root = self.engine.snapshot_reactive(&panel_uri);
        let mut out = Vec::new();
        collect_collection_item_ids(&root, &mut out);
        out
    }

    /// Scroll a panel's virtualized list so the named `entity_id` enters
    /// the viewport. ALL collection panels (sidebars and the main panel)
    /// render through `gpui::list(...)`-backed shells, so off-viewport
    /// rows have no BoundsRegistry entry until this RPC scrolls them in.
    ///
    /// Returns `Ok(())` when the scroll handler succeeded OR when the
    /// entity wasn't in any virtualized list (caller — `wait_for_entity_bounds`
    /// — keeps polling and lets the timeout be the authoritative failure
    /// signal). Returns `Err` only when the channel itself is broken.
    async fn scroll_to_entity(&self, entity_id: &EntityUri) -> Result<()> {
        let resp = self
            .dispatch_event(InteractionEvent::ScrollEntityIntoView {
                entity_id: entity_id.to_string(),
            })
            .await?;
        // `handled == false` without detail is the benign "not in any
        // virtualized list" case. A `detail` means the lookup itself failed
        // (unparseable URI, window update error) — surface that instead of
        // letting the caller's poll timeout absorb it silently.
        if let Some(detail) = resp.detail {
            anyhow::bail!("scroll_to_entity({entity_id}): {detail}");
        }
        Ok(())
    }

    /// What `selectable.on_mouse_down` would dispatch at click time — read
    /// the entity's `ReactiveViewModel` from the engine and return its
    /// bound `click_intent()`. The click handler reads the same VM at click
    /// time, so observing the intent ahead of dispatch is equivalent to
    /// observing what the user's click would do.
    fn click_intent_of(&self, entity_id: &EntityUri) -> Option<OperationIntent> {
        let root_uri = holon_api::root_layout_block_uri();
        let resolved = self.engine.snapshot_resolved(&root_uri);
        holon_frontend::focus_path::find_click_intent_in_view_model(&resolved, entity_id)
    }

    /// The text actually rendered for `entity_id` on screen — read from
    /// the `BoundsRegistry`'s recorded `displayed_text`, which the
    /// `text` / `editable_text` builders capture from the rendered widget.
    fn displayed_text(&self, entity_id: &EntityUri) -> Option<String> {
        self.geometry
            .find_by_entity_id(entity_id.as_str())
            .and_then(|info| info.displayed_text.map(|s| s.to_string()))
    }
}

/// `HOLON_PBT_SUPERHUMAN_INPUT=1` — stress mode: keystrokes are NOT paced to
/// one committed frame each, so sub-frame typing races the editor's
/// focus-gated backend-echo handling. Used to reproduce the dropped-character
/// race ("skip-when-focused staleness" family) as a prod bug.
fn superhuman_input() -> bool {
    static FLAG: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *FLAG.get_or_init(|| std::env::var("HOLON_PBT_SUPERHUMAN_INPUT").as_deref() == Ok("1"))
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
            if let Some(uri) = item.entity_id() {
                out.push(uri);
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

pub fn is_modifier(k: &holon_api::Key) -> bool {
    use holon_api::Key;
    matches!(k, Key::Cmd | Key::Ctrl | Key::Alt | Key::Shift)
}

pub fn modifier_name(k: &holon_api::Key) -> Option<&'static str> {
    use holon_api::Key;
    Some(match k {
        Key::Cmd => "cmd",
        Key::Ctrl => "ctrl",
        Key::Alt => "alt",
        Key::Shift => "shift",
        _ => return None,
    })
}

pub fn keystroke_name(k: &holon_api::Key) -> Option<String> {
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
