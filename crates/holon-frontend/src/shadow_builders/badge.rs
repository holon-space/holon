use super::prelude::*;

// `block_id` names the entity a literal-label badge marks. A badge built from
// `badge(col("status"))` binds its row through the label; one whose label is a
// literal — a disclosure the profile writes, not the data — has no binding at
// all, and a mark nobody can attribute to a block is not a mark.
holon_macros::widget_builder! {
    fn badge(label: String, block_id: Option<String>);
}
