//! The keystone's static examples: fixed `(wiring, transitions)` cases the
//! keystone runs through the same properties before its random draws.
//!
//! Unlike a hand-authored replay, a static example passes through the
//! generator's filter: a transition the reference machine's preconditions
//! reject is dropped, exactly as the random draw never offers it. So a case can
//! pin a sequence the model once admitted wrongly, and stays a meaningful case
//! once the model refuses it.

use holon_pbt_core::capabilities::RefReadOnlyHomes;
use holon_pbt_core::wiring::Wiring;
use proptest_state_machine::ReferenceStateMachine;

use crate::pbt::composed::wide_e2e::WideE2EMachine;
use crate::pbt::composed::wide_e2e::wide_e2e_ref_for;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transitions::E2ETransition;

pub struct StaticExample {
    pub name: &'static str,
    pub initial_state: ReferenceState,
    /// What the generator admits of the example, applied in order.
    pub transitions: Vec<E2ETransition>,
}

const RECIPE_PAGE: &str = r#"{"NavigateFocus": {"region": "main", "block_id": "block:7abb2e4d-fbd6-ec0a-7f7c-00d2ff055d8b"}}"#;
const BREAK_RECIPE: &str = r#"{"BreakIngestedRecipe": {"file": "Grüne Keystone-Soße.cook"}}"#;
const DELETE_RECIPE: &str = r#"{"DeleteRefusedRecipe": {"file": "Grüne Keystone-Soße.cook"}}"#;

/// Every static example, in run order.
pub fn keystone_static_examples() -> Vec<StaticExample> {
    let sql_only: Wiring = serde_json::from_str(
        r#"{"storage_adapters": ["Turso"], "sync_adapters": [], "actors": []}"#,
    )
    .expect("a valid wiring manifest");
    let mut examples = vec![
        // The write tier refuses a split of a read-only recipe step, so the
        // deleted file's document must hold only what the file declared.
        example(
            "a-split-read-only-step-leaves-the-deleted-recipe-nothing-undeclared",
            &sql_only,
            &[
                RECIPE_PAGE,
                r#"{"SplitBlock": {"block_id": "block:Gr%C3%BCne%20Keystone-So%C3%9Fe.cook::b::0", "position": 23}}"#,
                BREAK_RECIPE,
                DELETE_RECIPE,
            ],
        ),
        // The same for a create under the read-only recipe page.
        example(
            "a-create-under-the-read-only-page-leaves-the-deleted-recipe-nothing-undeclared",
            &sql_only,
            &[
                RECIPE_PAGE,
                r#"{"CreateBlockUnderFocus": {"content": "probe", "id": "block:gen-under-recipe"}}"#,
                BREAK_RECIPE,
                DELETE_RECIPE,
            ],
        ),
    ];
    examples.extend(enter_on_every_read_only_step());
    examples
}

/// Enter in an editor on each block homed in a read-only file, then the
/// recipe file is retired: the deleted document must hold only what it
/// declared.
fn enter_on_every_read_only_step() -> Vec<StaticExample> {
    let ui: Wiring = serde_json::from_str(
        r#"{"storage_adapters": ["Turso"], "sync_adapters": [], "actors": ["UI"]}"#,
    )
    .expect("a valid wiring manifest");
    wide_e2e_ref_for(&ui)
        .read_only_homed_blocks()
        .into_iter()
        .map(|block| {
            let block = serde_json::to_string(&block).expect("an entity uri serializes");
            let focus = format!(r#"{{"FocusEditableText": {{"block_id": {block}}}}}"#);
            example(
                "enter-on-a-read-only-step-leaves-the-deleted-recipe-nothing-undeclared",
                &ui,
                &[
                    RECIPE_PAGE,
                    &focus,
                    r#"{"PressKey": {"chord": ["enter"]}}"#,
                    BREAK_RECIPE,
                    DELETE_RECIPE,
                ],
            )
        })
        .collect()
}

fn example(name: &'static str, wiring: &Wiring, transitions: &[&str]) -> StaticExample {
    let initial_state = wide_e2e_ref_for(wiring);
    let mut state = initial_state.clone();
    let mut admitted = Vec::new();
    for json in transitions {
        let transition: E2ETransition = serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("static example {name}: unparsable transition {json}: {e}"));
        if WideE2EMachine::preconditions(&state, &transition) {
            state = WideE2EMachine::apply(state, &transition);
            admitted.push(transition);
        } else {
            eprintln!("[static example] {name}: the generator rejects {json}; dropped");
        }
    }
    StaticExample {
        name,
        initial_state,
        transitions: admitted,
    }
}
