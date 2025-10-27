//! UiDriver trait — abstracts how UI interactions are dispatched during PBT.
//!
//! Three levels:
//! 1. **FfiDriver**: All operations go through FFI fallback (headless testing).
//! 2. **GeometryDriver**: Uses element bounds + input simulation for supported ops.
//! 3. Future: PeekabooDriver for VLM-based interaction with any app.

use std::collections::HashMap;
use std::path::PathBuf;

use holon_api::Value;
use holon_frontend::geometry::{ElementRect, GeometryProvider};

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
}

pub struct CapturedScreenshot {
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// xcap-based window capture — finds the app window by title substring and captures it.
pub struct XcapBackend {
    window_title: String,
}

impl XcapBackend {
    pub fn new(window_title: impl Into<String>) -> Self {
        Self {
            window_title: window_title.into(),
        }
    }
}

impl ScreenshotBackend for XcapBackend {
    fn capture(&self) -> Option<CapturedScreenshot> {
        let windows = match xcap::Window::all() {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[xcap] failed to enumerate windows: {e}");
                return None;
            }
        };

        let window = windows.iter().find(|w| {
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

        let window = match window {
            Some(w) => w,
            None => {
                let info: Vec<_> = windows
                    .iter()
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
                return None;
            }
        };

        match window.capture_image() {
            Ok(img) => {
                let width = img.width();
                let height = img.height();
                let data = img.into_raw();
                Some(CapturedScreenshot {
                    data,
                    width,
                    height,
                })
            }
            Err(e) => {
                eprintln!("[xcap] capture_image failed: {e}");
                None
            }
        }
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

/// Geometry-based driver — queries element bounds and simulates input via enigo.
///
/// For supported operations (e.g. clicking on an element, typing text),
/// uses the `GeometryProvider` to find the element's position and simulates
/// mouse/keyboard input. Falls back to FFI for unsupported operations.
pub struct GeometryDriver {
    geometry: Box<dyn GeometryProvider>,
    screenshots: Option<ScreenshotConfig>,
}

struct ScreenshotConfig {
    backend: Box<dyn ScreenshotBackend>,
    dir: PathBuf,
    step_counter: u32,
}

impl GeometryDriver {
    pub fn new(geometry: Box<dyn GeometryProvider>) -> Self {
        Self {
            geometry,
            screenshots: None,
        }
    }

    /// Enable screenshot capture using the given backend and output directory.
    pub fn with_screenshots(mut self, backend: Box<dyn ScreenshotBackend>, dir: PathBuf) -> Self {
        std::fs::create_dir_all(&dir).expect("failed to create screenshot directory");
        self.screenshots = Some(ScreenshotConfig {
            backend,
            dir,
            step_counter: 0,
        });
        self
    }

    /// Capture a screenshot, optionally highlighting an element.
    /// Returns the path to the saved screenshot.
    pub fn capture_screenshot(
        &mut self,
        label: &str,
        highlight_element: Option<&str>,
    ) -> Option<PathBuf> {
        let config = self.screenshots.as_mut()?;
        config.step_counter += 1;

        let filename = format!("step-{:03}-{}.png", config.step_counter, sanitize(label));
        let final_path = config.dir.join(&filename);

        let captured = config.backend.capture()?;
        let bounds = highlight_element.and_then(|id| self.geometry.element_bounds(id));

        save_screenshot(&captured, &final_path, bounds);

        if let Some(rect) = bounds {
            eprintln!(
                "[screenshot] {filename} — highlighted {} at ({:.0}, {:.0}, {:.0}x{:.0})",
                highlight_element.unwrap_or("?"),
                rect.x,
                rect.y,
                rect.width,
                rect.height,
            );
        } else {
            eprintln!("[screenshot] {filename}");
        }

        Some(final_path)
    }
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

/// Save a captured screenshot as PNG, optionally with a highlight rectangle.
fn save_screenshot(captured: &CapturedScreenshot, path: &PathBuf, highlight: Option<ElementRect>) {
    use image::{ImageBuffer, Rgba, RgbaImage};

    let mut img: RgbaImage =
        ImageBuffer::from_raw(captured.width, captured.height, captured.data.clone())
            .expect("CapturedScreenshot dimensions don't match data length");

    if let Some(rect) = highlight {
        let (img_w, img_h) = (img.width(), img.height());

        // xcap captures at physical pixel resolution — coordinates may need scaling.
        // Detect scale: if image is ~2x the logical bounds, use 2x.
        let scale = if img_w > 2000 { 2.0_f32 } else { 1.0 };
        let x = (rect.x * scale) as u32;
        let y = (rect.y * scale) as u32;
        let w = (rect.width * scale) as u32;
        let h = (rect.height * scale) as u32;

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

    img.save(path).expect("failed to save screenshot PNG");
}

// ──── Input simulation (enigo) ────

#[cfg(feature = "enigo")]
impl GeometryDriver {
    fn simulate_set_content(&self, id: &str, new_value: &str) -> bool {
        let bounds = match self.geometry.element_bounds(id) {
            Some(b) => b,
            None => return false,
        };

        let (cx, cy) = bounds.center();

        use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};

        let mut enigo = match Enigo::new(&Settings::default()) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("[GeometryDriver] failed to create Enigo: {e}");
                return false;
            }
        };

        if let Err(e) = enigo.move_mouse(cx as i32, cy as i32, Coordinate::Abs) {
            eprintln!("[GeometryDriver] move_mouse failed: {e}");
            return false;
        }
        if let Err(e) = enigo.button(Button::Left, Direction::Click) {
            eprintln!("[GeometryDriver] click failed: {e}");
            return false;
        }

        std::thread::sleep(std::time::Duration::from_millis(50));

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

        eprintln!("[GeometryDriver] simulated set_content at ({cx:.0}, {cy:.0}) for element {id}",);
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
                    match self.geometry.element_bounds(&element_id) {
                        Some(bounds) => {
                            let (cx, cy) = bounds.center();
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
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    fn screenshot(&mut self, label: &str, highlight_element: Option<&str>) {
        self.capture_screenshot(label, highlight_element);
    }
}
