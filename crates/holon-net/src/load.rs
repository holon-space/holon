//! The loader at Holon's token value: a net file becomes a [`NetDef`] over
//! [`holon_pattern::Value`].

use std::path::Path;

use holon_pattern::Value;
pub use petri_workflow::load::NetLoadError;

use crate::exec::NetDef;
use crate::host::new_net;

pub type Logic = petri_workflow::load::Logic<Value>;

/// Builds the net the file at `path` declares. `logic` is asked for every
/// transition the file holds, by name.
pub fn load(path: &Path, logic: impl FnMut(&str) -> Logic) -> Result<NetDef, NetLoadError> {
    petri_workflow::load::load(new_net(), path, logic)
}

/// [`load`] on text already in hand. `path` is what the refusals name.
pub fn net_def(
    path: &str,
    text: &str,
    logic: impl FnMut(&str) -> Logic,
) -> Result<NetDef, NetLoadError> {
    petri_workflow::load::net_def(new_net(), path, text, logic)
}
