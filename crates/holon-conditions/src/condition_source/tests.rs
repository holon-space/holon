use super::*;
use crate::condition_bus::Condition;
use crate::condition_bus::ConditionKind;

/// The row's `id` column reaches the render binding, which classifies it.
/// The bus's mirror key joins subject and kind with `\u{1F}` and so forms
/// no URI: reusing it as the id cost every conditions row its identity in
/// the bounds registry (`named_source_rows_windowed`, D142.a fallout).
#[test]
fn a_conditions_row_id_names_an_entity() {
    let row = condition_row(&Condition {
        subject: "todoist".to_string(),
        reason: ConditionKind::IntegrationConnectFailed {
            integration: "todoist".to_string(),
            error: "binary not found on PATH".to_string(),
        },
    });
    let id = row.get(ID_COLUMN).expect("the row declares an id column");
    assert_eq!(
        holon_api::widget_spec::row_id_of(&row)
            .entity()
            .map(|u| u.to_string()),
        Some("condition:todoist:integration-connect-failed".to_string()),
        "the conditions row id must classify as an entity, got {id:?}"
    );
}

/// A subject is an integration name from config, so the id constructor —
/// not the caller — owns URI-safety.
#[test]
fn a_subject_needing_encoding_still_names_an_entity() {
    let row = condition_row(&Condition {
        subject: "a name/with spaces".to_string(),
        reason: ConditionKind::IntegrationConnectFailed {
            integration: "a name/with spaces".to_string(),
            error: "boom".to_string(),
        },
    });
    assert!(
        holon_api::widget_spec::row_id_of(&row).entity().is_some(),
        "an awkward subject must still yield a classifiable id, got {:?}",
        row.get(ID_COLUMN)
    );
}
