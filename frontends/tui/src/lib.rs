pub mod app_main;
pub mod di;
pub mod geometry;
pub mod input_pump;
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
