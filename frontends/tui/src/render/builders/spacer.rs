use super::prelude::*;

pub fn build(_ba: BA<'_>) -> TuiWidget {
    TuiWidget::Text {
        content: " ".to_string(),
        bold: false,
    }
}
