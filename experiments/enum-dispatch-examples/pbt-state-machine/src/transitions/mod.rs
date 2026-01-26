//! One file per transition. Each file owns its struct, its strategy,
//! its applicability rule, its preconditions, and both its ref and SUT
//! apply paths. Adding a transition = create a sibling file + add one
//! line to the `declare_transitions!` invocation below.

mod add_item;
mod remove_item;
mod rename_item;

pub use add_item::AddItem;
pub use remove_item::RemoveItem;
pub use rename_item::RenameItem;

crate::declare_transitions! {
    pub enum E2ETransitionEnum {
        AddItem(AddItem),
        RemoveItem(RemoveItem),
        RenameItem(RenameItem),
    }
}
