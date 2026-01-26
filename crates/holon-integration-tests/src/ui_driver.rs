//! UiDriver trait — abstracts how UI interactions are dispatched during PBT.
//!
//! Three levels:
//! 1. **FfiDriver**: All operations go through FFI fallback (headless testing).
//! 2. **GeometryDriver**: Uses element bounds + input simulation for supported ops.
//! 3. Future: PeekabooDriver for VLM-based interaction with any app.

use std::collections::HashMap;
use std::path::PathBuf;

use holon_api::Value;
use holon_frontend::geometry::{ElementInfo, GeometryProvider};
use holon_frontend::navigation::NavDirection;

use crate::screenshot_overlay::{Overlay, Phase};

// ──── Screenshot backend trait ────

/// Captures a screenshot and returns raw RGBA8 pixel data.
///
/// Frontends plug in their own backend:
/// - `XcapBackend`: window-level capture via xcap (default, works for any frontend)
/// - Future: Blinc framebuffer readback when wgpu integration lands
pub trait ScreenshotBackend: Send + Sync {
    /// Capture and return raw RGBA8 pixel data + dimensions.
    /// Returns `None` if capture fails.
    fn capture(&self) -> Option<CapturedScreenshot>;

    /// The window title used for capture. Needed by the SIGUSR1 watcher to
    /// create its own backend instance on a separate thread.
    fn window_title(&self) -> String;
}

#[derive(Clone)]
pub struct CapturedScreenshot {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// xcap-based window capture — finds the app window by title substring and captures it.
///
/// `xcap::Window::all()` enumerates every OS window on each call (~700 ms on
/// macOS in our PBT runs). The window we want is stable for the lifetime of
/// the test, so cache the matched handle and only re-enumerate if the cached
/// handle's `capture_image` fails (window closed, recreated, etc.).
pub struct XcapBackend {
    window_title: String,
    cached: std::sync::Mutex<Option<xcap::Window>>,
}

impl XcapBackend {
    pub fn new(window_title: impl Into<String>) -> Self {
        Self {
            window_title: window_title.into(),
            cached: std::sync::Mutex::new(None),
        }
    }

    fn find_window(&self) -> Option<xcap::Window> {
        let windows = match xcap::Window::all() {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[xcap] failed to enumerate windows: {e}");
                return None;
            }
        };
        let matched = windows.into_iter().find(|w| {
            let title_match = w
                .title()
                .map(|t| t.contains(&self.window_title))
                .unwrap_or(false);
            let app_match = w
                .app_name()
                .map(|a| a.contains(&self.window_title))
                .unwrap_or(false);
            title_match || app_match
        });
        if matched.is_none() {
            // Re-enumerate just for the diagnostic dump on miss.
            let info: Vec<_> = xcap::Window::all()
                .unwrap_or_default()
                .iter()
                // ALLOW(filter_map_ok): OS window queries
                .filter_map(|w| {
                    let title = w.title().ok()?;
                    let app = w.app_name();
                    Some(format!("{title:?} (app={app:?})"))
                })
                .collect();
            eprintln!(
                "[xcap] no window matching {:?} found. Available: {:?}",
                self.window_title, info
            );
        }
        matched
    }
}

impl ScreenshotBackend for XcapBackend {
    #[tracing::instrument(level = "info", skip_all, name = "pbt.xcap_capture")]
    fn capture(&self) -> Option<CapturedScreenshot> {
        // Try the cached window first; on capture failure, re-enumerate
        // and retry once.
        let mut guard = self.cached.lock().unwrap();
        for attempt in 0..2 {
            if guard.is_none() {
                *guard = self.find_window();
            }
            let Some(window) = guard.as_ref() else {
                return None;
            };
            match window.capture_image() {
                Ok(img) => {
                    let width = img.width();
                    let height = img.height();
                    let data = img.into_raw();
                    return Some(CapturedScreenshot {
                        data,
                        width,
                        height,
                    });
                }
                Err(e) => {
                    eprintln!(
                        "[xcap] capture_image failed (attempt {attempt}): {e} — invalidating cache"
                    );
                    *guard = None;
                }
            }
        }
        None
    }

    fn window_title(&self) -> String {
        self.window_title.clone()
    }
}

// ──── UiDriver trait ────

/// How UI interactions are dispatched during PBT testing.
///
/// Returns `true` if the interaction was handled via the UI,
/// `false` to fall back to FFI (direct engine operation).
#[async_trait::async_trait]
pub trait UiDriver: Send + Sync {
    /// Try to execute an operation via the UI.
    async fn try_ui_interaction(
        &mut self,
        entity: &str,
        op: &str,
        params: &HashMap<String, Value>,
    ) -> bool;

    /// Wait for UI to settle after an interaction.
    async fn settle(&mut self);

    /// Capture a screenshot of the current state, optionally highlighting an element.
    /// Default implementation does nothing.
    fn screenshot(&mut self, _label: &str, _highlight_element: Option<&str>) {}

    /// Capture a screenshot with a translucent overlay (action banner +
    /// pass/fail badge + optional assertion text). Default implementation
    /// falls back to `screenshot()` so non-overlay-aware drivers still work.
    fn screenshot_overlay(
        &mut self,
        label: &str,
        _phase: Phase,
        highlight_element: Option<&str>,
        _overlay: &Overlay,
    ) {
        self.screenshot(label, highlight_element);
    }

    /// Click on an element to focus it. Returns true if element was found and clicked.
    async fn click_element(&mut self, _id: &str) -> bool {
        false
    }

    /// Send an arrow key for cross-block navigation.
    async fn send_arrow(&mut self, _direction: NavDirection) {}
}

// ──── FFI-only driver ────

/// FFI-only driver — all operations fall back to direct engine execution.
pub struct FfiDriver;

#[async_trait::async_trait]
impl UiDriver for FfiDriver {
    async fn try_ui_interaction(
        &mut self,
        _entity: &str,
        _op: &str,
        _params: &HashMap<String, Value>,
    ) -> bool {
        false
    }

    async fn settle(&mut self) {}
}

// ──── Geometry-based driver with screenshot support ────

/// Shared state holding the most recent screenshot analysis. Written by
/// `GeometryDriver::capture_screenshot`, read by inv14 in the PBT checker.
pub type VisualState = std::sync::Arc<std::sync::Mutex<Option<ScreenshotEmptiness>>>;

/// Geometry-based driver — queries element bounds and simulates input via enigo.
///
/// For supported operations (e.g. clicking on an element, typing text),
/// uses the `GeometryProvider` to find the element's position and simulates
/// mouse/keyboard input. Falls back to FFI for unsupported operations.
pub struct GeometryDriver {
    geometry: Box<dyn GeometryProvider>,
    screenshots: Option<ScreenshotConfig>,
    visual_state: Option<VisualState>,
}

struct ScreenshotConfig {
    backend: Box<dyn ScreenshotBackend>,
    dir: PathBuf,
    step_counter: u32,
    /// Pending PNG-encode-and-write jobs running on background threads.
    /// Joined in `Drop` so the test process doesn't exit before the
    /// last screenshots hit disk.
    pending_saves: Vec<std::thread::JoinHandle<()>>,
}

impl Drop for ScreenshotConfig {
    fn drop(&mut self) {
        // Drain any in-flight encode jobs.
        for h in self.pending_saves.drain(..) {
            let _ = h.join();
        }
    }
}

impl GeometryDriver {
    pub fn new(geometry: Box<dyn GeometryProvider>) -> Self {
        Self {
            geometry,
            screenshots: None,
            visual_state: None,
        }
    }

    /// Enable screenshot capture using the given backend and output directory.
    pub fn with_screenshots(mut self, backend: Box<dyn ScreenshotBackend>, dir: PathBuf) -> Self {
        std::fs::create_dir_all(&dir).expect("failed to create screenshot directory");
        self.screenshots = Some(ScreenshotConfig {
            backend,
            dir,
            step_counter: 0,
            pending_saves: Vec::new(),
        });
        self
    }

    /// Share a `VisualState` so inv14 can read the last screenshot analysis.
    pub fn with_visual_state(mut self, state: VisualState) -> Self {
        self.visual_state = Some(state);
        self
    }

    /// Capture a screenshot, optionally highlighting an element.
    /// Returns the path to the saved screenshot.
    pub fn capture_screenshot(
        &mut self,
        label: &str,
        highlight_element: Option<&str>,
    ) -> Option<PathBuf> {
        self.capture_internal(label, None, highlight_element, None)
    }

    /// Capture a screenshot with a translucent overlay (action banner +
    /// optional pass/fail badge + optional assertion text). Pre/Post for
    /// the same logical step share a step counter; filenames are
    /// `{millis}-step-{NNN}-{pre|post}-{label}.png` so a per-step pair sorts
    /// in capture order.
    pub fn capture_screenshot_with_overlay(
        &mut self,
        label: &str,
        phase: Phase,
        highlight_element: Option<&str>,
        overlay: &Overlay,
    ) -> Option<PathBuf> {
        self.capture_internal(label, Some(phase), highlight_element, Some(overlay))
    }

    #[tracing::instrument(level = "info", skip_all, name = "pbt.screenshot", fields(label = %label, phase = ?phase))]
    fn capture_internal(
        &mut self,
        label: &str,
        phase: Option<Phase>,
        highlight_element: Option<&str>,
        overlay: Option<&Overlay>,
    ) -> Option<PathBuf> {
        let config = self.screenshots.as_mut()?;

        // Pre advances the counter; Post reuses it so a per-step pair shares
        // the same NNN. Plain capture_screenshot (no phase) always advances.
        match phase {
            None | Some(Phase::Pre) => config.step_counter += 1,
            Some(Phase::Post) => {}
        }
        let step = config.step_counter;

        let millis = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis();
        let filename = match phase {
            Some(p) => format!(
                "{millis}-step-{:03}-{}-{}.png",
                step,
                p.as_str(),
                sanitize(label)
            ),
            None => format!("{millis}-step-{:03}-{}.png", step, sanitize(label)),
        };
        let final_path = config.dir.join(&filename);

        let captured = config.backend.capture()?;
        let info = highlight_element.and_then(|id| self.geometry.element_info(id));

        // Analyze screenshot for "mostly empty" UI — the ground truth for visible
        // content. BoundsRegistry reports layout, not paint, and stale/clipped
        // elements can linger there even when the window is visibly blank.
        let emptiness = analyze_screenshot_emptiness(&captured);
        if let Some(ref state) = self.visual_state {
            *state.lock().unwrap() = Some(emptiness);
        }

        // PNG encode + disk write is ~1 s on the main thread. Hand off
        // to a worker so the next transition's `apply()` can start
        // immediately. The worker handles are drained in
        // `ScreenshotConfig::drop` so the process can't exit before
        // the last shots are flushed. Cap concurrency at 4 in-flight
        // jobs by joining the oldest if we're at the limit — this
        // bounds memory (each `CapturedScreenshot` is ~30 MB) without
        // serialising the common case.
        const MAX_INFLIGHT_SAVES: usize = 4;
        if config.pending_saves.len() >= MAX_INFLIGHT_SAVES
            && let Some(oldest) = config.pending_saves.drain(..1).next()
        {
            let _ = oldest.join();
        }
        // Drop completed jobs so the vec doesn't grow unbounded.
        config.pending_saves.retain(|h| !h.is_finished());

        let cap_clone = captured.clone();
        let path_clone = final_path.clone();
        let info_clone = info.clone();
        let overlay_clone = overlay.cloned();
        let handle = std::thread::spawn(move || {
            save_screenshot(
                &cap_clone,
                &path_clone,
                info_clone.as_ref(),
                overlay_clone.as_ref(),
            );
        });
        config.pending_saves.push(handle);

        if let Some(info) = &info {
            eprintln!(
                "[screenshot] {filename} — highlighted {} at ({:.0}, {:.0}, {:.0}x{:.0}) content_frac={:.4}",
                highlight_element.unwrap_or("?"),
                info.x,
                info.y,
                info.width,
                info.height,
                emptiness.content_fraction,
            );
        } else {
            eprintln!(
                "[screenshot] {filename} content_frac={:.4}",
                emptiness.content_fraction,
            );
        }

        Some(final_path)
    }

    /// Spawn a background thread that captures a screenshot on each SIGUSR1.
    ///
    /// Useful for debugging stuck tests — send `kill -USR1 <pid>` from another
    /// terminal to capture the current window state without waiting for the
    /// next transition boundary.
    ///
    /// The returned handle keeps the watcher alive; drop it to stop.
    pub fn spawn_signal_watcher(&self) -> Option<SignalScreenshotWatcher> {
        let config = self.screenshots.as_ref()?;
        let dir = config.dir.clone();
        let window_title = config.backend.window_title();

        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        signal_hook::flag::register(signal_hook::consts::SIGUSR1, flag.clone())
            .expect("failed to register SIGUSR1 handler");

        let handle = std::thread::spawn(move || {
            let backend = XcapBackend::new(window_title);
            let mut counter = 0u32;
            loop {
                std::thread::sleep(std::time::Duration::from_millis(250));
                if flag.swap(false, std::sync::atomic::Ordering::SeqCst) {
                    counter += 1;
                    let millis = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis();
                    let filename = format!("{millis}-signal-{counter:03}.png");
                    let path = dir.join(&filename);
                    match backend.capture() {
                        Some(captured) => {
                            save_screenshot(&captured, &path, None, None);
                            eprintln!("[screenshot] SIGUSR1 capture: {filename}");
                        }
                        None => eprintln!("[screenshot] SIGUSR1 capture failed — window not found"),
                    }
                }
            }
        });

        eprintln!(
            "[screenshot] SIGUSR1 watcher active — send `kill -USR1 {}` to capture",
            std::process::id()
        );

        Some(SignalScreenshotWatcher { _handle: handle })
    }
}

/// Guard that keeps the SIGUSR1 watcher thread alive. The thread runs until
/// the process exits (it's a daemon thread with no explicit shutdown).
pub struct SignalScreenshotWatcher {
    _handle: std::thread::JoinHandle<()>,
}

/// Result of analyzing a captured screenshot for emptiness.
#[derive(Debug, Clone, Copy)]
pub struct ScreenshotEmptiness {
    /// Fraction of pixels in the content area (below the title bar) that differ
    /// noticeably from the background color. Range [0.0, 1.0]. Values near 0
    /// indicate a mostly-empty UI.
    pub content_fraction: f32,
}

/// Analyze a captured screenshot for the fraction of "content" pixels — pixels
/// that are brighter than the background threshold. Skips the top strip
/// (title bar) to avoid false positives from window chrome.
#[tracing::instrument(level = "info", skip_all, name = "pbt.analyze_emptiness")]
pub fn analyze_screenshot_emptiness(captured: &CapturedScreenshot) -> ScreenshotEmptiness {
    let width = captured.width as usize;
    let height = captured.height as usize;
    if width == 0 || height == 0 {
        return ScreenshotEmptiness {
            content_fraction: 0.0,
        };
    }

    // Skip the top ~40 logical pixels for title bar + toolbar; at retina 2x
    // that is 80 physical pixels. We over-skip by using 80 unconditionally;
    // the content area below still covers the vast majority of the window.
    let skip_y = 80usize.min(height / 10);
    let data = &captured.data;

    // Background threshold: a pixel is "background" if all RGB channels are
    // below 45 (dark background) OR if all channels are within a tight range
    // near the dominant dark color. We use a simple brightness check.
    let brightness_threshold: u32 = 45;

    let mut total = 0usize;
    let mut content = 0usize;
    for y in skip_y..height {
        for x in 0..width {
            let idx = (y * width + x) * 4;
            if idx + 2 >= data.len() {
                break;
            }
            let r = data[idx] as u32;
            let g = data[idx + 1] as u32;
            let b = data[idx + 2] as u32;
            total += 1;
            if r > brightness_threshold || g > brightness_threshold || b > brightness_threshold {
                content += 1;
            }
        }
    }

    let content_fraction = if total == 0 {
        0.0
    } else {
        content as f32 / total as f32
    };
    ScreenshotEmptiness { content_fraction }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Save a captured screenshot as PNG, with an optional element highlight
/// rectangle and an optional translucent overlay (action banner +
/// pass/fail badge + assertion text).
#[tracing::instrument(level = "info", skip_all, name = "pbt.save_screenshot")]
pub(crate) fn save_screenshot(
    captured: &CapturedScreenshot,
    path: &PathBuf,
    highlight: Option<&ElementInfo>,
    overlay: Option<&Overlay>,
) {
    use image::{ImageBuffer, Rgba, RgbaImage};

    let mut img: RgbaImage =
        ImageBuffer::from_raw(captured.width, captured.height, captured.data.clone())
            .expect("CapturedScreenshot dimensions don't match data length");

    if let Some(info) = highlight {
        let (img_w, img_h) = (img.width(), img.height());

        // xcap captures at physical pixel resolution — coordinates may need scaling.
        // Detect scale: if image is ~2x the logical bounds, use 2x.
        let scale = if img_w > 2000 { 2.0_f32 } else { 1.0 };
        let x = (info.x * scale) as u32;
        let y = (info.y * scale) as u32;
        let w = (info.width * scale) as u32;
        let h = (info.height * scale) as u32;

        let color = Rgba([255, 50, 50, 255]);
        let border = 3u32;

        // Top and bottom edges
        for dx in x.saturating_sub(border)..=(x + w + border).min(img_w.saturating_sub(1)) {
            for t in 0..border {
                let top_y = y.saturating_sub(border) + t;
                let bot_y = (y + h + t).min(img_h.saturating_sub(1));
                if top_y < img_h {
                    img.put_pixel(dx.min(img_w - 1), top_y, color);
                }
                if bot_y < img_h {
                    img.put_pixel(dx.min(img_w - 1), bot_y, color);
                }
            }
        }
        // Left and right edges
        for dy in y.saturating_sub(border)..=(y + h + border).min(img_h.saturating_sub(1)) {
            for t in 0..border {
                let left_x = x.saturating_sub(border) + t;
                let right_x = (x + w + t).min(img_w.saturating_sub(1));
                if left_x < img_w {
                    img.put_pixel(left_x, dy.min(img_h - 1), color);
                }
                if right_x < img_w {
                    img.put_pixel(right_x, dy.min(img_h - 1), color);
                }
            }
        }
    }

    if let Some(overlay) = overlay {
        crate::screenshot_overlay::paint_overlay(&mut img, overlay);
    }

    img.save(path).expect("failed to save screenshot PNG");
}

// ──── Input simulation (enigo) ────

#[cfg(feature = "enigo")]
impl GeometryDriver {
    /// Create an enigo instance. Panics on failure.
    fn enigo() -> enigo::Enigo {
        enigo::Enigo::new(&enigo::Settings::default()).expect("failed to create Enigo")
    }

    /// Click at absolute screen coordinates via enigo.
    fn click_at(enigo: &mut enigo::Enigo, x: f32, y: f32) {
        use enigo::{Button, Coordinate, Direction, Mouse};
        enigo
            .move_mouse(x as i32, y as i32, Coordinate::Abs)
            .expect("enigo move_mouse failed");
        enigo
            .button(Button::Left, Direction::Click)
            .expect("enigo click failed");
    }

    /// Click on an element's center. Panics if element not in BoundsRegistry.
    fn enigo_click_element(&self, id: &str) {
        let info = self.geometry.element_info(id).unwrap_or_else(|| {
            panic!("[GeometryDriver] element {id} not found in BoundsRegistry — stale geometry")
        });
        let (cx, cy) = info.center();
        let mut enigo = Self::enigo();
        Self::click_at(&mut enigo, cx, cy);
        eprintln!("[GeometryDriver] clicked element {id} at ({cx:.0}, {cy:.0})");
    }

    /// Send an arrow key via enigo.
    fn enigo_send_arrow(&self, direction: NavDirection) {
        use enigo::{Direction as Dir, Key, Keyboard};
        let key = match direction {
            NavDirection::Up => Key::UpArrow,
            NavDirection::Down => Key::DownArrow,
            NavDirection::Left => Key::LeftArrow,
            NavDirection::Right => Key::RightArrow,
        };
        let mut enigo = Self::enigo();
        enigo.key(key, Dir::Click).expect("enigo arrow key failed");
    }

    fn simulate_set_content(&self, id: &str, new_value: &str) -> bool {
        let info = match self.geometry.element_info(id) {
            Some(i) => i,
            None => return false,
        };

        let (cx, cy) = info.center();

        use enigo::{Direction, Key, Keyboard};

        let mut enigo = Self::enigo();

        // Click to focus the element
        Self::click_at(&mut enigo, cx, cy);

        std::thread::sleep(std::time::Duration::from_millis(50));

        // Select all + type replacement text
        #[cfg(target_os = "macos")]
        let select_all_modifier = Key::Meta;
        #[cfg(not(target_os = "macos"))]
        let select_all_modifier = Key::Control;

        let _ = enigo.key(select_all_modifier, Direction::Press);
        let _ = enigo.key(Key::Unicode('a'), Direction::Click);
        let _ = enigo.key(select_all_modifier, Direction::Release);

        if let Err(e) = enigo.text(new_value) {
            eprintln!("[GeometryDriver] text input failed: {e}");
            return false;
        }

        eprintln!("[GeometryDriver] simulated set_content at ({cx:.0}, {cy:.0}) for element {id}");
        true
    }
}

// ──── UiDriver impl ────

#[async_trait::async_trait]
impl UiDriver for GeometryDriver {
    async fn try_ui_interaction(
        &mut self,
        entity: &str,
        op: &str,
        params: &HashMap<String, Value>,
    ) -> bool {
        let element_id = params
            .get("id")
            .and_then(|v| v.as_string())
            .unwrap_or_default();

        match op {
            "set_field" => {
                let field = params
                    .get("field")
                    .and_then(|v| v.as_string())
                    .unwrap_or_default();
                if field != "content" {
                    return false;
                }

                if element_id.is_empty() {
                    return false;
                }

                #[cfg(feature = "enigo")]
                {
                    let new_value = params
                        .get("value")
                        .and_then(|v| v.as_string())
                        .unwrap_or_default();
                    return self.simulate_set_content(&element_id, &new_value);
                }

                #[cfg(not(feature = "enigo"))]
                {
                    match self.geometry.element_info(element_id) {
                        Some(info) => {
                            let (cx, cy) = info.center();
                            eprintln!(
                                "[GeometryDriver] element {element_id} at ({cx:.0}, {cy:.0}) — \
                                 enigo feature disabled, falling back to FFI",
                            );
                        }
                        None => {
                            eprintln!(
                                "[GeometryDriver] element {element_id} not found for {entity}.{op}"
                            );
                        }
                    }
                    return false;
                }
            }
            _ => false,
        }
    }

    async fn settle(&mut self) {
        // GPUI debug builds are ~10x slower than release. The render pipeline:
        // mutation → CDC → watcher signal → ReactiveEngine → structural signal
        // → watch_live → GPUI cx.spawn wakeup → render → prepaint → record.
        // 200ms is enough for release, but debug needs ~500ms for the full
        // pipeline to deliver data to the BoundsRegistry.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    fn screenshot(&mut self, label: &str, highlight_element: Option<&str>) {
        self.capture_screenshot(label, highlight_element);
    }

    fn screenshot_overlay(
        &mut self,
        label: &str,
        phase: Phase,
        highlight_element: Option<&str>,
        overlay: &Overlay,
    ) {
        self.capture_screenshot_with_overlay(label, phase, highlight_element, overlay);
    }

    #[cfg(feature = "enigo")]
    async fn click_element(&mut self, id: &str) -> bool {
        self.enigo_click_element(id);
        true
    }

    #[cfg(feature = "enigo")]
    async fn send_arrow(&mut self, direction: NavDirection) {
        self.enigo_send_arrow(direction);
    }
}
