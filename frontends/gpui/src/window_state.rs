//! Desktop window-state persistence: remember the window's position, size, and
//! presentation mode (windowed / maximized / fullscreen) across restarts.
//!
//! The on-disk file lives next to `holon.toml` in the app's config dir and is
//! *derived / ephemeral* state — a corrupt or missing file must never crash the
//! app. We fall back to platform defaults, loudly (WARN) on corruption.
//!
//! The one classic bug this feature must avoid is restoring a window off-screen
//! (e.g. onto a monitor that was unplugged). [`clamp_bounds`] is the pure
//! function that guarantees the restored rectangle sits fully inside a
//! currently-connected display; it is unit-tested in isolation.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// File name inside the config dir (sibling of `holon.toml`).
const WINDOW_STATE_FILE: &str = "window_state.json";

/// Smallest window we will ever restore to, in logical pixels. Guards against a
/// corrupt tiny/zero size producing an invisible or unusable window.
const MIN_WIDTH: f32 = 300.0;
const MIN_HEIGHT: f32 = 200.0;

/// Which of the three top-level presentation modes the window was in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMode {
    Windowed,
    Maximized,
    Fullscreen,
}

/// A rectangle in global (possibly multi-display) logical-pixel coordinates.
/// Plain `f32`s keep the persisted format decoupled from gpui's geometry types.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SavedBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl SavedBounds {
    /// Build from a gpui pixel-space rectangle.
    pub fn from_gpui(b: gpui::Bounds<gpui::Pixels>) -> Self {
        SavedBounds {
            x: f32::from(b.origin.x),
            y: f32::from(b.origin.y),
            width: f32::from(b.size.width),
            height: f32::from(b.size.height),
        }
    }

    /// Convert back into a gpui pixel-space rectangle.
    pub fn to_gpui(&self) -> gpui::Bounds<gpui::Pixels> {
        gpui::Bounds {
            origin: gpui::point(gpui::px(self.x), gpui::px(self.y)),
            size: gpui::size(gpui::px(self.width), gpui::px(self.height)),
        }
    }

    fn center(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    /// Area of the intersection of two rectangles (0.0 if disjoint).
    fn intersection_area(&self, other: &DisplayRect) -> f32 {
        let ix = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let iy = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        if ix <= 0.0 || iy <= 0.0 {
            0.0
        } else {
            ix * iy
        }
    }
}

/// A currently-connected display's usable rectangle plus its stable uuid (if the
/// platform provides one). The uuid lets us prefer the display the window was
/// last on even when its saved coordinates happen to overlap another monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayRect {
    pub uuid: Option<String>,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl DisplayRect {
    fn center(&self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// The full persisted window state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PersistedWindowState {
    pub mode: WindowMode,
    /// The *restore* rectangle: the windowed bounds to return to even when the
    /// window was maximized or fullscreen when saved.
    pub bounds: SavedBounds,
    /// Stable uuid of the display the window was on, if known.
    #[serde(default)]
    pub display_uuid: Option<String>,
}

impl PersistedWindowState {
    /// Snapshot the live window's presentation mode, restore bounds, and the
    /// display it is on.
    pub fn from_window(window: &gpui::Window, cx: &gpui::App) -> Self {
        let (mode, restore) = match window.window_bounds() {
            gpui::WindowBounds::Windowed(b) => (WindowMode::Windowed, b),
            gpui::WindowBounds::Maximized(b) => (WindowMode::Maximized, b),
            gpui::WindowBounds::Fullscreen(b) => (WindowMode::Fullscreen, b),
        };
        let display_uuid = window
            .display(cx)
            // ALLOW(ok): display uuid is optional metadata; absence => None
            .and_then(|d| d.uuid().ok())
            .map(|u| u.to_string());
        PersistedWindowState {
            mode,
            bounds: SavedBounds::from_gpui(restore),
            display_uuid,
        }
    }

    /// The `WindowBounds` to open with, clamped onto a currently-connected
    /// display so the window can never restore off-screen.
    pub fn to_window_bounds(&self, displays: &[DisplayRect]) -> gpui::WindowBounds {
        let clamped = clamp_bounds(self.bounds, self.display_uuid.as_deref(), displays);
        let b = clamped.to_gpui();
        match self.mode {
            WindowMode::Windowed => gpui::WindowBounds::Windowed(b),
            WindowMode::Maximized => gpui::WindowBounds::Maximized(b),
            WindowMode::Fullscreen => gpui::WindowBounds::Fullscreen(b),
        }
    }
}

/// Snapshot every currently-connected display's usable rectangle.
pub fn connected_displays(cx: &gpui::App) -> Vec<DisplayRect> {
    cx.displays()
        .iter()
        .map(|d| {
            let b = d.visible_bounds();
            DisplayRect {
                // ALLOW(ok): display uuid is optional metadata; absence => None
                uuid: d.uuid().ok().map(|u| u.to_string()),
                x: f32::from(b.origin.x),
                y: f32::from(b.origin.y),
                width: f32::from(b.size.width),
                height: f32::from(b.size.height),
            }
        })
        .collect()
}

/// Fit `saved` fully inside one currently-connected display, so a restored
/// window is never (even partially) off-screen.
///
/// Targeting: if `preferred_uuid` names a connected display, that display is the
/// sole candidate; otherwise every display is a candidate. Among candidates we
/// pick the one with the largest overlap with `saved`, falling back to the one
/// whose centre is nearest when there is no overlap at all (the unplugged-monitor
/// case). The rectangle is then shrunk to fit and shifted fully inside the
/// target.
///
/// With no displays at all the input is returned unchanged — the caller decides
/// what to do when the platform reports zero displays.
pub fn clamp_bounds(
    saved: SavedBounds,
    preferred_uuid: Option<&str>,
    displays: &[DisplayRect],
) -> SavedBounds {
    if displays.is_empty() {
        return saved;
    }

    let preferred: Vec<&DisplayRect> = preferred_uuid
        .and_then(|want| {
            let matches: Vec<&DisplayRect> = displays
                .iter()
                .filter(|d| d.uuid.as_deref() == Some(want))
                .collect();
            (!matches.is_empty()).then_some(matches)
        })
        .unwrap_or_else(|| displays.iter().collect());

    let target = preferred
        .iter()
        .copied()
        .max_by(|a, b| {
            let aa = saved.intersection_area(a);
            let ba = saved.intersection_area(b);
            aa.partial_cmp(&ba).unwrap_or(std::cmp::Ordering::Equal)
        })
        .expect("candidate set is non-empty");

    // If nothing overlaps the best-overlap target, re-target to the nearest
    // display by centre distance instead.
    let target = if saved.intersection_area(target) > 0.0 {
        target
    } else {
        let (sx, sy) = saved.center();
        preferred
            .iter()
            .copied()
            .min_by(|a, b| {
                let da = dist2(a.center(), (sx, sy));
                let db = dist2(b.center(), (sx, sy));
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("candidate set is non-empty")
    };

    let width = saved.width.clamp(MIN_WIDTH, target.width.max(MIN_WIDTH));
    let height = saved.height.clamp(MIN_HEIGHT, target.height.max(MIN_HEIGHT));
    // Shift the origin so the (possibly shrunk) rect lies fully within target.
    let max_x = target.x + (target.width - width).max(0.0);
    let max_y = target.y + (target.height - height).max(0.0);
    let x = saved.x.clamp(target.x, max_x);
    let y = saved.y.clamp(target.y, max_y);

    SavedBounds {
        x,
        y,
        width,
        height,
    }
}

fn dist2(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

/// Absolute path of the state file for a given config dir.
pub fn state_path(config_dir: &Path) -> PathBuf {
    config_dir.join(WINDOW_STATE_FILE)
}

/// Load persisted window state. Returns `None` when the file is absent (normal
/// first run — logged at debug) or unreadable/corrupt (logged at WARN, so the
/// degraded fall-to-defaults is disclosed rather than silent).
pub fn load(config_dir: &Path) -> Option<PersistedWindowState> {
    let path = state_path(config_dir);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            tracing::debug!(path = %path.display(), "no saved window state; using defaults");
            return None;
        }
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "reading window state failed — falling back to default window bounds"
            );
            return None;
        }
    };
    match serde_json::from_str::<PersistedWindowState>(&raw) {
        Ok(state) => Some(state),
        Err(e) => {
            tracing::warn!(
                path = %path.display(),
                error = %e,
                "window state file is corrupt — falling back to default window bounds"
            );
            None
        }
    }
}

/// Persist window state to `{config_dir}/window_state.json`. Errors are returned
/// (never swallowed) so callers can log with their own context.
pub fn save(config_dir: &Path, state: &PersistedWindowState) -> anyhow::Result<()> {
    let path = state_path(config_dir);
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| anyhow::anyhow!("serializing window state: {e}"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("creating config dir {}: {e}", parent.display()))?;
    }
    std::fs::write(&path, json)
        .map_err(|e| anyhow::anyhow!("writing window state {}: {e}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn display(uuid: &str, x: f32, y: f32, w: f32, h: f32) -> DisplayRect {
        DisplayRect {
            uuid: Some(uuid.to_string()),
            x,
            y,
            width: w,
            height: h,
        }
    }

    fn bounds(x: f32, y: f32, w: f32, h: f32) -> SavedBounds {
        SavedBounds {
            x,
            y,
            width: w,
            height: h,
        }
    }

    #[test]
    fn fully_visible_bounds_are_unchanged() {
        let displays = vec![display("A", 0.0, 0.0, 1920.0, 1080.0)];
        let saved = bounds(100.0, 100.0, 800.0, 600.0);
        assert_eq!(clamp_bounds(saved, None, &displays), saved);
    }

    #[test]
    fn partially_offscreen_right_is_pulled_back_in() {
        let displays = vec![display("A", 0.0, 0.0, 1920.0, 1080.0)];
        // Window hangs 300px past the right edge.
        let saved = bounds(1420.0, 100.0, 800.0, 600.0);
        let out = clamp_bounds(saved, None, &displays);
        assert!(out.x + out.width <= 1920.0, "right edge inside display: {out:?}");
        assert_eq!(out.width, 800.0, "size preserved when it fits");
        assert_eq!(out.x, 1120.0);
    }

    #[test]
    fn offscreen_negative_origin_is_clamped_to_display_origin() {
        let displays = vec![display("A", 0.0, 0.0, 1920.0, 1080.0)];
        let saved = bounds(-500.0, -400.0, 800.0, 600.0);
        let out = clamp_bounds(saved, None, &displays);
        assert_eq!(out.x, 0.0);
        assert_eq!(out.y, 0.0);
    }

    #[test]
    fn oversized_window_is_shrunk_to_display() {
        let displays = vec![display("A", 0.0, 0.0, 1280.0, 720.0)];
        let saved = bounds(0.0, 0.0, 4000.0, 3000.0);
        let out = clamp_bounds(saved, None, &displays);
        assert_eq!(out.width, 1280.0);
        assert_eq!(out.height, 720.0);
        assert_eq!(out.x, 0.0);
        assert_eq!(out.y, 0.0);
    }

    #[test]
    fn tiny_corrupt_size_is_floored_to_minimum() {
        let displays = vec![display("A", 0.0, 0.0, 1920.0, 1080.0)];
        let saved = bounds(10.0, 10.0, 1.0, 1.0);
        let out = clamp_bounds(saved, None, &displays);
        assert_eq!(out.width, MIN_WIDTH);
        assert_eq!(out.height, MIN_HEIGHT);
    }

    #[test]
    fn window_on_unplugged_monitor_moves_to_remaining_display() {
        // Saved on a secondary monitor at x=1920..; only the primary remains.
        let displays = vec![display("primary", 0.0, 0.0, 1920.0, 1080.0)];
        let saved = bounds(2200.0, 300.0, 800.0, 600.0);
        let out = clamp_bounds(saved, Some("secondary-gone"), &displays);
        assert!(out.x >= 0.0 && out.x + out.width <= 1920.0, "moved onto primary: {out:?}");
        assert!(out.y >= 0.0 && out.y + out.height <= 1080.0, "moved onto primary: {out:?}");
    }

    #[test]
    fn preferred_display_wins_over_geometric_overlap() {
        // Two displays side by side. Saved rect overlaps the LEFT one
        // geometrically, but the saved preferred uuid names the RIGHT one, so
        // the window should be clamped onto the right display.
        let displays = vec![
            display("left", 0.0, 0.0, 1000.0, 1000.0),
            display("right", 1000.0, 0.0, 1000.0, 1000.0),
        ];
        let saved = bounds(100.0, 100.0, 400.0, 400.0);
        let out = clamp_bounds(saved, Some("right"), &displays);
        assert!(out.x >= 1000.0, "clamped onto right display: {out:?}");
    }

    #[test]
    fn most_overlapping_display_is_chosen_without_preference() {
        let displays = vec![
            display("left", 0.0, 0.0, 1000.0, 1000.0),
            display("right", 1000.0, 0.0, 1000.0, 1000.0),
        ];
        // 700px of the 800px width sits on the right display.
        let saved = bounds(900.0, 100.0, 800.0, 400.0);
        let out = clamp_bounds(saved, None, &displays);
        assert!(out.x + out.width <= 2000.0);
        assert!(out.x >= 1000.0, "kept on the majority (right) display: {out:?}");
    }

    #[test]
    fn no_displays_returns_input_untouched() {
        let saved = bounds(100.0, 100.0, 800.0, 600.0);
        assert_eq!(clamp_bounds(saved, None, &[]), saved);
    }

    #[test]
    fn round_trips_through_json() {
        let state = PersistedWindowState {
            mode: WindowMode::Maximized,
            bounds: bounds(50.0, 60.0, 1024.0, 768.0),
            display_uuid: Some("abc-123".to_string()),
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: PersistedWindowState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }
}
