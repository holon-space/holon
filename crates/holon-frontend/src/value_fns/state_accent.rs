//! `state_accent(state_string)` — map a task state to a default accent color.
//!
//! Returns a hex string (`#RRGGBB`) suitable for `card(accent: ...)`. Lets
//! kanban boards / collection profiles drive accent color from the
//! `task_state` column without storing a per-row color.
//!
//! Mapping (case-insensitive, leading/trailing whitespace ignored):
//!
//! | Input               | Hex      | Intent          |
//! | ------------------- | -------- | --------------- |
//! | `DONE`              | `#7D9D7D`| sage / completed|
//! | `DOING`, `IN PROGRESS`, `IN-PROGRESS`, `NEXT`, `STARTED` | `#D4A373`| amber / active|
//! | `BLOCKED`, `WAIT`, `WAITING`, `HOLD`                     | `#C97064`| coral / blocked|
//! | empty / `TODO` / `OPEN` / unknown                        | `#5A5A55`| neutral / pending|
//!
//! Caller pattern:
//!
//! ```rhai
//! card(accent: state_accent(col("task_state")), text(col("title")))
//! ```

use holon_api::render_eval::ResolvedArgs;
use holon_api::{InterpValue, Value};

use crate::reactive::BuilderServices;
use crate::render_context::RenderContext;
use crate::render_interpreter::{RenderInterpreter, ValueFn};
use crate::ReactiveViewModel;

struct StateAccentValueFn;

const ACCENT_DONE: &str = "#7D9D7D";
const ACCENT_ACTIVE: &str = "#D4A373";
const ACCENT_BLOCKED: &str = "#C97064";
const ACCENT_NEUTRAL: &str = "#5A5A55";

pub fn accent_for_state(state: &str) -> &'static str {
    let key = state.trim().to_ascii_uppercase();
    match key.as_str() {
        "DONE" | "COMPLETED" | "FINISHED" | "CLOSED" => ACCENT_DONE,
        "DOING" | "IN PROGRESS" | "IN-PROGRESS" | "INPROGRESS" | "NEXT" | "STARTED" => {
            ACCENT_ACTIVE
        }
        "BLOCKED" | "BLOCK" | "WAIT" | "WAITING" | "HOLD" | "ON HOLD" => ACCENT_BLOCKED,
        _ => ACCENT_NEUTRAL,
    }
}

impl ValueFn for StateAccentValueFn {
    fn invoke(
        &self,
        args: &ResolvedArgs,
        _services: &dyn BuilderServices,
        _ctx: &RenderContext,
    ) -> InterpValue {
        let state = args
            .positional
            .first()
            .and_then(|v| v.as_string())
            .unwrap_or("");
        InterpValue::Value(Value::String(accent_for_state(state).to_string()))
    }
}

/// Register `state_accent` on the given interpreter. Collision-checked by
/// `register_value_fn`.
pub fn register_state_accent(interp: &mut RenderInterpreter<ReactiveViewModel>) {
    interp.register_value_fn("state_accent", StateAccentValueFn);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_states_map_to_palette() {
        assert_eq!(accent_for_state("DONE"), ACCENT_DONE);
        assert_eq!(accent_for_state("done"), ACCENT_DONE);
        assert_eq!(accent_for_state(" Done "), ACCENT_DONE);
        assert_eq!(accent_for_state("In Progress"), ACCENT_ACTIVE);
        assert_eq!(accent_for_state("DOING"), ACCENT_ACTIVE);
        assert_eq!(accent_for_state("BLOCKED"), ACCENT_BLOCKED);
        assert_eq!(accent_for_state("waiting"), ACCENT_BLOCKED);
    }

    #[test]
    fn unknown_state_is_neutral() {
        assert_eq!(accent_for_state(""), ACCENT_NEUTRAL);
        assert_eq!(accent_for_state("TODO"), ACCENT_NEUTRAL);
        assert_eq!(accent_for_state("???"), ACCENT_NEUTRAL);
    }
}
