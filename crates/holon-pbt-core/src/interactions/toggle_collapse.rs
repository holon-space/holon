//! Variant: toggle an outline/tree row's expanded state.

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ToggleCollapse {
    /// The `target_id` of the `expand_toggle` widget — typically the
    /// row's `entity_uri` as rendered (`EntityUri::to_string()`). The
    /// frontend tags its chevron with `expand_toggle_id_for(target_id)`
    /// in the bounds registry so the same string locates it for click
    /// dispatch.
    pub target_id: String,
}
