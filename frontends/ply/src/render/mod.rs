pub mod builders;
pub mod context;
pub mod interpreter;

/// Widget type for ply — a closure that builds elements on a `Ui` handle.
///
/// Ply is immediate-mode (no retained widget tree), so we represent widgets
/// as closures that build elements when called with a `Ui` reference.
pub type PlyWidget = Box<dyn Fn(&mut ply_engine::Ui<'_, ()>) + Send + Sync>;

/// Create an empty widget (renders nothing).
pub fn empty_widget() -> PlyWidget {
    Box::new(|_ui| {})
}
