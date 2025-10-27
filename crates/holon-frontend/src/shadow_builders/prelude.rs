pub(crate) use crate::render_interpreter::BuilderArgs;
pub(crate) use crate::view_model::{LazyChildren, NodeKind, ViewModel};
pub(crate) use holon_api::Value;

pub(crate) type BA<'a> = BuilderArgs<'a, ViewModel>;
