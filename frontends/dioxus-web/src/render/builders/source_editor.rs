use holon_frontend::view_model::ViewKind;

use super::prelude::*;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::SourceEditor {
        language, content, ..
    } = &node.kind
    else {
        return rsx! {};
    };
    let lang = language.clone();
    let content = content.clone();
    rsx! {
        pre {
            style: "background: var(--surface, #1f1f1f); border: 1px solid var(--border, rgba(255,255,255,0.09)); padding: 10px 12px; border-radius: var(--radius, 6px); overflow-x: auto; font-size: 0.85em; line-height: 1.5; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; color: var(--accent, #4a9eda);",
            code { "#+begin_src {lang}\n{content}\n#+end_src" }
        }
    }
}
