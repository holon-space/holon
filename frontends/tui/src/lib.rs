pub mod app_main;
pub mod render;
pub mod state;
pub mod stylesheet;

/// Return the set of widget names this TUI frontend supports.
pub fn render_supported_widgets() -> std::collections::HashSet<String> {
    render::builders::create_interpreter().supported_widgets()
}
