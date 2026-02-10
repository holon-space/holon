use super::prelude::*;

/// drop_zone(position:N) — thin spacer placeholder for drag targets.
pub fn build(_args: &ResolvedArgs, _ctx: &RenderContext) -> Div {
    div().h(4.0)
}
