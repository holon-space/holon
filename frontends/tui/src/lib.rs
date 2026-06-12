//! @c4 container
//! @c4 layer UI
//! Pattern: MVVM View
//!
//! Terminal UI frontend — the MVVM **View** layer; its render functions build ratatui widgets from holon-frontend's `ReactiveViewModel`.

pub mod app_main;
pub mod di;
pub mod geometry;
pub mod input_pump;
pub mod keybindings;
pub mod render;
pub mod stylesheet;
pub mod user_driver;

/// Return the set of widget names this TUI frontend supports.
pub fn render_supported_widgets() -> std::collections::HashSet<String> {
    render::supported_widget_names()
        .iter()
        .map(|s| s.to_string())
        .collect()
}
