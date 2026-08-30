//! `WebUserDriver` — the keystone's web arm: drives the dioxus-web frontend in
//! a real Chrome, so the browser stack (DOM event loop → dioxus → wasm worker →
//! engine → OPFS) sits under the same `UserDriver` contract as headless and
//! GPUI. Design: `docs/Testing/WebArmPBT.md`.
//!
//! # Transport (design ruling D3.a: in-Rust CDP, no sidecar)
//!
//! `chromiumoxide` — speaks the Chrome DevTools Protocol from Rust and launches
//! Chrome itself. Rejected alternative: `fantoccini` + WebDriver, which needs a
//! `chromedriver` binary installed and version-matched to the local Chrome
//! (this machine has Chrome but no chromedriver), reintroducing exactly the
//! sidecar D3.a rules out. The cost is that CDP has no auto-waiting: every
//! action verb below ends in an explicit DOM-quiescence await.
//!
//! # Entity addressing
//!
//! Entities are reached through the `data-entity-id` attribute the dioxus
//! renderer emits on `rendered-text`, `editor-cell` and `selectable` nodes. The
//! value is the `ViewModel::row_id()` string, which is the scheme-qualified URI
//! (`block:welcome`) — so it matches `EntityUri::as_str()`, not `id()`.
//!
//! # Hazard: `send_key_chord` moves the caret (increment 2 must model this)
//!
//! `send_key_chord` focuses its target with a real click, and a click lands at
//! the element's centre — so it *repositions the caret* before the chord fires.
//! A chord aimed at a block whose caret sat at offset 94 dispatches with the
//! caret at whatever offset the centre maps to, and `enter` then splits
//! mid-word instead of at the end. The verb also refuses `extra_params` (the
//! channel headless drivers use to pass an explicit `split_block` cursor byte)
//! rather than dropping them silently, so the caret this driver uses is always
//! click-derived and never the one the caller had in mind.
//!
//! Increment 2's oracle MUST model the click-derived caret. If it predicts from
//! a caret the reference state tracked independently, every chord after a
//! non-centre caret will diverge — and it will look like an application bug
//! rather than a harness artefact.
//!
//! # Increment 1 scope
//!
//! Three real-gesture verbs (`click_entity`, `send_raw_keystroke` — which is
//! what carries `replace_text`/typing — and `send_key_chord`) plus the
//! observation verbs they need. Everything else fails loud: increment 2 adds
//! the MCP-relay oracle, which is also what gives the region verbs and true
//! engine quiescence.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use chromiumoxide::Browser;
use chromiumoxide::BrowserConfig;
use chromiumoxide::Page;
use chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventParams;
use chromiumoxide::cdp::browser_protocol::input::DispatchKeyEventType;
use chromiumoxide::cdp::js_protocol::runtime::EventConsoleApiCalled;
use chromiumoxide::keys::get_key_definition;
use futures_util::StreamExt as _;
use holon_api::EntityUri;
use holon_api::KeyChord;
use holon_api::Value;
use holon_frontend::operations::OperationIntent;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::user_driver::UserDriver;
use tokio::task::JoinHandle;

use crate::web_relay_oracle::WebRelayOracle;

/// One rendered entity as the browser currently shows it.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RenderedNode {
    pub id: String,
    pub role: String,
    pub text: String,
    pub visible: bool,
}

/// Collects every `data-entity-id`-carrying node with its role, text and
/// visibility. This is the web arm's rendered-set read.
const SNAPSHOT_JS: &str = r"
Array.from(document.querySelectorAll('[data-entity-id]')).map(e => ({
  id: e.getAttribute('data-entity-id'),
  role: e.getAttribute('data-role') || '',
  text: e.innerText || '',
  visible: !!(e.offsetWidth || e.offsetHeight || e.getClientRects().length),
}))
";

/// How often the DOM is re-read while waiting for a gesture to settle.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// How long the rendered set must hold still before a gesture counts as
/// settled WITHOUT a relay oracle. Override with `HOLON_WEB_SETTLE_MS`.
fn settle_window() -> Duration {
    Duration::from_millis(match std::env::var("HOLON_WEB_SETTLE_MS") {
        Ok(v) => v
            .parse()
            .unwrap_or_else(|e| panic!("HOLON_WEB_SETTLE_MS={v:?} is not a valid u64: {e}")),
        Err(_) => 300,
    })
}

/// How long the rendered set must hold still AFTER the engine has reported
/// convergence. This window no longer has to cover engine round-trips — only
/// the render that follows a converged engine — so it is a fraction of
/// [`settle_window`]. Override with `HOLON_WEB_RENDER_WINDOW_MS`.
fn render_window() -> Duration {
    Duration::from_millis(match std::env::var("HOLON_WEB_RENDER_WINDOW_MS") {
        Ok(v) => v
            .parse()
            .unwrap_or_else(|e| panic!("HOLON_WEB_RENDER_WINDOW_MS={v:?} is not a valid u64: {e}")),
        Err(_) => 60,
    })
}

pub struct WebUserDriver {
    browser: Browser,
    page: Page,
    /// Last rendered-set read. The trait's observation verbs are sync but CDP
    /// is not, so every action verb refreshes this before returning.
    snapshot: Mutex<Vec<RenderedNode>>,
    handler: JoinHandle<()>,
    /// Everything the page logged, newest last. Wasm-side `tracing` output
    /// only reaches the harness through here.
    console: Arc<Mutex<Vec<String>>>,
    console_task: JoinHandle<()>,
    /// Gesture → last observable DOM change, for the most recent verb.
    last_effect: Mutex<Duration>,
    /// Engine-convergence time reported by the relay for the most recent verb.
    /// `None` on a DOM-only driver.
    last_engine_wait: Mutex<Option<Duration>>,
    settle_window: Duration,
    quiescence_timeout: Duration,
    /// The authoritative channel. When present it is the PRIMARY settled
    /// signal and the DOM window drops to [`render_window`]; when absent the
    /// driver is DOM-only, which increment 1 measured to be unsound for
    /// quiescence (see [`Self::await_quiescence`]).
    oracle: Option<Arc<WebRelayOracle>>,
}

impl WebUserDriver {
    /// Launch with only the DOM channel. Sound for gesture *effects*, NOT for
    /// quiescence — prefer [`Self::launch_with_oracle`].
    pub async fn launch(url: &str, headless: bool) -> Result<Self> {
        Self::launch_inner(url, headless, None).await
    }

    /// Launch and attach the relay oracle as the primary settled signal. The
    /// oracle must already be connected to the same hub the page dials, and
    /// the page must be the only `role=browser` socket on it.
    pub async fn launch_with_oracle(
        url: &str,
        headless: bool,
        oracle: Arc<WebRelayOracle>,
    ) -> Result<Self> {
        Self::launch_inner(url, headless, Some(oracle)).await
    }

    /// Launch Chrome, open `url` in a fresh incognito browser context (which is
    /// what makes OPFS empty per case), and wait for the app to render.
    async fn launch_inner(
        url: &str,
        headless: bool,
        oracle: Option<Arc<WebRelayOracle>>,
    ) -> Result<Self> {
        let mut builder = BrowserConfig::builder().args(vec![
            // The app needs crossOriginIsolated for wasm-wasip1-threads; the
            // server sends COOP/COEP, these keep Chrome from vetoing it.
            "--enable-features=SharedArrayBuffer",
            "--disable-dev-shm-usage",
        ]);
        if !headless {
            builder = builder.with_head();
        }
        let config = builder
            .build()
            .map_err(|e| anyhow::anyhow!("BrowserConfig::build failed: {e}"))?;
        let (mut browser, mut handler) = Browser::launch(config)
            .await
            .context("launching Chrome over CDP failed — is Chrome installed?")?;
        let handler = tokio::spawn(async move { while handler.next().await.is_some() {} });

        browser
            .start_incognito_context()
            .await
            .context("creating a fresh incognito browser context failed")?;
        let page = browser
            .new_page(url)
            .await
            .with_context(|| format!("opening {url} failed"))?;

        // The page's console is the only place a wasm-side `tracing::error!`
        // surfaces. Capturing it keeps a dropped intent from reading as a
        // silent no-op.
        let mut console_events = page
            .event_listener::<EventConsoleApiCalled>()
            .await
            .context("subscribing to the page console failed")?;
        let console: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let console_sink = Arc::clone(&console);
        let console_task = tokio::spawn(async move {
            while let Some(event) = console_events.next().await {
                let text = event
                    .args
                    .iter()
                    .filter_map(|arg| arg.value.as_ref())
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        other => other.to_string(),
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                console_sink
                    .lock()
                    .unwrap()
                    .push(format!("{:?}: {text}", event.r#type));
            }
        });

        let driver = Self {
            browser,
            page,
            snapshot: Mutex::new(Vec::new()),
            handler,
            console,
            console_task,
            last_effect: Mutex::new(Duration::ZERO),
            last_engine_wait: Mutex::new(None),
            settle_window: if oracle.is_some() {
                render_window()
            } else {
                settle_window()
            },
            quiescence_timeout: Duration::from_secs(30),
            oracle,
        };
        driver.assert_cross_origin_isolated().await?;
        driver.await_boot().await?;
        // Only now can the oracle prove itself: the page dials the hub during
        // boot, so a liveness check before this point tests the hub, not the
        // engine.
        if let Some(oracle) = &driver.oracle {
            oracle.await_ready(driver.quiescence_timeout).await?;
        }
        driver.await_quiescence().await?;
        Ok(driver)
    }

    /// Block until the app publishes `data-boot-state="ready"`.
    ///
    /// Without this, DOM quiescence alone declares the shell settled while the
    /// worker is still booting — the app renders a stable "booting…" frame for
    /// seconds. Any non-ready terminal state fails here rather than surfacing
    /// as a missing-element error three verbs later.
    async fn await_boot(&self) -> Result<()> {
        let deadline = Instant::now() + self.quiescence_timeout;
        loop {
            let state = self.boot_state().await?;
            match state.as_str() {
                "ready" => return Ok(()),
                "booting" | "" => {}
                terminal => bail!(
                    "app boot ended in state {terminal:?} — page text: {}",
                    self.body_text().await.unwrap_or_default() /* ALLOW(ok): best-effort
                                                                * diagnostic enrichment inside a
                                                                * bail! that is already failing
                                                                * loudly */
                ),
            }
            if Instant::now() >= deadline {
                bail!(
                    "app never reached boot state 'ready' within {:?} (last state {state:?})",
                    self.quiescence_timeout
                );
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// The wasm worker only boots under cross-origin isolation. Checking it
    /// here turns a missing COOP/COEP header into one clear failure instead of
    /// a hang in the first verb.
    async fn assert_cross_origin_isolated(&self) -> Result<()> {
        let isolated: bool = self
            .page
            .evaluate("crossOriginIsolated")
            .await?
            .into_value()
            .context("reading crossOriginIsolated failed")?;
        if !isolated {
            bail!(
                "page is not crossOriginIsolated — the server must send COOP: same-origin and \
                 COEP: require-corp, or the wasm worker never boots"
            );
        }
        Ok(())
    }

    /// The app's `data-boot-state` marker as it reads right now. Empty when the
    /// title bar is not mounted.
    pub async fn boot_state(&self) -> Result<String> {
        self.page
            .evaluate(
                "(document.querySelector('[data-boot-state]')||{getAttribute:()=>''})\
                 .getAttribute('data-boot-state') || ''",
            )
            .await?
            .into_value()
            .context("reading data-boot-state failed")
    }

    /// Read the rendered set and store it for the sync observation verbs.
    pub async fn refresh_snapshot(&self) -> Result<Vec<RenderedNode>> {
        let nodes: Vec<RenderedNode> = self
            .page
            .evaluate(SNAPSHOT_JS)
            .await?
            .into_value()
            .context("rendered-set snapshot did not deserialize")?;
        *self.snapshot.lock().unwrap() = nodes.clone();
        Ok(nodes)
    }

    /// Wait until the gesture has settled. Two channels, in the order D4.a
    /// rules for:
    ///
    /// 1. **Relay (primary, when attached).** The engine reports its own
    ///    convergence over MCP. This is the sound signal: a gesture travels
    ///    dioxus → worker → engine → projection → DOM and the worker only
    ///    advances on a 16ms tick pump, so the DOM sits unchanged mid-flight —
    ///    increment 1 watched a DOM-stability rule declare a block split
    ///    settled one gesture early (design §4a).
    /// 2. **DOM (secondary).** Even a converged engine has not rendered yet, so
    ///    the rendered set must still hold still — but only for
    ///    [`render_window`], since no further engine work can arrive.
    ///
    /// Without an oracle the DOM window is the ONLY signal and widens to
    /// [`settle_window`]; that mode is retained for the increment-1 spike and
    /// is documented as unsound for quiescence.
    ///
    /// Records the gesture→last-DOM-change delay in `last_effect` and the
    /// engine's own convergence time in `last_engine_wait`.
    pub async fn await_quiescence(&self) -> Result<()> {
        let start = Instant::now();
        match &self.oracle {
            Some(oracle) => {
                let report = oracle
                    .await_quiescence(self.quiescence_timeout)
                    .await
                    .context(
                        "relay oracle failed to observe engine convergence — the web arm's \
                         primary settled signal is gone, so no assertion after this gesture \
                         would mean anything",
                    )?;
                *self.last_engine_wait.lock().unwrap() = Some(report.waited);
            }
            None => *self.last_engine_wait.lock().unwrap() = None,
        }
        // Seeded AFTER the relay wait, not at `start`: the relay round-trip
        // easily exceeds the render window, and a `last_change` still anchored
        // at `start` would satisfy the stability test on the first poll — the
        // DOM channel would assert nothing at all.
        let mut last_change = Instant::now();
        let mut previous = self.render_key(&self.refresh_snapshot().await?);
        loop {
            tokio::time::sleep(POLL_INTERVAL).await;
            let current = self.render_key(&self.refresh_snapshot().await?);
            if current != previous {
                previous = current;
                last_change = Instant::now();
            } else if last_change.elapsed() >= self.settle_window {
                *self.last_effect.lock().unwrap() = last_change.duration_since(start);
                return Ok(());
            }
            if start.elapsed() >= self.quiescence_timeout {
                bail!(
                    "DOM never settled within {:?} — last rendered set had {} nodes",
                    self.quiescence_timeout,
                    self.snapshot.lock().unwrap().len()
                );
            }
        }
    }

    /// Gesture → last observable DOM change for the most recent verb. Zero
    /// means the gesture produced no rendered change at all.
    pub fn last_effect(&self) -> Duration {
        *self.last_effect.lock().unwrap()
    }

    /// Engine convergence time the relay reported for the most recent verb.
    /// `None` on a DOM-only driver.
    pub fn last_engine_wait(&self) -> Option<Duration> {
        *self.last_engine_wait.lock().unwrap()
    }

    /// The authoritative channel, for assertions that must not be read off the
    /// DOM. `None` on a DOM-only driver.
    pub fn oracle(&self) -> Option<&Arc<WebRelayOracle>> {
        self.oracle.as_ref()
    }

    fn render_key(&self, nodes: &[RenderedNode]) -> String {
        nodes
            .iter()
            .map(|n| format!("{}\u{1}{}\u{1}{}", n.id, n.role, n.text))
            .collect::<Vec<_>>()
            .join("\u{2}")
    }

    pub fn snapshot(&self) -> Vec<RenderedNode> {
        self.snapshot.lock().unwrap().clone()
    }

    /// Evaluate `js` in the page and return its JSON value. This is the DOM
    /// oracle channel's read primitive.
    pub async fn evaluate_json(&self, js: &str) -> Result<serde_json::Value> {
        Ok(self.page.evaluate(js).await?.into_value()?)
    }

    /// Whole-page text — the coarse end-state assertion channel.
    pub async fn body_text(&self) -> Result<String> {
        Ok(self
            .page
            .evaluate("document.body.innerText")
            .await?
            .into_value()?)
    }

    /// Candidate selectors for `entity_id`, most-preferred first. `region`
    /// picks the order: "sidebar" wants the selectable row, everything else
    /// wants the editor cell or the text body.
    ///
    /// One selector per role rather than a comma-joined group: a grouped
    /// selector resolves in DOM order, which hands back the block's *hidden*
    /// rendered-text once it flips into edit mode.
    fn selectors_for(entity_id: &EntityUri, region: &str) -> Vec<String> {
        let id = entity_id.as_str();
        let roles: &[&str] = match region {
            "sidebar" => &["selectable", "rendered-text", "editor-cell"],
            _ => &["editor-cell", "rendered-text", "selectable"],
        };
        roles
            .iter()
            .map(|role| format!("[data-role=\"{role}\"][data-entity-id=\"{id}\"]"))
            .collect()
    }

    /// First candidate node that exists AND has a clickable point.
    async fn find_clickable(
        &self,
        entity_id: &EntityUri,
        region: &str,
    ) -> Result<chromiumoxide::Element> {
        let selectors = Self::selectors_for(entity_id, region);
        let mut attempts = Vec::new();
        for selector in &selectors {
            match self.page.find_element(selector.as_str()).await {
                Ok(element) => match element.clickable_point().await {
                    Ok(_) => return Ok(element),
                    Err(e) => attempts.push(format!("{selector}: present but not clickable ({e})")),
                },
                Err(e) => attempts.push(format!("{selector}: {e}")),
            }
        }
        bail!(
            "no clickable DOM node for {entity_id} in region {region}:\n  {}",
            attempts.join("\n  ")
        )
    }

    /// Map a GPUI-style key name onto the CDP/US-keyboard name the DevTools
    /// protocol expects. Unknown names fail loud rather than dropping a
    /// keystroke the test believes it sent.
    fn cdp_key(keystroke: &str) -> Result<&'static str> {
        Ok(match keystroke {
            "enter" => "Enter",
            "tab" => "Tab",
            "escape" => "Escape",
            "backspace" => "Backspace",
            "delete" => "Delete",
            "space" => " ",
            "home" => "Home",
            "end" => "End",
            "left" => "ArrowLeft",
            "right" => "ArrowRight",
            "up" => "ArrowUp",
            "down" => "ArrowDown",
            other => {
                // Single printable chars pass through as themselves.
                if other.chars().count() == 1 {
                    return get_key_definition(other)
                        .map(|d| d.key)
                        .ok_or_else(|| anyhow::anyhow!("no US-keyboard definition for {other:?}"));
                }
                bail!(
                    "WebUserDriver: no CDP key mapping for {other:?}. Add it to `cdp_key` rather \
                     than letting the keystroke vanish."
                )
            }
        })
    }

    /// CDP modifier bitmask (Alt=1, Ctrl=2, Meta=4, Shift=8).
    fn modifier_mask(modifiers: &[&str]) -> Result<i64> {
        let mut mask = 0;
        for m in modifiers {
            mask |= match *m {
                "alt" => 1,
                "ctrl" => 2,
                "cmd" | "meta" => 4,
                "shift" => 8,
                other => bail!("WebUserDriver: unknown modifier {other:?}"),
            };
        }
        Ok(mask)
    }

    async fn dispatch_key(&self, keystroke: &str, modifiers: &[&str]) -> Result<()> {
        let key = Self::cdp_key(keystroke)?;
        let def = get_key_definition(key)
            .ok_or_else(|| anyhow::anyhow!("no US-keyboard definition for {key:?}"))?;
        let mask = Self::modifier_mask(modifiers)?;

        let mut down = DispatchKeyEventParams::builder()
            .key(def.key)
            .code(def.code)
            .windows_virtual_key_code(def.key_code)
            .native_virtual_key_code(def.key_code)
            .modifiers(mask);
        // A char-producing key must carry `text`, or the editor sees a bare
        // keydown and inserts nothing. Modified chords never produce text.
        let down_type = if mask == 0 && def.key.chars().count() == 1 {
            down = down.text(def.text.unwrap_or(def.key));
            DispatchKeyEventType::KeyDown
        } else {
            DispatchKeyEventType::RawKeyDown
        };
        self.page
            .execute(down.clone().r#type(down_type).build().unwrap())
            .await?;
        self.page
            .execute(down.r#type(DispatchKeyEventType::KeyUp).build().unwrap())
            .await?;
        self.await_quiescence().await
    }

    /// Everything the page has logged so far.
    pub fn console_log(&self) -> Vec<String> {
        self.console.lock().unwrap().clone()
    }

    pub async fn close(mut self) -> Result<()> {
        self.browser.close().await?;
        self.console_task.abort();
        self.handler.abort();
        Ok(())
    }
}

#[async_trait::async_trait]
impl UserDriver for WebUserDriver {
    async fn synthetic_dispatch(
        &self,
        entity: &str,
        op: &str,
        _: HashMap<String, Value>,
    ) -> Result<()> {
        bail!(
            "WebUserDriver::synthetic_dispatch({entity}.{op}) is a DECLARED CAP. The relay could \
             carry it (`execute_operation`), but the web arm exists to prove the DOM→intent \
             wiring, so a transition that needs synthetic dispatch is skipped rather than \
             smuggled past the layer under test."
        )
    }

    async fn click_entity(&self, entity_id: &EntityUri, region: &str) -> Result<()> {
        let element = self.find_clickable(entity_id, region).await?;
        element.scroll_into_view().await?;
        element
            .click()
            .await
            .with_context(|| format!("clicking {entity_id} failed"))?;
        self.await_quiescence().await
    }

    /// Screen-driver semantics: the page's own click handler resolves the
    /// intent, so we cannot prove synchronously which one fired.
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

    /// Focus the entity, then send the chord through Chrome's real input
    /// pipeline. Whether a handler consumed it is not observable from CDP, so
    /// this reports `true` only when the rendered set changed.
    ///
    /// The focus click lands at the element's centre and therefore MOVES THE
    /// CARET before the chord fires — see the caret hazard in the module doc
    /// before writing an oracle against this verb.
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
                "WebUserDriver::send_key_chord got extra_params {:?} — the DOM path has no \
                 channel for them; refusing to drop them silently.",
                extra_params.keys().collect::<Vec<_>>()
            );
        }
        let (modifier_keys, plain_keys): (Vec<_>, Vec<_>) = chord.0.iter().partition(|k| {
            matches!(
                k,
                holon_api::Key::Cmd
                    | holon_api::Key::Ctrl
                    | holon_api::Key::Alt
                    | holon_api::Key::Shift
            )
        });
        let [key] = plain_keys.as_slice() else {
            bail!(
                "send_key_chord needs exactly one non-modifier key, got {:?}",
                plain_keys
            );
        };
        let key = key.to_string();
        let modifier_names: Vec<String> = modifier_keys.iter().map(|k| k.to_string()).collect();
        let modifiers: Vec<&str> = modifier_names.iter().map(String::as_str).collect();

        self.click_entity(entity_id, "main").await?;
        let before = self.render_key(&self.snapshot());
        self.dispatch_key(&key, &modifiers).await?;
        Ok(self.render_key(&self.snapshot()) != before)
    }

    async fn send_raw_keystroke(&self, keystroke: &str, modifiers: &[&str]) -> Result<()> {
        self.dispatch_key(keystroke, modifiers).await
    }

    async fn scroll_to_entity(&self, entity_id: &EntityUri) -> Result<()> {
        self.find_clickable(entity_id, "main")
            .await?
            .scroll_into_view()
            .await?;
        Ok(())
    }

    async fn drop_entity(&self, _: &EntityUri, _: &EntityUri, _: &EntityUri) -> Result<bool> {
        bail!(
            "WebUserDriver::drop_entity is not implemented — HTML5 drag-and-drop needs its own \
             CDP gesture sequence (design increment 4)."
        )
    }

    fn is_widget_visible(&self, entity_id: &EntityUri) -> bool {
        self.snapshot()
            .iter()
            .any(|n| n.id == entity_id.as_str() && n.visible)
    }

    fn displayed_text(&self, entity_id: &EntityUri) -> Option<String> {
        self.snapshot()
            .into_iter()
            .find(|n| n.id == entity_id.as_str() && n.role != "selectable")
            .map(|n| n.text)
    }

    fn is_in_region(&self, _: &EntityUri, _: holon_api::Region) -> bool {
        unimplemented!(
            "WebUserDriver region verbs are increment 2 — the dioxus renderer emits no region \
             marker, so region membership must come from the MCP-relay oracle."
        )
    }

    fn entities_in_region(&self, _: holon_api::Region) -> Vec<EntityUri> {
        unimplemented!("WebUserDriver::entities_in_region is increment 2 (see is_in_region)")
    }

    fn reachable_entities_in_region(&self, _: holon_api::Region) -> Vec<EntityUri> {
        unimplemented!(
            "WebUserDriver::reachable_entities_in_region is increment 2 (see is_in_region)"
        )
    }

    fn click_intent_of(&self, _: &EntityUri) -> Option<OperationIntent> {
        unimplemented!(
            "WebUserDriver::click_intent_of is increment 2 — the bound intent lives in the page's \
             ViewModel, readable only over the MCP relay."
        )
    }
}
