//! Pixel screenshot capture for the windowed composed PBT.
//!
//! Enabled only when `HOLON_CAPTURE_DIR` is set to a directory; otherwise the
//! sink is a no-op and the run costs nothing extra. When enabled, each capture
//! renders the current window frame offscreen (`HeadlessAppContext::
//! capture_screenshot` → the platform headless renderer, Metal on macOS),
//! composites the shared `screenshot_overlay` banner/badge, writes
//! `<seq>-<label>.png`, and appends one JSONL line to `events.jsonl` so the
//! report flipbook can reconstruct the run.
//!
//! SAFETY: `FrameSink` holds a raw `*const HeadlessAppContext` — the same
//! single-thread contract `SimUserDriver` relies on — and `capture` casts it to
//! `&mut` to call `capture_screenshot`. The owner (`with_windowed_wide_sut`)
//! pins `app` for the sink's lifetime and drives it only from the gpui thread.

use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use gpui::AnyWindowHandle;
use gpui::HeadlessAppContext;
use holon_integration_tests::screenshot_overlay::Overlay;
use holon_integration_tests::screenshot_overlay::paint_overlay;

/// Offscreen frame capturer over a headless-rendered window.
pub struct FrameSink {
    app_ptr: *const HeadlessAppContext,
    window: AnyWindowHandle,
    dir: Option<PathBuf>,
    seq: AtomicUsize,
}

impl FrameSink {
    /// Build a sink. Enabled iff `HOLON_CAPTURE_DIR` is set to a non-empty dir.
    pub fn new(app: &HeadlessAppContext, window: AnyWindowHandle) -> Self {
        let dir = std::env::var("HOLON_CAPTURE_DIR")
            .ok()
            .filter(|d| !d.is_empty())
            .map(PathBuf::from);
        if let Some(d) = &dir {
            if let Err(e) = std::fs::create_dir_all(d) {
                eprintln!(
                    "[capture] cannot create HOLON_CAPTURE_DIR {}: {e}",
                    d.display()
                );
            }
        }
        Self {
            app_ptr: app,
            window,
            dir,
            seq: AtomicUsize::new(0),
        }
    }

    pub fn enabled(&self) -> bool {
        self.dir.is_some()
    }

    /// Banner-only frame (a step with no pass/fail verdict).
    pub fn capture_action(&self, label: &str) {
        self.capture(label, Overlay::action(label));
    }

    /// Post-step frame with a green pass badge.
    pub fn capture_pass(&self, label: &str) {
        self.capture(label, Overlay::pass(label));
    }

    /// Failure frame with a red badge + assertion text.
    pub fn capture_fail(&self, label: &str, assertion: &str) {
        self.capture(label, Overlay::fail(label, assertion));
    }

    fn capture(&self, label: &str, overlay: Overlay) {
        let Some(dir) = &self.dir else {
            return;
        };
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        // SAFETY: see module doc — single-thread, app pinned for the sink's life.
        let app = unsafe { &mut *(self.app_ptr as *mut HeadlessAppContext) };
        let mut img = match app.capture_screenshot(self.window) {
            Ok(img) => img,
            Err(e) => {
                eprintln!("[capture] render failed (seq {seq}): {e:#}");
                return;
            }
        };
        paint_overlay(&mut img, &overlay);
        let fname = format!("{seq:04}-{}.png", sanitize(label));
        let path = dir.join(&fname);
        if let Err(e) = img.save(&path) {
            eprintln!("[capture] save {} failed: {e}", path.display());
            return;
        }
        self.append_event(seq, label, &fname);
    }

    fn append_event(&self, seq: usize, label: &str, png: &str) {
        let Some(dir) = &self.dir else {
            return;
        };
        use std::io::Write;
        let line = format!("{{\"seq\":{seq},\"label\":{label:?},\"png\":{png:?}}}\n");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("events.jsonl"))
        {
            let _ = f.write_all(line.as_bytes());
        }
    }
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
