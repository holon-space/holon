//! Shared UI-interaction variant structs.
//!
//! Each variant lives in its own file. The structs themselves carry
//! only data; per-PBT behaviour (the `TransitionFactory` /
//! `TransitionImpl` impls) lives in the consumer crate's own
//! per-variant file. See the crate-level docs for why.

pub mod deliver_block_content;
pub mod switch_view_mode;
pub mod toggle_collapse;
pub mod toggle_drawer;

pub use deliver_block_content::DeliverBlockContent;
pub use switch_view_mode::SwitchViewMode;
pub use toggle_collapse::ToggleCollapse;
pub use toggle_drawer::ToggleDrawer;
