use super::prelude::*;
use holon_frontend::render_interpreter::shared_col_build;

pub fn build(ba: BA<'_>) -> Element {
    let children: Vec<Element> = shared_col_build(&ba);
    rsx! {
        div { display: "flex", flex_direction: "column",
            {children.into_iter()}
        }
    }
}
