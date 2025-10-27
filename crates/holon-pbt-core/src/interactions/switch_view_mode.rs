//! Variant: switch a mode-switchable block to the named mode.

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct SwitchViewMode {
    /// The VMS's `entity_uri` as rendered (`EntityUri::to_string()`).
    pub block_id: String,
    /// The target mode name (e.g. `"table_view"`, `"tree_view"`).
    pub target_mode: String,
}
