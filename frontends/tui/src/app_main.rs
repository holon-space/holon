use crate::render::builders::{create_interpreter, TuiWidget};
use crate::state::AppState;
use holon_frontend::{FrontendSession, RenderContext};
use r3bl_tui::{
    col, height, new_style, render_tui_styled_texts_into, row, surface, throws_with_return,
    tui_color, tui_styled_text, tui_styled_texts, App, BoxedSafeApp, CommonResult,
    ComponentRegistryMap, EventPropagation, FlexBoxId, GlobalData, HasFocus, InputEvent, Key,
    KeyPress, LayoutManagement, LengthOps, Pos, RenderOpCommon, RenderOpIRVec, RenderPipeline,
    Size, SurfaceProps, ZOrder, SPACER_GLYPH,
};
use std::marker::PhantomData;
use std::sync::Arc;

use crate::stylesheet;

#[derive(Clone, Debug)]
pub enum AppSignal {
    Noop,
}

impl Default for AppSignal {
    fn default() -> Self {
        AppSignal::Noop
    }
}

/// Application state for r3bl framework
#[derive(Clone)]
pub struct TuiState {
    pub session: Arc<FrontendSession>,
    pub app_state: AppState,
    pub rt_handle: tokio::runtime::Handle,
    pub status_message: String,
}

impl std::fmt::Debug for TuiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TuiState")
            .field("status_message", &self.status_message)
            .finish()
    }
}

impl Default for TuiState {
    fn default() -> Self {
        panic!("TuiState::default() should not be called — use TuiState::new()")
    }
}

impl std::fmt::Display for TuiState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TuiState")
    }
}

impl r3bl_tui::HasEditorBuffers for TuiState {
    fn get_mut_editor_buffer(&mut self, _id: FlexBoxId) -> Option<&mut r3bl_tui::EditorBuffer> {
        None
    }
    fn insert_editor_buffer(&mut self, _id: FlexBoxId, _buffer: r3bl_tui::EditorBuffer) {}
    fn contains_editor_buffer(&self, _id: FlexBoxId) -> bool {
        false
    }
}

impl r3bl_tui::HasDialogBuffers for TuiState {
    fn get_mut_dialog_buffer(&mut self, _id: FlexBoxId) -> Option<&mut r3bl_tui::DialogBuffer> {
        None
    }
}

pub struct AppMain {
    _phantom: PhantomData<(TuiState, AppSignal)>,
}

impl Default for AppMain {
    fn default() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl AppMain {
    pub fn new_boxed() -> BoxedSafeApp<TuiState, AppSignal> {
        Box::new(Self::default())
    }
}

impl App for AppMain {
    type S = TuiState;
    type AS = AppSignal;

    fn app_init(
        &mut self,
        _component_registry_map: &mut ComponentRegistryMap<TuiState, AppSignal>,
        _has_focus: &mut HasFocus,
    ) {
    }

    fn app_handle_input_event(
        &mut self,
        input_event: InputEvent,
        global_data: &mut GlobalData<TuiState, AppSignal>,
        _component_registry_map: &mut ComponentRegistryMap<TuiState, AppSignal>,
        _has_focus: &mut HasFocus,
    ) -> CommonResult<EventPropagation> {
        throws_with_return!({
            // Handle Ctrl+Q exit
            if let InputEvent::Keyboard(KeyPress::WithModifiers {
                key: Key::Character('q'),
                mask,
            }) = input_event
            {
                if mask.ctrl_key_state == r3bl_tui::KeyState::Pressed {
                    return Ok(EventPropagation::Propagate);
                }
            }

            // Handle Ctrl+R sync
            if let InputEvent::Keyboard(KeyPress::WithModifiers {
                key: Key::Character('r'),
                mask,
            }) = input_event
            {
                if mask.ctrl_key_state == r3bl_tui::KeyState::Pressed {
                    let engine = global_data.state.session.engine().clone();
                    tracing::info!("[TUI] Sync triggered (Ctrl+r)");
                    tokio::spawn(async move {
                        let params = std::collections::HashMap::new();
                        match engine.execute_operation("*", "sync", params).await {
                            Ok(_) => tracing::info!("[TUI] Sync completed"),
                            Err(e) => tracing::error!("[TUI] Sync failed: {}", e),
                        }
                    });
                    global_data.state.status_message = "Syncing...".to_string();
                    return Ok(EventPropagation::ConsumedRender);
                }
            }

            EventPropagation::Propagate
        });
    }

    fn app_handle_signal(
        &mut self,
        _action: &AppSignal,
        _global_data: &mut GlobalData<TuiState, AppSignal>,
        _component_registry_map: &mut ComponentRegistryMap<TuiState, AppSignal>,
        _has_focus: &mut HasFocus,
    ) -> CommonResult<EventPropagation> {
        throws_with_return!({ EventPropagation::ConsumedRender });
    }

    fn app_render(
        &mut self,
        global_data: &mut GlobalData<TuiState, AppSignal>,
        _component_registry_map: &mut ComponentRegistryMap<TuiState, AppSignal>,
        _has_focus: &mut HasFocus,
    ) -> CommonResult<RenderPipeline> {
        throws_with_return!({
            let window_size = global_data.window_size;
            let state = &global_data.state;

            let widget_spec = state.app_state.widget_spec();
            let render_ctx =
                RenderContext::new(Arc::clone(&state.session), state.rt_handle.clone());
            let data_rows: Vec<_> = widget_spec.data.iter().map(|r| r.data.clone()).collect();
            let render_ctx = render_ctx.with_data_rows(data_rows);

            let interp = create_interpreter();
            let root_widget = interp.interpret(&widget_spec.render_expr, &render_ctx);

            let mut surface = {
                let mut it = surface!(stylesheet: stylesheet::create_stylesheet()?);

                it.surface_start(SurfaceProps {
                    pos: row(0) + col(0),
                    size: window_size.col_width + (window_size.row_height - height(2)),
                })?;

                // Title bar
                {
                    let mut title_ops = RenderOpIRVec::new();
                    title_ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(2), row(0))));
                    let title_texts = tui_styled_texts! {
                        tui_styled_text! {
                            @style: new_style!(bold color_fg: {tui_color!(hex "#00AAFF")}),
                            @text: "Holon (R3BL TUI)"
                        },
                    };
                    render_tui_styled_texts_into(&title_texts, &mut title_ops);
                    it.render_pipeline.push(ZOrder::Normal, title_ops);
                }

                // Content area — render widget tree
                {
                    let mut content_ops = RenderOpIRVec::new();
                    render_widget_tree(
                        &root_widget,
                        &mut content_ops,
                        2, // start_row (after title)
                        2, // start_col
                    );
                    it.render_pipeline.push(ZOrder::Normal, content_ops);
                }

                it.surface_end()?;
                it
            };

            // Status bar
            render_status_bar(
                &mut surface.render_pipeline,
                window_size,
                &state.status_message,
            );

            surface.render_pipeline
        });
    }
}

/// Recursively render a TuiWidget tree into r3bl render operations.
fn render_widget_tree(
    widget: &TuiWidget,
    ops: &mut RenderOpIRVec,
    start_row: usize,
    start_col: usize,
) -> usize {
    match widget {
        TuiWidget::Text { content, bold } => {
            let lines: Vec<&str> = content.split('\n').collect();
            for (i, line) in lines.iter().enumerate() {
                *ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((
                    col(start_col),
                    row(start_row + i),
                )));
                let fg = tui_color!(hex "#CCCCCC");
                let style = if *bold {
                    new_style!(bold color_fg: {fg})
                } else {
                    new_style!(color_fg: {fg})
                };
                let texts = tui_styled_texts! {
                    tui_styled_text! { @style: style, @text: line },
                };
                render_tui_styled_texts_into(&texts, ops);
            }
            lines.len().max(1)
        }
        TuiWidget::Checkbox { checked } => {
            *ops +=
                RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(start_col), row(start_row))));
            let text = if *checked { "[✓] " } else { "[ ] " };
            let fg = if *checked {
                tui_color!(hex "#00FF00")
            } else {
                tui_color!(hex "#888888")
            };
            let texts = tui_styled_texts! {
                tui_styled_text! { @style: new_style!(color_fg: {fg}), @text: text },
            };
            render_tui_styled_texts_into(&texts, ops);
            1
        }
        TuiWidget::Badge { content } => {
            let display = format!(" [{}] ", content);
            let texts = tui_styled_texts! {
                tui_styled_text! {
                    @style: new_style!(color_fg: {tui_color!(hex "#FFFF00")} bold),
                    @text: &display
                },
            };
            render_tui_styled_texts_into(&texts, ops);
            1
        }
        TuiWidget::Icon { symbol } => {
            let display = format!("{} ", symbol);
            let texts = tui_styled_texts! {
                tui_styled_text! {
                    @style: new_style!(color_fg: {tui_color!(hex "#CCCCCC")}),
                    @text: &display
                },
            };
            render_tui_styled_texts_into(&texts, ops);
            1
        }
        TuiWidget::Row { children } => {
            // Render children horizontally (no cursor movement between — they append inline)
            let mut max_rows = 1;
            for child in children {
                let rows = render_widget_tree(child, ops, start_row, start_col);
                max_rows = max_rows.max(rows);
            }
            max_rows
        }
        TuiWidget::Column { children } => {
            let mut current_row = start_row;
            for child in children {
                let rows = render_widget_tree(child, ops, current_row, start_col);
                current_row += rows;
            }
            current_row - start_row
        }
        TuiWidget::Empty => 0,
    }
}

fn render_status_bar(pipeline: &mut RenderPipeline, size: Size, status_msg: &str) {
    let color_bg = tui_color!(hex "#076DEB");
    let color_fg = tui_color!(hex "#E9C940");

    let help_text = format!("Ctrl+q: Exit | Ctrl+r: Sync | {}", status_msg);

    let styled_texts = tui_styled_texts! {
        tui_styled_text! {
            @style: new_style!(color_fg:{color_fg} color_bg:{color_bg}),
            @text: &help_text
        },
    };

    let row_idx = row(size.row_height.convert_to_index());

    let mut render_ops = RenderOpIRVec::new();
    render_ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(0), row_idx)));
    render_ops += RenderOpCommon::ResetColor;
    render_ops += RenderOpCommon::SetBgColor(color_bg);
    render_ops += r3bl_tui::RenderOpIR::PaintTextWithAttributes(
        SPACER_GLYPH.repeat(size.col_width.as_usize()).into(),
        None,
    );
    render_ops += RenderOpCommon::ResetColor;
    render_ops += RenderOpCommon::MoveCursorPositionAbs(Pos::from((col(2), row_idx)));
    render_tui_styled_texts_into(&styled_texts, &mut render_ops);
    pipeline.push(ZOrder::Normal, render_ops);
}
