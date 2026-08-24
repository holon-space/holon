//! `inv-no-declared-column-absent` wiring — needs the `DeclaredColumnGaps` read
//! cap (a process-global WARN-event capture). Only a slice that registers the
//! cap (the `wide_e2e` slice) selects it; others deselect it (disclosed, not
//! faked). See [`crate::pbt::composed::declared_column_gaps`].

use holon_pbt_core::RunMode;
use holon_pbt_core::composition::Attribution;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;

use crate::pbt::composed::declared_column_gaps::DeclaredColumnGaps;
use crate::pbt::composed::declared_column_gaps::InvNoDeclaredColumnAbsent;

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvNoDeclaredColumnAbsent::new(),
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn DeclaredColumnGaps>()],
            sut_absent: Vec::new(),
            ref_present: Vec::new(),
        },
        Attribution::cross_cutting(file!()),
    ))
}
