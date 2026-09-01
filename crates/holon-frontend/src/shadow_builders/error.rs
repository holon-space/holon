use super::prelude::*;

// `degraded_disclosure` marks an error the system already understands and has
// disclosed — a view over an integration that is known not to be running,
// rather than this block breaking. It follows `annotate_degraded`'s prop
// convention so a renderer can style it calmly; `message` carries the same
// sentence for renderers that do not.
holon_macros::widget_builder! {
    fn error(message: String, degraded_disclosure: Option<String>, integration: Option<String>);
}
