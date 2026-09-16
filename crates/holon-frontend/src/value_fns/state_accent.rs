//! `state_accent(state_string)` — map a task state to an accent THEME TOKEN.
//!
//! Returns a token name (`holon_api::theme_token::THEME_TOKENS`) suitable for
//! `card(accent: ...)`. Lets kanban boards / collection profiles drive accent
//! colour from the `task_state` column without storing a per-row colour.
//!
//! It returns a token rather than a hex so the accent follows the active theme:
//! the earlier hardcoded palette was a dark-theme palette painted unchanged in
//! a light theme, and it named colours no theme could reach.
//!
//! Mapping (case-insensitive, leading/trailing whitespace ignored):
//!
//! | Input               | Token     | Intent          |
//! | ------------------- | --------- | --------------- |
//! | `DONE`              | `success` | sage / completed|
//! | `DOING`, `IN PROGRESS`, `IN-PROGRESS`, `NEXT`, `STARTED` | `warning` | amber / active|
//! | `BLOCKED`, `WAIT`, `WAITING`, `HOLD`                     | `error`   | coral / blocked|
//! | empty / `TODO` / `OPEN` / unknown                        | `muted`   | neutral / pending|
//!
//! Caller pattern:
//!
//! ```rhai
//! card(accent: state_accent(col("task_state")), text(col("title")))
//! ```

use holon_api::InterpValue;
use holon_api::Value;
use holon_api::render_eval::ResolvedArgs;

use crate::ReactiveViewModel;
use crate::reactive::BuilderServices;
use crate::render_context::RenderContext;
use crate::render_interpreter::RenderInterpreter;
use crate::render_interpreter::ValueFn;

struct StateAccentValueFn;

const ACCENT_DONE: &str = "success";
const ACCENT_ACTIVE: &str = "warning";
const ACCENT_BLOCKED: &str = "error";
const ACCENT_NEUTRAL: &str = "muted";

pub fn accent_for_state(state: &str) -> &'static str {
    let key = state.trim().to_ascii_uppercase();
    match key.as_str() {
        "DONE" | "COMPLETED" | "FINISHED" | "CLOSED" => ACCENT_DONE,
        // `NOW` is LogSeq's in-progress keyword (DOING-family); `LATER` is
        // its not-started keyword (TODO-family, neutral) — ForeignVaultCompat §4.
        "DOING" | "NOW" | "IN PROGRESS" | "IN-PROGRESS" | "INPROGRESS" | "NEXT" | "STARTED" => {
            ACCENT_ACTIVE
        }
        "BLOCKED" | "BLOCK" | "WAIT" | "WAITING" | "HOLD" | "ON HOLD" => ACCENT_BLOCKED,
        "LATER" | "TODO" | "OPEN" | "" => ACCENT_NEUTRAL,
        _ => ACCENT_NEUTRAL,
    }
}

impl ValueFn for StateAccentValueFn {
    fn invoke(
        &self,
        args: &ResolvedArgs,
        _: &dyn BuilderServices,
        _: &RenderContext,
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
