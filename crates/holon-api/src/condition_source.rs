//! The conditions in effect, as a named row source.
//!
//! The first registration on the [`row_source`](crate::row_source) seam, and
//! the reason the seam exists: "what is currently degraded" is state no query
//! produces, so before this every surface that wanted to show it kept its own
//! mirror — which is how a provider's status glyph and its banner came to
//! disagree.
//!
//! The columns are the kind-level facts plus the instance's identity. There is
//! deliberately no prose column: the per-kind message text still lives in the
//! GPUI toast layer, and minting a second copy of it here would recreate the
//! divergence this source removes. Inc 3 moves that text down beside the
//! remedies, and it becomes a column then.

use std::sync::Arc;

use crate::Value;
use crate::condition_bus::Condition;
use crate::condition_bus::ConditionBus;
use crate::condition_profile::ConditionPlacement;
use crate::condition_profile::ConditionSeverity;
use crate::live_data_source::ID_COLUMN;
use crate::live_data_source::LiveDataSource;
use crate::row_source::NamedSourceDef;
use crate::widget_spec::DataRow;

/// What a row of the `conditions` source carries.
///
/// `subject` is filterable because that is the join D120.a asks for: a
/// provider row showing its own conditions beside it. The mirror keys on
/// subject first, so a filtered view reads one contiguous run of the holder.
pub const CONDITIONS: NamedSourceDef = NamedSourceDef {
    name: "conditions",
    columns: &[
        ID_COLUMN,
        "subject",
        "kind",
        "severity",
        "label",
        "placement",
    ],
};

/// The `conditions` source over `bus`.
pub fn conditions_source(bus: &ConditionBus) -> Arc<LiveDataSource<Condition>> {
    LiveDataSource::new(
        CONDITIONS,
        bus.conditions(),
        Arc::new(|key: &str, condition: &Condition| condition_row(key, condition)),
    )
}

fn condition_row(key: &str, condition: &Condition) -> DataRow {
    let profile = condition.reason.profile();
    DataRow::from([
        (
            ID_COLUMN.to_string(),
            Value::String(format!("condition:{key}")),
        ),
        (
            "subject".to_string(),
            Value::String(condition.subject.clone()),
        ),
        (
            "kind".to_string(),
            Value::String(condition.reason.condition_kind().to_string()),
        ),
        (
            "severity".to_string(),
            Value::String(severity_name(profile.severity()).to_string()),
        ),
        (
            "label".to_string(),
            Value::String(profile.label().to_string()),
        ),
        (
            "placement".to_string(),
            Value::String(placement_name(profile.placement()).to_string()),
        ),
    ])
}

/// The severity as a template reads it. Lower-case and stable: a render spec
/// matches on this string, so it is part of the contract, not a `Debug` render.
pub const fn severity_name(severity: ConditionSeverity) -> &'static str {
    match severity {
        ConditionSeverity::Info => "info",
        ConditionSeverity::Warning => "warning",
        ConditionSeverity::Error => "error",
    }
}

/// The declared placement as a template reads it. Same contract as
/// [`severity_name`].
pub const fn placement_name(placement: ConditionPlacement) -> &'static str {
    match placement {
        ConditionPlacement::Toast => "toast",
        ConditionPlacement::Banner => "banner",
        ConditionPlacement::Modal => "modal",
        ConditionPlacement::Section(_) => "section",
    }
}
