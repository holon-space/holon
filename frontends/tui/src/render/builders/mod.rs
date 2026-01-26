mod prelude;

holon_macros::builder_registry!("src/render/builders",
    skip: [prelude],
    register: TuiWidget
);

use holon_frontend::render_interpreter::{BuilderArgs, RenderInterpreter};

/// A TUI widget tree node — the `W` type for the shadow builder interpreter.
///
/// Unlike GPUI where `Div` is the universal element, r3bl's rendering is
/// imperative (write render ops to a buffer). We use this enum as an
/// intermediate representation that gets flattened to render ops during
/// the final paint pass.
#[derive(Debug, Clone)]
pub enum TuiWidget {
    Text {
        content: String,
        bold: bool,
    },
    Row {
        children: Vec<TuiWidget>,
    },
    Column {
        children: Vec<TuiWidget>,
    },
    Checkbox {
        checked: bool,
    },
    Badge {
        content: String,
    },
    Icon {
        symbol: String,
    },
    /// Placeholder for unsupported/empty widgets
    Empty,
}

impl holon_frontend::render_interpreter::WithEntity for TuiWidget {
    fn attach_entity(
        &mut self,
        _entity: std::sync::Arc<std::collections::HashMap<String, holon_api::Value>>,
    ) {
        // TUI doesn't use entity data for navigation
    }
}

impl TuiWidget {
    /// Flatten this widget tree into a single line of text for simple rendering.
    pub fn to_line(&self) -> String {
        match self {
            TuiWidget::Text { content, .. } => content.clone(),
            TuiWidget::Row { children } => children
                .iter()
                .map(|c| c.to_line())
                .collect::<Vec<_>>()
                .join(""),
            TuiWidget::Column { children } => children
                .iter()
                .map(|c| c.to_line())
                .collect::<Vec<_>>()
                .join("\n"),
            TuiWidget::Checkbox { checked } => {
                if *checked {
                    "[✓] ".to_string()
                } else {
                    "[ ] ".to_string()
                }
            }
            TuiWidget::Badge { content } => format!(" [{}] ", content),
            TuiWidget::Icon { symbol } => format!("{} ", symbol),
            TuiWidget::Empty => String::new(),
        }
    }

    /// Check if this is a checkbox
    pub fn is_checkbox(&self) -> bool {
        matches!(self, TuiWidget::Checkbox { .. })
    }
}

pub(crate) type BA<'a> = BuilderArgs<'a, TuiWidget>;

pub fn create_interpreter() -> RenderInterpreter<TuiWidget> {
    let mut interp = RenderInterpreter::new();

    register_all(&mut interp);

    interp.register("col", |ba: BA<'_>| {
        let children: Vec<TuiWidget> = holon_frontend::render_interpreter::shared_col_build(&ba);
        TuiWidget::Column { children }
    });

    interp
}
