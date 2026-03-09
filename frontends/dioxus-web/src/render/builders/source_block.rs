use super::prelude::*;

pub fn render(
    language: &String,
    content: &String,
    _name: &String,
    _editable: &bool,
    _ctx: &DioxusRenderContext,
) -> Element {
    let lang = language.clone();
    let content = content.clone();
    rsx! {
        pre {
            style: "background: #1a1a2e; padding: 8px; border-radius: 4px; overflow-x: auto; font-size: 0.85em; color: #a0c4ff;",
            code { "#+begin_src {lang}\n{content}\n#+end_src" }
        }
    }
}
