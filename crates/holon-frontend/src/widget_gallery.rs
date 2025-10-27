use std::collections::HashMap;

use holon_api::render_types::{Arg, RenderExpr};
use holon_api::Value;

// ── Helpers ──────────────────────────────────────────────────────────────

fn lit_str(s: &str) -> RenderExpr {
    RenderExpr::Literal {
        value: Value::String(s.into()),
    }
}

fn lit_bool(b: bool) -> RenderExpr {
    RenderExpr::Literal {
        value: Value::Boolean(b),
    }
}

fn lit_f64(f: f64) -> RenderExpr {
    RenderExpr::Literal {
        value: Value::Float(f),
    }
}

fn pos(value: RenderExpr) -> Arg {
    Arg { name: None, value }
}

fn named(name: &str, value: RenderExpr) -> Arg {
    Arg {
        name: Some(name.into()),
        value,
    }
}

fn call(name: &str, args: Vec<Arg>) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: name.into(),
        args,
    }
}

fn col_ref(name: &str) -> RenderExpr {
    RenderExpr::ColumnRef { name: name.into() }
}

fn section(title: &str, children: Vec<RenderExpr>) -> RenderExpr {
    let mut args = vec![named("title", lit_str(title))];
    for child in children {
        args.push(pos(child));
    }
    call("section", args)
}

fn column(children: Vec<RenderExpr>) -> RenderExpr {
    call("column", children.into_iter().map(pos).collect())
}

fn column_gap(children: Vec<RenderExpr>, gap: f64) -> RenderExpr {
    let mut args: Vec<Arg> = children.into_iter().map(pos).collect();
    args.push(named("gap", lit_f64(gap)));
    call("column", args)
}

fn row(children: Vec<RenderExpr>, gap: f64) -> RenderExpr {
    let mut args: Vec<Arg> = children.into_iter().map(pos).collect();
    args.push(named("gap", lit_f64(gap)));
    call("row", args)
}

// ── Gallery ──────────────────────────────────────────────────────────────

pub fn widget_gallery_render_expr() -> RenderExpr {
    column(vec![
        text_and_labels_section(),
        inputs_section(),
        layout_section(),
        data_display_section(),
        collections_section(),
        feedback_section(),
    ])
}

fn text_and_labels_section() -> RenderExpr {
    section(
        "Text & Labels",
        vec![
            call("text", vec![pos(lit_str("Plain text"))]),
            call(
                "text",
                vec![
                    named("content", lit_str("Bold text")),
                    named("bold", lit_bool(true)),
                ],
            ),
            call(
                "text",
                vec![
                    named("content", lit_str("Large text")),
                    named("size", lit_f64(20.0)),
                ],
            ),
            call(
                "text",
                vec![
                    named("content", lit_str("Colored text")),
                    named("color", lit_str("blue")),
                ],
            ),
            call(
                "text",
                vec![
                    named("content", lit_str("Muted text")),
                    named("color", lit_str("muted")),
                ],
            ),
            row(
                vec![
                    call("badge", vec![pos(lit_str("Active"))]),
                    call("badge", vec![pos(lit_str("Archived"))]),
                    call("badge", vec![pos(lit_str("Draft"))]),
                ],
                8.0,
            ),
            row(
                vec![
                    call("icon", vec![pos(lit_str("check"))]),
                    call("icon", vec![pos(lit_str("star"))]),
                    call("icon", vec![pos(lit_str("settings"))]),
                    call("icon", vec![pos(lit_str("search"))]),
                    call("icon", vec![pos(lit_str("tag"))]),
                ],
                8.0,
            ),
        ],
    )
}

fn inputs_section() -> RenderExpr {
    section(
        "Inputs",
        vec![
            row(
                vec![
                    call(
                        "checkbox",
                        vec![
                            named("checked", lit_bool(false)),
                            named("label", lit_str("Unchecked")),
                        ],
                    ),
                    call(
                        "checkbox",
                        vec![
                            named("checked", lit_bool(true)),
                            named("label", lit_str("Checked")),
                        ],
                    ),
                ],
                16.0,
            ),
            call(
                "editable_text",
                vec![named("content", lit_str("Edit me..."))],
            ),
            row(
                vec![
                    call("state_toggle", vec![pos(lit_str("task_state"))]),
                    call("state_toggle", vec![pos(lit_str("task_state_doing"))]),
                    call("state_toggle", vec![pos(lit_str("task_state_done"))]),
                    call("state_toggle", vec![pos(lit_str("task_state_empty"))]),
                ],
                12.0,
            ),
        ],
    )
}

fn layout_section() -> RenderExpr {
    section(
        "Layout",
        vec![
            row(
                vec![
                    call("text", vec![pos(lit_str("Left"))]),
                    call("spacer", vec![named("width", lit_f64(40.0))]),
                    call("text", vec![pos(lit_str("Right"))]),
                ],
                8.0,
            ),
            call("spacer", vec![named("height", lit_f64(8.0))]),
            section(
                "Nested Section",
                vec![call("text", vec![pos(lit_str("Sections can nest"))])],
            ),
            // columns with item_template iterates over data rows
            call(
                "columns",
                vec![named(
                    "item_template",
                    call(
                        "column",
                        vec![
                            pos(call(
                                "text",
                                vec![pos(col_ref("name")), named("bold", lit_bool(true))],
                            )),
                            pos(call(
                                "text",
                                vec![
                                    pos(col_ref("description")),
                                    named("color", lit_str("muted")),
                                ],
                            )),
                        ],
                    ),
                )],
            ),
        ],
    )
}

fn data_display_section() -> RenderExpr {
    section(
        "Data Display",
        vec![
            call(
                "source_block",
                vec![
                    named("language", lit_str("rust")),
                    named(
                        "content",
                        lit_str("fn main() {\n    println!(\"Hello, world!\");\n}"),
                    ),
                    named("name", lit_str("example.rs")),
                ],
            ),
            // A table with item_template rendering columns from data rows
            call(
                "table",
                vec![named(
                    "item_template",
                    call(
                        "row",
                        vec![
                            pos(call("text", vec![pos(col_ref("name"))])),
                            pos(call("badge", vec![pos(col_ref("status"))])),
                            pos(call(
                                "text",
                                vec![
                                    pos(col_ref("description")),
                                    named("color", lit_str("muted")),
                                ],
                            )),
                        ],
                    ),
                )],
            ),
        ],
    )
}

fn collections_section() -> RenderExpr {
    section(
        "Collections",
        vec![
            // list with gap and item_template
            call(
                "list",
                vec![
                    named("gap", lit_f64(4.0)),
                    named(
                        "item_template",
                        row(
                            vec![
                                call("icon", vec![pos(lit_str("star"))]),
                                call("text", vec![pos(col_ref("name"))]),
                                call("badge", vec![pos(col_ref("status"))]),
                            ],
                            8.0,
                        ),
                    ),
                ],
            ),
            // tree with parent_id hierarchy
            call(
                "tree",
                vec![named(
                    "item_template",
                    row(
                        vec![
                            call("icon", vec![pos(lit_str("tag"))]),
                            call("text", vec![pos(col_ref("name"))]),
                        ],
                        4.0,
                    ),
                )],
            ),
            // outline (same data as tree, different visual)
            call(
                "outline",
                vec![named(
                    "item_template",
                    call("text", vec![pos(col_ref("name"))]),
                )],
            ),
        ],
    )
}

fn feedback_section() -> RenderExpr {
    section(
        "Feedback",
        vec![
            call("error", vec![pos(lit_str("Something went wrong!"))]),
            // pref_field with choice type
            call(
                "pref_field",
                vec![
                    named("key", lit_str("gallery.demo_choice")),
                    named("pref_type", lit_str("choice")),
                    named("requires_restart", lit_bool(false)),
                ],
            ),
            // pref_field with toggle type
            call(
                "pref_field",
                vec![
                    named("key", lit_str("gallery.demo_toggle")),
                    named("pref_type", lit_str("toggle")),
                    named("requires_restart", lit_bool(true)),
                ],
            ),
        ],
    )
}

// ── Design System Gallery ────────────────────────────────────────────────
// Sections that showcase VISION_UI.md's design tokens: colors, typography,
// spacing, status indicators. These are the primary iteration target for
// visual design work.

/// Design-focused gallery — the entry point for design iteration.
///
/// Includes the widget gallery plus design-system-specific sections
/// (color palette, typography scale, spacing, status indicators).
pub fn design_gallery_render_expr() -> RenderExpr {
    column(vec![
        color_palette_section(),
        typography_section(),
        spacing_section(),
        status_indicators_section(),
        card_layout_section(),
        // Include the existing widget sections for completeness
        text_and_labels_section(),
        inputs_section(),
        layout_section(),
        data_display_section(),
        collections_section(),
        feedback_section(),
    ])
}

fn color_swatch(label: &str, hex: &str) -> RenderExpr {
    row(
        vec![
            call(
                "badge",
                vec![pos(lit_str(hex)), named("color", lit_str(hex))],
            ),
            call(
                "text",
                vec![
                    pos(lit_str(label)),
                    named("size", lit_f64(13.0)),
                    named("color", lit_str("muted")),
                ],
            ),
        ],
        8.0,
    )
}

fn color_palette_section() -> RenderExpr {
    section(
        "Color Palette — Light Theme",
        vec![
            call(
                "text",
                vec![
                    pos(lit_str(
                        "Warm, professional, alive. Not clinical, not childish.",
                    )),
                    named("color", lit_str("muted")),
                    named("size", lit_f64(13.0)),
                ],
            ),
            // Primary surfaces
            row(
                vec![
                    color_swatch("Background", "#FAFAF8"),
                    color_swatch("Surface", "#F5F4F0"),
                    color_swatch("Text Primary", "#2D2D2A"),
                    color_swatch("Text Secondary", "#6B6B65"),
                ],
                16.0,
            ),
            // Accent & semantic
            row(
                vec![
                    color_swatch("Accent: Teal", "#2A7D7D"),
                    color_swatch("Accent: Coral", "#E07A5F"),
                    color_swatch("Success: Sage", "#7D9D7D"),
                    color_swatch("Warning: Amber", "#D4A373"),
                    color_swatch("Error: Rose", "#C97064"),
                ],
                16.0,
            ),
        ],
    )
}

fn typography_section() -> RenderExpr {
    section(
        "Typography Scale",
        vec![
            call(
                "text",
                vec![
                    pos(lit_str("Heading 1 — 24px / 600")),
                    named("size", lit_f64(24.0)),
                    named("bold", lit_bool(true)),
                ],
            ),
            call(
                "text",
                vec![
                    pos(lit_str("Heading 2 — 20px / 600")),
                    named("size", lit_f64(20.0)),
                    named("bold", lit_bool(true)),
                ],
            ),
            call(
                "text",
                vec![
                    pos(lit_str("Heading 3 — 18px / 600")),
                    named("size", lit_f64(18.0)),
                    named("bold", lit_bool(true)),
                ],
            ),
            call(
                "text",
                vec![
                    pos(lit_str("Body — 15px / 400")),
                    named("size", lit_f64(15.0)),
                ],
            ),
            call(
                "text",
                vec![
                    pos(lit_str("UI Label — 13px / 500")),
                    named("size", lit_f64(13.0)),
                    named("bold", lit_bool(true)),
                ],
            ),
            call(
                "text",
                vec![
                    pos(lit_str("Muted secondary text for labels and hints")),
                    named("size", lit_f64(13.0)),
                    named("color", lit_str("muted")),
                ],
            ),
            call(
                "source_block",
                vec![
                    named("language", lit_str("rust")),
                    named(
                        "content",
                        lit_str("// Monospace: JetBrains Mono 14px\nlet x = 42;"),
                    ),
                    named("name", lit_str("mono.rs")),
                ],
            ),
        ],
    )
}

fn spacing_section() -> RenderExpr {
    section(
        "Spacing — 4px Grid",
        vec![
            call(
                "text",
                vec![
                    pos(lit_str("All spacing is a multiple of 4px")),
                    named("color", lit_str("muted")),
                    named("size", lit_f64(13.0)),
                ],
            ),
            // Show different gap sizes
            row(
                vec![
                    call("badge", vec![pos(lit_str("4px"))]),
                    call("badge", vec![pos(lit_str("gap"))]),
                ],
                4.0,
            ),
            row(
                vec![
                    call("badge", vec![pos(lit_str("8px"))]),
                    call("badge", vec![pos(lit_str("gap"))]),
                ],
                8.0,
            ),
            row(
                vec![
                    call("badge", vec![pos(lit_str("12px"))]),
                    call("badge", vec![pos(lit_str("gap"))]),
                ],
                12.0,
            ),
            row(
                vec![
                    call("badge", vec![pos(lit_str("16px"))]),
                    call("badge", vec![pos(lit_str("gap"))]),
                ],
                16.0,
            ),
            row(
                vec![
                    call("badge", vec![pos(lit_str("24px"))]),
                    call("badge", vec![pos(lit_str("gap"))]),
                ],
                24.0,
            ),
        ],
    )
}

fn status_indicators_section() -> RenderExpr {
    section(
        "Status Indicators",
        vec![
            call(
                "text",
                vec![
                    pos(lit_str(
                        "Soft indicators, not alarm signals. Sage, amber, coral — never neon.",
                    )),
                    named("color", lit_str("muted")),
                    named("size", lit_f64(13.0)),
                ],
            ),
            row(
                vec![
                    call("icon", vec![pos(lit_str("check"))]),
                    call(
                        "text",
                        vec![pos(lit_str("Synced")), named("color", lit_str("green"))],
                    ),
                ],
                8.0,
            ),
            row(
                vec![
                    call("icon", vec![pos(lit_str("clock"))]),
                    call(
                        "text",
                        vec![pos(lit_str("Pending")), named("color", lit_str("amber"))],
                    ),
                ],
                8.0,
            ),
            row(
                vec![
                    call("icon", vec![pos(lit_str("alert-triangle"))]),
                    call(
                        "text",
                        vec![
                            pos(lit_str("Attention needed")),
                            named("color", lit_str("coral")),
                        ],
                    ),
                ],
                8.0,
            ),
            row(
                vec![
                    call("icon", vec![pos(lit_str("x"))]),
                    call("error", vec![pos(lit_str("Error: sync failed"))]),
                ],
                8.0,
            ),
            // Task state progression
            call(
                "text",
                vec![
                    pos(lit_str("Task state progression:")),
                    named("bold", lit_bool(true)),
                    named("size", lit_f64(13.0)),
                ],
            ),
            row(
                vec![
                    call("state_toggle", vec![pos(lit_str("task_state_empty"))]),
                    call("text", vec![pos(lit_str("→"))]),
                    call("state_toggle", vec![pos(lit_str("task_state"))]),
                    call("text", vec![pos(lit_str("→"))]),
                    call("state_toggle", vec![pos(lit_str("task_state_doing"))]),
                    call("text", vec![pos(lit_str("→"))]),
                    call("state_toggle", vec![pos(lit_str("task_state_done"))]),
                ],
                8.0,
            ),
        ],
    )
}

fn card_layout_section() -> RenderExpr {
    section(
        "Card Layouts",
        vec![
            call(
                "text",
                vec![
                    pos(lit_str("Cards use Surface background with 12-16px padding")),
                    named("color", lit_str("muted")),
                    named("size", lit_f64(13.0)),
                ],
            ),
            // Simulate a "Today's Focus" card from VISION_UI.md
            section(
                "Today's Focus",
                vec![
                    row(
                        vec![
                            call("icon", vec![pos(lit_str("star"))]),
                            call(
                                "text",
                                vec![
                                    pos(lit_str("Complete API authentication")),
                                    named("bold", lit_bool(true)),
                                ],
                            ),
                            call("badge", vec![pos(lit_str("In Progress"))]),
                        ],
                        8.0,
                    ),
                    row(
                        vec![
                            call("icon", vec![pos(lit_str("star"))]),
                            call(
                                "text",
                                vec![
                                    pos(lit_str("Review PR from Sarah")),
                                    named("bold", lit_bool(true)),
                                ],
                            ),
                            call("badge", vec![pos(lit_str("Ready"))]),
                        ],
                        8.0,
                    ),
                    row(
                        vec![
                            call("icon", vec![pos(lit_str("star"))]),
                            call(
                                "text",
                                vec![
                                    pos(lit_str("Prepare slides for Friday")),
                                    named("bold", lit_bool(true)),
                                ],
                            ),
                            call("badge", vec![pos(lit_str("Not Started"))]),
                        ],
                        8.0,
                    ),
                ],
            ),
            // Inbox + Watcher side by side
            call(
                "columns",
                vec![named(
                    "item_template",
                    column(vec![
                        call(
                            "text",
                            vec![pos(col_ref("name")), named("bold", lit_bool(true))],
                        ),
                        call(
                            "text",
                            vec![
                                pos(col_ref("description")),
                                named("color", lit_str("muted")),
                                named("size", lit_f64(13.0)),
                            ],
                        ),
                    ]),
                )],
            ),
        ],
    )
}

// ── App Mockup Modes (Orient / Flow / Capture) ──────────────────────────

fn watcher_card(source: &str, summary: &str, accent: &str, icon_char: &str) -> RenderExpr {
    call(
        "card",
        vec![
            named("accent", lit_str(accent)),
            pos(row(
                vec![
                    call(
                        "text",
                        vec![pos(lit_str(icon_char)), named("color", lit_str(accent))],
                    ),
                    call(
                        "text",
                        vec![pos(lit_str(source)), named("bold", lit_bool(true))],
                    ),
                ],
                6.0,
            )),
            pos(call(
                "text",
                vec![pos(lit_str(summary)), named("size", lit_f64(13.0))],
            )),
        ],
    )
}

fn orient_task_row(text: &str, accent: Option<&str>) -> RenderExpr {
    let indicator_color = accent.unwrap_or("#3A3A36");
    row(
        vec![
            call(
                "spacer",
                vec![
                    named("width", lit_f64(6.0)),
                    named("height", lit_f64(6.0)),
                    named("color", lit_str(indicator_color)),
                ],
            ),
            call(
                "text",
                vec![pos(lit_str(text)), named("size", lit_f64(13.0))],
            ),
        ],
        8.0,
    )
}

fn orient_project(title: &str, tasks: Vec<RenderExpr>) -> RenderExpr {
    let mut children = vec![row(
        vec![
            call(
                "text",
                vec![
                    pos(lit_str("▾")),
                    named("size", lit_f64(12.0)),
                    named("color", lit_str("muted")),
                ],
            ),
            call(
                "text",
                vec![pos(lit_str(title)), named("bold", lit_bool(true))],
            ),
        ],
        8.0,
    )];
    children.extend(tasks);
    column(children)
}

/// Orient mode: Watcher Synthesis + Today's Plan in two columns.
pub fn orient_mode_expr() -> RenderExpr {
    let watcher_panel = column_gap(
        vec![
            call(
                "text",
                vec![
                    pos(lit_str("Watcher Synthesis")),
                    named("bold", lit_bool(true)),
                    named("size", lit_f64(16.0)),
                ],
            ),
            watcher_card(
                "JIRA",
                "5 high-priority JIRA issues across active projects.",
                "#5DBDBD",
                "✦",
            ),
            watcher_card(
                "Gmail",
                "3 overdue emails from key stakeholders.",
                "#C97064",
                "✉",
            ),
            watcher_card(
                "Linear",
                "2 projects with approaching deadlines.",
                "#7D9D7D",
                "◆",
            ),
            watcher_card(
                "Gmail",
                "4 high-priority JIRA tiks across R projects.",
                "#D4A373",
                "✉",
            ),
            watcher_card("Linear", "1 blocked issue needs triage.", "#5DBDBD", "◆"),
        ],
        12.0,
    );

    let today_panel = column(vec![
        call(
            "text",
            vec![
                pos(lit_str("Today's Plan")),
                named("bold", lit_bool(true)),
                named("size", lit_f64(16.0)),
            ],
        ),
        orient_project(
            "Project: Delta Sharing Implementation",
            vec![
                orient_task_row("Investigate CRDT conflict resolution strategies.", None),
                orient_task_row(
                    "JIRA-123: Define sync protocol specifications.",
                    Some("#5DBDBD"),
                ),
                orient_task_row("Schedule team sync on architecture.", Some("#C97064")),
                orient_task_row("Schedule team sync on architecture.", Some("#7D9D7D")),
                orient_task_row(
                    "JIRA-123: Define sync protocol specifications.",
                    Some("#D4A373"),
                ),
                orient_task_row(
                    "JIRA-123: Define sync protocol specifications.",
                    Some("#5DBDBD"),
                ),
                orient_task_row("Schedule team sync on architecture.", Some("#C97064")),
            ],
        ),
        orient_project(
            "Project: API Gateway Redesign",
            vec![
                orient_task_row("Investigate CRDT conflict resolution strategies.", None),
                orient_task_row("JIRA-456: Review rate limiting strategy.", Some("#5DBDBD")),
                orient_task_row("Overdue emails from key projects.", Some("#D4A373")),
                orient_task_row("Schedule team sync on architecture.", Some("#C97064")),
                orient_task_row("Completed tasks on architecture.", None),
            ],
        ),
    ]);

    row(vec![watcher_panel, today_panel], 24.0)
}

fn flow_task_card(text: &str, accent: &str, icon: &str, completed: bool) -> RenderExpr {
    let mut text_args = vec![pos(lit_str(text)), named("size", lit_f64(14.0))];
    if completed {
        text_args.push(named("color", lit_str("muted")));
    }
    call(
        "card",
        vec![
            named(
                "accent",
                lit_str(if completed { "#555550" } else { accent }),
            ),
            pos(row(
                vec![
                    call(
                        "text",
                        vec![
                            pos(lit_str(icon)),
                            named("color", lit_str(if completed { "muted" } else { accent })),
                        ],
                    ),
                    call("text", text_args),
                ],
                8.0,
            )),
        ],
    )
}

/// Flow mode: focused task cards for active project.
pub fn flow_mode_expr() -> RenderExpr {
    column_gap(
        vec![
            row(
                vec![
                    call(
                        "text",
                        vec![
                            pos(lit_str("▾")),
                            named("size", lit_f64(12.0)),
                            named("color", lit_str("muted")),
                        ],
                    ),
                    call(
                        "text",
                        vec![
                            pos(lit_str("Project: Delta Sharing Implementation")),
                            named("bold", lit_bool(true)),
                            named("size", lit_f64(16.0)),
                        ],
                    ),
                ],
                10.0,
            ),
            flow_task_card(
                "Investigate CRDT conflict resolution strategies.",
                "#5DBDBD",
                "■",
                false,
            ),
            flow_task_card(
                "JIRA-123: Define sync protocol specifications.",
                "#5DBDBD",
                "✦",
                false,
            ),
            flow_task_card("Schedule team sync on architecture.", "#C97064", "■", false),
            flow_task_card("Completed tasks on architecture.", "#555550", "■", true),
        ],
        16.0,
    )
}

/// Capture mode: quick capture input.
pub fn capture_mode_expr() -> RenderExpr {
    column(vec![
        call(
            "text",
            vec![
                pos(lit_str("Quick Capture")),
                named("color", lit_str("muted")),
            ],
        ),
        call(
            "card",
            vec![
                named("accent", lit_str("#3A3A36")),
                pos(call(
                    "text",
                    vec![
                        pos(lit_str("What's on your mind?")),
                        named("size", lit_f64(15.0)),
                        named("color", lit_str("muted")),
                    ],
                )),
            ],
        ),
        call(
            "text",
            vec![
                pos(lit_str("↵ Enter to save")),
                named("size", lit_f64(12.0)),
                named("color", lit_str("muted")),
            ],
        ),
    ])
}

fn chat_msg(sender: &str, time: &str, text: &str) -> RenderExpr {
    call(
        "chat_bubble",
        vec![
            named("sender", lit_str(sender)),
            named("time", lit_str(time)),
            pos(call("text", vec![pos(lit_str(text))])),
        ],
    )
}

fn tool_call_entry(header: &str, icon: &str, detail: &str) -> RenderExpr {
    call(
        "collapsible",
        vec![
            named("summary", lit_str(header)),
            named("icon", lit_str(icon)),
            pos(call(
                "text",
                vec![
                    pos(lit_str(detail)),
                    named("size", lit_f64(12.0)),
                    named("color", lit_str("muted")),
                ],
            )),
        ],
    )
}

/// Chat mode: conversation with assistant, tool calls, and messages.
pub fn chat_mode_expr() -> RenderExpr {
    column_gap(
        vec![
            chat_msg("system", "", "Today — March 25, 2026"),
            chat_msg(
                "user",
                "9:14 AM",
                "Can you help me understand how the CRDT sync protocol works in the Delta Sharing project?",
            ),
            chat_msg(
                "assistant",
                "9:14 AM",
                "The Delta Sharing sync uses a two-phase approach:\n\n1. Capture phase — local edits are recorded as Loro operations in a CRDT document\n2. Reconciliation phase — when peers connect, they exchange operation logs and merge automatically\n\nThe key insight is that Loro's tree-move operations are commutative, so the merge order doesn't matter. Conflicts only arise at the semantic level (e.g. two users editing the same paragraph), not the structural level.",
            ),
            tool_call_entry(
                "Read — crates/holon/src/sync/org_sync_controller.rs",
                "📄",
                "Lines 1-245 of org_sync_controller.rs\n\npub struct OrgSyncController { ... }\nimpl OrgSyncController {\n    pub fn on_remote_changes(&self, ops: Vec<LoroOp>) { ... }\n}",
            ),
            tool_call_entry(
                "Grep — Search for 'Lamport' in crates/holon — 3 matches",
                "🔍",
                "loro_bridge.rs:42: // Lamport timestamp for causal ordering\nloro_bridge.rs:89: let lamport = op.lamport_timestamp();\nconflict.rs:15: // Last-writer-wins using Lamport clock",
            ),
            chat_msg(
                "user",
                "9:15 AM",
                "What about the case where two users move the same block to different parents?",
            ),
            chat_msg(
                "assistant",
                "9:15 AM",
                "Great question. Loro handles tree-move conflicts with a \"last-writer-wins\" policy based on Lamport timestamps. If user A moves block X under parent P1, and user B moves X under P2, the operation with the higher Lamport timestamp wins.\n\nIn practice, this is rare because the Petri net scheduler assigns blocks to focus areas, reducing concurrent moves to the same block.",
            ),
            chat_msg(
                "user",
                "9:16 AM",
                "Makes sense. Can you show me where the reconciliation happens in the code?",
            ),
            tool_call_entry(
                "Grep — Search for 'on_remote_changes' — 2 matches",
                "🔍",
                "org_sync_controller.rs:147: pub fn on_remote_changes(&self, ops: Vec<LoroOp>) {\ndi.rs:203: controller.on_remote_changes(batch);",
            ),
            chat_msg(
                "assistant",
                "9:17 AM",
                "The reconciliation entry point is OrgSyncController::on_remote_changes() in org_sync_controller.rs. It:\n\n1. Receives a batch of remote Loro operations\n2. Applies them to the local Loro document\n3. Diffs the resulting block tree against the SQL cache\n4. Emits block.created / block.updated / block.deleted events\n5. The CacheEventSubscriber picks these up and updates QueryableCache\n\nThe echo suppression (last_projection) prevents file re-writes when the change originated from an org file edit on this machine.",
            ),
            chat_msg(
                "user",
                "9:18 AM",
                "Perfect, that clears it up. One more thing — how does the watcher synthesis tie into this?",
            ),
        ],
        4.0,
    )
}

/// Interpret a mode expression into a ReactiveViewModel.
pub fn mode_view_model(expr: &RenderExpr) -> crate::reactive_view_model::ReactiveViewModel {
    let services = crate::reactive::StubBuilderServices::new();
    crate::reactive::interpret_pure(expr, &[], &services)
}

// ── Standalone interpretation ────────────────────────────────────────────

/// Interpret the design gallery into a `ReactiveViewModel` using `StubBuilderServices`.
///
/// This is the main entry point for standalone gallery apps. No database,
/// no DI, no backend — just the shadow interpreter + hardcoded data.
pub fn design_gallery_view_model() -> crate::reactive_view_model::ReactiveViewModel {
    let services = crate::reactive::StubBuilderServices::new();
    crate::reactive::interpret_pure(
        &design_gallery_render_expr(),
        &design_gallery_rows(),
        &services,
    )
}

/// Interpret the original widget gallery into a `ReactiveViewModel` using `StubBuilderServices`.
pub fn widget_gallery_view_model() -> crate::reactive_view_model::ReactiveViewModel {
    let services = crate::reactive::StubBuilderServices::new();
    crate::reactive::interpret_pure(
        &widget_gallery_render_expr(),
        &widget_gallery_rows(),
        &services,
    )
}

/// Data rows for the design gallery. Includes widget_gallery_rows plus extras.
pub fn design_gallery_rows() -> Vec<std::sync::Arc<HashMap<String, Value>>> {
    widget_gallery_rows()
}

/// Sample data rows for data-dependent widgets in the gallery.
///
/// Columns used across sections:
/// - table/list/columns: name, status, description
/// - tree/outline: id, parent_id, sort_key, name
/// - state_toggle: task_state
/// - pref_field: key, label, value, pref_type, options
pub fn widget_gallery_rows() -> Vec<std::sync::Arc<HashMap<String, Value>>> {
    vec![
        // Root item (tree parent)
        std::sync::Arc::new(HashMap::from([
            ("id".into(), Value::String("root".into())),
            ("parent_id".into(), Value::Null),
            ("sort_key".into(), Value::Integer(0)),
            ("name".into(), Value::String("Block editor".into())),
            ("status".into(), Value::String("Active".into())),
            (
                "description".into(),
                Value::String("Core editing component".into()),
            ),
            ("task_state".into(), Value::String("TODO".into())),
            ("task_state_doing".into(), Value::String("DOING".into())),
            ("task_state_done".into(), Value::String("DONE".into())),
            ("task_state_empty".into(), Value::String("".into())),
            // pref_field data for the choice demo
            ("key".into(), Value::String("gallery.demo_choice".into())),
            ("label".into(), Value::String("Demo Choice".into())),
            ("value".into(), Value::String("option_a".into())),
            ("pref_type".into(), Value::String("choice".into())),
            (
                "options".into(),
                Value::Array(vec![
                    Value::String("option_a".into()),
                    Value::String("option_b".into()),
                    Value::String("option_c".into()),
                ]),
            ),
        ])),
        // Child of root
        std::sync::Arc::new(HashMap::from([
            ("id".into(), Value::String("child1".into())),
            ("parent_id".into(), Value::String("root".into())),
            ("sort_key".into(), Value::Integer(0)),
            ("name".into(), Value::String("Sync engine".into())),
            ("status".into(), Value::String("Beta".into())),
            (
                "description".into(),
                Value::String("Real-time collaboration".into()),
            ),
            ("key".into(), Value::String("gallery.demo_toggle".into())),
            ("label".into(), Value::String("Demo Toggle".into())),
            ("value".into(), Value::Boolean(true)),
            ("pref_type".into(), Value::String("toggle".into())),
        ])),
        // Another child of root
        std::sync::Arc::new(HashMap::from([
            ("id".into(), Value::String("child2".into())),
            ("parent_id".into(), Value::String("root".into())),
            ("sort_key".into(), Value::Integer(1)),
            ("name".into(), Value::String("Query compiler".into())),
            ("status".into(), Value::String("Stable".into())),
            (
                "description".into(),
                Value::String("PRQL / GQL / SQL".into()),
            ),
        ])),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gallery_expr_has_six_sections() {
        let expr = widget_gallery_render_expr();
        match &expr {
            RenderExpr::FunctionCall { name, args } => {
                assert_eq!(name, "column");
                assert_eq!(args.len(), 6, "expected 6 sections in gallery");
                for arg in args {
                    match &arg.value {
                        RenderExpr::FunctionCall { name, .. } => {
                            assert_eq!(name, "section");
                        }
                        other => panic!("expected section FunctionCall, got {other:?}"),
                    }
                }
            }
            other => panic!("expected column FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn design_gallery_has_eleven_sections() {
        let expr = design_gallery_render_expr();
        match &expr {
            RenderExpr::FunctionCall { name, args } => {
                assert_eq!(name, "column");
                assert_eq!(args.len(), 11, "expected 11 sections in design gallery");
            }
            other => panic!("expected column FunctionCall, got {other:?}"),
        }
    }

    #[test]
    fn design_gallery_view_model_produces_tree() {
        let vm = design_gallery_view_model();
        // Should be a Col with children (the 11 sections)
        let snapshot = vm.snapshot();
        assert!(
            !matches!(snapshot.kind, crate::view_model::ViewKind::Empty),
            "design gallery should not be empty"
        );
    }

    #[test]
    fn chat_mode_has_collapsibles_with_children() {
        let expr = chat_mode_expr();
        let vm = mode_view_model(&expr);
        let snap = vm.snapshot();
        tracing::debug!("{snap:#?}");

        fn find_collapsibles(
            vm: &crate::view_model::ViewModel,
        ) -> Vec<&crate::view_model::ViewModel> {
            let mut result = vec![];
            if vm.widget_name() == Some("collapsible") {
                result.push(vm);
            }
            for child in vm.children() {
                result.extend(find_collapsibles(child));
            }
            result
        }

        let collapsibles = find_collapsibles(&snap);
        assert!(
            collapsibles.len() >= 3,
            "expected >=3 collapsibles, got {}",
            collapsibles.len()
        );
        for (i, c) in collapsibles.iter().enumerate() {
            assert!(
                !c.children().is_empty(),
                "collapsible {i} should have children"
            );
        }
    }

    #[test]
    fn gallery_rows_non_empty() {
        let rows = widget_gallery_rows();
        assert!(rows.len() >= 3);
        assert!(rows[0].contains_key("name"));
        assert!(rows[0].contains_key("task_state"));
    }
}
