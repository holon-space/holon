//! @c4 component
//! @c4 layer Core
//! @c4 uses holon-api "live data, entity and row-source vocabulary" "Rust"
//!
//! The degradation vocabulary: the condition bus, each condition's profile and
//! detail, and the bus as a row source. No storage backend, no frontend deps.

/// flutter_rust_bridge:ignore
pub mod condition_bus;
pub mod condition_detail;
pub mod condition_profile;
pub mod condition_source;

pub use condition_bus::Condition;
pub use condition_bus::ConditionBus;
pub use condition_bus::ConditionChange;
pub use condition_bus::ConditionKey;
pub use condition_bus::ConditionKind;
pub use condition_bus::ConditionSubscription;
pub use condition_bus::DatabaseHold;
pub use condition_bus::DerivedFieldSeat;
pub use condition_bus::DroppedPanics;
pub use condition_bus::EarlierPanic;
pub use condition_bus::EarlierPanics;
pub use condition_bus::HolonChange;
pub use condition_bus::IngestRefusals;
pub use condition_bus::QuarantinedTable;
pub use condition_bus::RefusedFile;
pub use condition_profile::AllClear;
pub use condition_profile::ClearingEvent;
pub use condition_profile::ConditionPlacement;
pub use condition_profile::ConditionProfile;
pub use condition_profile::ConditionSeverity;
pub use condition_profile::OpId;
pub use condition_profile::RemedySlot;
pub use condition_profile::SettingsSection;
