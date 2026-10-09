//! Work a background task does on its own timer, not because something asked.
//!
//! A measurement that charges a window for the work done inside it (the
//! keystone's per-transition SQL budgets) must not charge it for a timer that
//! happened to fire there. Each pass of such a task runs under a span marked
//! with [`mark_periodic`]; the marker travels in the OpenTelemetry context, so
//! every span under that pass can be told apart from the window's own work.

/// The OpenTelemetry context value every span under a [`mark_periodic`] span
/// inherits: the name of the periodic task whose pass opened it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeriodicTask(pub &'static str);

/// The span attribute a span collector stamps on each span that inherits a
/// [`PeriodicTask`], carrying the task's name.
pub const PERIODIC_TASK_ATTR: &str = "periodic_task";

/// Mark `span` as one pass of the periodic task `task`.
///
/// Call it on a span opened `parent: None` and not yet entered: the marker
/// replaces the span's OpenTelemetry parent, and a started span's parent is
/// fixed. Without an OpenTelemetry layer, or with the span filtered out, there
/// is nothing to mark.
pub fn mark_periodic(span: &tracing::Span, task: &'static str) {
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_opentelemetry::SetParentError;

    match span.set_parent(opentelemetry::Context::new().with_value(PeriodicTask(task))) {
        Ok(()) | Err(SetParentError::LayerNotFound | SetParentError::SpanDisabled) => {}
        Err(SetParentError::AlreadyStarted) => {
            panic!("periodic task '{task}': its pass span was entered before it was marked")
        }
    }
}
