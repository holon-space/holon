use std::collections::HashMap;

use holon_api::Value;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;

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
        accordion_section(),
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

fn note(s: &str) -> RenderExpr {
    call(
        "text",
        vec![pos(lit_str(s)), named("color", lit_str("muted"))],
    )
}

/// Accordion — documentation entry (plan §3 prop table). Rendered as text, not
/// a live `accordion(...)`: the widget is fail-loud about placement (it must be
/// a DIRECT child of a main-panel flow column) and the gallery is not one, so a
/// live instance would correctly render the placement error widget instead.
fn accordion_section() -> RenderExpr {
    section(
        "Accordion (bounded auxiliary region)",
        vec![
            call(
                "text",
                vec![pos(lit_str(
                    "accordion(#{...}, child…) — a bounded, collapsible region \
                     pinned at the bottom of a main-panel column.",
                ))],
            ),
            note("Props:"),
            note("• title: String — header text (default: empty)."),
            note("• icon: String — header icon name (optional)."),
            note(
                "• max_height_fraction: Float — cap as a fraction of the PANEL \
                 height; default 0.4; must be finite and in (0.0, 1.0].",
            ),
            note("• collapsible: Bool — default true."),
            note("• collapsed: Bool — initial state only; default false."),
            note(
                "• pinned: Bool — default true. true = pinned footer at the \
                 column bottom (needs a flow-column parent). false = in-flow: \
                 capped+collapsible at its natural position inside a section \
                 stack (no split, scrolls with siblings).",
            ),
            note(
                "• sticky: Bool — default false; wins over pinned. An absolute \
                 .occlude() overlay footer inside a section stack, pinned by \
                 min(viewport_bottom − footer_h, next_section_top); the cap is \
                 px-computed (fraction × the section stack's DEFINITE height, \
                 asserted fail-loud). A wheel over the footer is hard-contained \
                 (v1 — no fall-through).",
            ),
            note(
                "Placement (fail-loud; anywhere else renders the standard error \
                 widget, never a silently-mispositioned region): pinned:true \
                 MUST be a direct child of a main-panel (flow) column; \
                 pinned:false and sticky:true MUST be inside a section stack.",
            ),
            note(
                "Semantics: the cap is a fraction of the panel height (window \
                 minus chrome). Content shorter than the cap shrinks to fit; \
                 taller content scrolls WITHIN the capped region while the \
                 outline above scrolls independently.",
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

/// Board mode: kanban-style lanes rendered through `card(...)`.
///
/// Demonstrates the `board(item_template: card(...), lane_field: ..., rows:
/// [...])` shape end-to-end. The shadow `board` builder groups the inline rows
/// by `lane_field` into `board_lane(...)` view models whose children are
/// interpreted via `item_template` — i.e. through the regular `card` builder.
pub fn board_mode_expr() -> RenderExpr {
    fn card_row(title: &str, status: &str, accent: &str, summary: &str) -> Value {
        Value::Object(HashMap::from([
            ("title".to_string(), Value::String(title.into())),
            ("status".to_string(), Value::String(status.into())),
            ("accent".to_string(), Value::String(accent.into())),
            ("summary".to_string(), Value::String(summary.into())),
        ]))
    }

    let rows = Value::Array(vec![
        card_row(
            "Design review",
            "To Do",
            "#5DBDBD",
            "Walk through the new VISION_UI palette.",
        ),
        card_row(
            "Write tests",
            "To Do",
            "#7D9D7D",
            "Cover the board grouping path.",
        ),
        card_row(
            "Fix CI pipeline",
            "To Do",
            "#C97064",
            "Flaky lint job blocks merges.",
        ),
        card_row(
            "Update docs",
            "To Do",
            "#D4A373",
            "Refresh the design gallery section.",
        ),
        card_row(
            "Sortable widget",
            "In Progress",
            "#9D7DBD",
            "Drag-and-drop polish.",
        ),
        card_row(
            "Board demo",
            "In Progress",
            "#5DBDBD",
            "End-to-end card composition.",
        ),
        card_row("Ship v2.0", "Done", "#7D9D7D", "Tagged and announced."),
        card_row(
            "Customer demo",
            "Done",
            "#D4A373",
            "Recorded walkthrough shipped.",
        ),
    ]);

    let card_template = call(
        "card",
        vec![
            named("accent", col_ref("accent")),
            pos(call(
                "text",
                vec![pos(col_ref("title")), named("bold", lit_bool(true))],
            )),
            pos(call(
                "text",
                vec![
                    pos(col_ref("summary")),
                    named("size", lit_f64(13.0)),
                    named("color", lit_str("muted")),
                ],
            )),
        ],
    );

    let lane_order = Value::Array(vec![
        Value::String("To Do".into()),
        Value::String("In Progress".into()),
        Value::String("Done".into()),
    ]);

    call(
        "board",
        vec![
            named("item_template", card_template),
            named("lane_field", lit_str("status")),
            named("lane_order", RenderExpr::Literal { value: lane_order }),
            named("rows", RenderExpr::Literal { value: rows }),
        ],
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
                "Can you help me understand how the CRDT sync protocol works in the Delta Sharing \
                 project?",
            ),
            chat_msg(
                "assistant",
                "9:14 AM",
                "The Delta Sharing sync uses a two-phase approach:\n\n1. Capture phase — local \
                 edits are recorded as Loro operations in a CRDT document\n2. Reconciliation \
                 phase — when peers connect, they exchange operation logs and merge \
                 automatically\n\nThe key insight is that Loro's tree-move operations are \
                 commutative, so the merge order doesn't matter. Conflicts only arise at the \
                 semantic level (e.g. two users editing the same paragraph), not the structural \
                 level.",
            ),
            tool_call_entry(
                "Read — crates/holon/src/sync/file_sync_controller.rs",
                "📄",
                "Lines 1-245 of file_sync_controller.rs\n\npub struct FileSyncController { ... \
                 }\nimpl FileSyncController {\n    pub fn on_remote_changes(&self, ops: \
                 Vec<LoroOp>) { ... }\n}",
            ),
            tool_call_entry(
                "Grep — Search for 'Lamport' in crates/holon — 3 matches",
                "🔍",
                "loro_bridge.rs:42: // Lamport timestamp for causal ordering\nloro_bridge.rs:89: \
                 let lamport = op.lamport_timestamp();\nconflict.rs:15: // Last-writer-wins using \
                 Lamport clock",
            ),
            chat_msg(
                "user",
                "9:15 AM",
                "What about the case where two users move the same block to different parents?",
            ),
            chat_msg(
                "assistant",
                "9:15 AM",
                "Great question. Loro handles tree-move conflicts with a \"last-writer-wins\" \
                 policy based on Lamport timestamps. If user A moves block X under parent P1, and \
                 user B moves X under P2, the operation with the higher Lamport timestamp \
                 wins.\n\nIn practice, this is rare because the Petri net scheduler assigns \
                 blocks to focus areas, reducing concurrent moves to the same block.",
            ),
            chat_msg(
                "user",
                "9:16 AM",
                "Makes sense. Can you show me where the reconciliation happens in the code?",
            ),
            tool_call_entry(
                "Grep — Search for 'on_remote_changes' — 2 matches",
                "🔍",
                "file_sync_controller.rs:147: pub fn on_remote_changes(&self, ops: Vec<LoroOp>) \
                 {\ndi.rs:203: controller.on_remote_changes(batch);",
            ),
            chat_msg(
                "assistant",
                "9:17 AM",
                "The reconciliation entry point is FileSyncController::on_remote_changes() in \
                 file_sync_controller.rs. It:\n\n1. Receives a batch of remote Loro \
                 operations\n2. Applies them to the local Loro document\n3. Diffs the resulting \
                 block tree against the SQL cache\n4. Emits block.created / block.updated / \
                 block.deleted events\n5. The CacheEventSubscriber picks these up and updates \
                 QueryableCache\n\nThe echo suppression (last_projection) prevents file re-writes \
                 when the change originated from an org file edit on this machine.",
            ),
            chat_msg(
                "user",
                "9:18 AM",
                "Perfect, that clears it up. One more thing — how does the watcher synthesis tie \
                 into this?",
            ),
        ],
        4.0,
    )
}

// ── Actions Mode (ADR 0024 / docs/Proposals/action-ux.md) ────────────────
// Mocks the MVP action UX: rules become the most legible blocks on the page.
// Everything is a render profile over (hard-coded) vault data — no backend.

fn muted_line(text: &str) -> RenderExpr {
    call(
        "text",
        vec![
            pos(lit_str(text)),
            named("size", lit_f64(13.0)),
            named("color", lit_str("muted")),
        ],
    )
}

/// A rule block rendered as its collapsed **rule card**: name, enabled toggle,
/// `last fired: …`, an optional fail-loud guard error, and the when/then
/// source in an expandable section. `error` present → red error state.
fn rule_card(
    name: &str,
    accent: &str,
    enabled: bool,
    last_fired: &str,
    source: &str,
    error: Option<&str>,
) -> RenderExpr {
    let header = row(
        vec![
            call(
                "text",
                vec![pos(lit_str("⚙")), named("color", lit_str(accent))],
            ),
            call(
                "text",
                vec![
                    pos(lit_str(name)),
                    named("bold", lit_bool(true)),
                    named("size", lit_f64(16.0)),
                ],
            ),
            call("checkbox", vec![named("checked", lit_bool(enabled))]),
            call(
                "text",
                vec![
                    pos(lit_str(if enabled { "Enabled" } else { "Disabled" })),
                    named("size", lit_f64(13.0)),
                    named("color", lit_str(if enabled { "success" } else { "muted" })),
                ],
            ),
        ],
        8.0,
    );

    let mut children = vec![pos(header)];
    if let Some(err) = error {
        children.push(pos(call("error", vec![pos(lit_str(err))])));
    }
    children.push(pos(muted_line(&format!("last fired: {last_fired}"))));
    children.push(pos(call(
        "collapsible",
        vec![
            named("summary", lit_str("when / then source")),
            named("icon", lit_str("⚙")),
            pos(call("text", vec![named("content", lit_str(source))])),
        ],
    )));

    let mut args = vec![named("accent", lit_str(accent))];
    args.extend(children);
    call("card", args)
}

/// A journal-entry block row carrying the subtle ⚙ provenance affordance and
/// inline "created by <rule> · <time>" secondary text (ADR 0024 P8 `fired-by`).
fn provenance_row(entry: &str, rule: &str, time: &str, badge: bool) -> RenderExpr {
    let gear = call(
        "text",
        vec![
            pos(lit_str("⚙")),
            named("size", lit_f64(12.0)),
            named("color", lit_str("muted")),
        ],
    );
    let mut cells = vec![call(
        "text",
        vec![pos(lit_str(entry)), named("size", lit_f64(14.0))],
    )];
    // DECIDED (action-ux.md, 2026-07-09 demo review): provenance is
    // hover-only. The "created by <rule> · <time>" caption lives inside
    // `on_hover`, revealed only while the ⚙ trigger is hovered.
    if badge {
        cells.push(call(
            "on_hover",
            vec![
                pos(gear),
                pos(muted_line(&format!("created by {rule} · {time}"))),
            ],
        ));
    } else {
        cells.push(gear);
    }
    row(cells, 8.0)
}

/// One firing in the Automations audit journal. `accent` tints the tick.
fn automation_row(time: &str, rule: &str, detail: &str, accent: &str) -> RenderExpr {
    row(
        vec![
            call(
                "text",
                vec![
                    pos(lit_str(time)),
                    named("size", lit_f64(12.0)),
                    named("color", lit_str("muted")),
                ],
            ),
            call(
                "text",
                vec![pos(lit_str("⚙")), named("color", lit_str(accent))],
            ),
            call(
                "text",
                vec![
                    pos(lit_str(rule)),
                    named("bold", lit_bool(true)),
                    named("size", lit_f64(13.0)),
                ],
            ),
            call(
                "text",
                vec![pos(lit_str("→")), named("color", lit_str("muted"))],
            ),
            call(
                "text",
                vec![pos(lit_str(detail)), named("size", lit_f64(13.0))],
            ),
        ],
        8.0,
    )
}

/// A named enabled-transition affordance for the in-context action bar (ADR
/// 0024 presentation ladder #1: horizontal text is the highest-bandwidth
/// label format). Rendered as a `badge` — the real data-driven `op_button`
/// widget requires a `chain_ops`/`ops_of` row source, absent in this static
/// mock; a badge carries the same named-transition label statically.
fn action_button(display_name: &str) -> RenderExpr {
    call("badge", vec![pos(lit_str(display_name))])
}

/// Actions mode: rule cards + provenance + automations page + action bar +
/// dry-run dialog. One scrollable column of sections, like the other modes.
pub fn actions_mode_expr() -> RenderExpr {
    // ── 1. Rule cards ────────────────────────────────────────────────────
    let rule_cards = section(
        "Rules",
        vec![
            muted_line(
                "Rules render as the most legible blocks on the page — not broken query results.",
            ),
            rule_card(
                "Daily journal",
                "#7D9D7D",
                true,
                "today 00:03",
                "when: today and not journal(date = today)\nthen: block.create\n  parent: \
                 journals\n  name: \"{today}\"",
                None,
            ),
            rule_card(
                "Weekly digest",
                "#C97064",
                false,
                "never",
                "when: jurnal(date = last_week)\nthen: block.create\n  parent: digests\n  name: \
                 \"Digest {week}\"",
                Some("guard references unknown relation 'jurnal'"),
            ),
        ],
    );

    // ── 2. Provenance badge ──────────────────────────────────────────────
    let provenance = section(
        "Provenance",
        vec![
            muted_line(
                "Every auto-created block carries a ⚙ affordance back to the rule that made it.",
            ),
            provenance_row("2026-07-09", "Daily journal", "00:03", true),
            provenance_row("2026-07-08", "Daily journal", "00:02", false),
        ],
    );

    // ── 3. Automations page ──────────────────────────────────────────────
    let automations = section(
        "Automations",
        vec![
            muted_line(
                "An ordinary, user-customizable page: a query over provenance-stamped effects.",
            ),
            call(
                "text",
                vec![
                    pos(lit_str("Today — 2026-07-09")),
                    named("bold", lit_bool(true)),
                    named("size", lit_f64(13.0)),
                ],
            ),
            automation_row(
                "00:03",
                "Daily journal",
                "created '2026-07-09' in Journals",
                "#7D9D7D",
            ),
            call(
                "text",
                vec![
                    pos(lit_str("Yesterday — 2026-07-08")),
                    named("bold", lit_bool(true)),
                    named("size", lit_f64(13.0)),
                ],
            ),
            automation_row(
                "00:02",
                "Daily journal",
                "created '2026-07-08' in Journals",
                "#7D9D7D",
            ),
            automation_row(
                "14:20",
                "Archive done",
                "moved 'Ship v2.0' to Archive",
                "#5DBDBD",
            ),
            automation_row(
                "09:00",
                "Weekly digest",
                "guard error: unknown relation 'jurnal'",
                "#C97064",
            ),
        ],
    );

    // ── 4. In-context action bar ─────────────────────────────────────────
    let action_bar = section(
        "In-context actions",
        vec![
            muted_line("The net computes what you can do; the UI offers it where you're looking."),
            call(
                "card",
                vec![
                    named("accent", lit_str("#5DBDBD")),
                    pos(row(
                        vec![
                            call("state_toggle", vec![pos(lit_str("task_state_doing"))]),
                            call(
                                "text",
                                vec![
                                    pos(lit_str("Review PR from Sarah")),
                                    named("bold", lit_bool(true)),
                                    named("size", lit_f64(15.0)),
                                ],
                            ),
                        ],
                        8.0,
                    )),
                    pos(muted_line("3 actions available here")),
                    pos(row(
                        vec![
                            action_button("Send to review"),
                            action_button("Archive with note"),
                            action_button("Split"),
                        ],
                        12.0,
                    )),
                ],
            ),
        ],
    );

    // ── 5. Dry-run dialog ────────────────────────────────────────────────
    let dry_run = section(
        "Dry-run before enable",
        vec![
            muted_line("Enabling a new rule first shows what it would do — scary becomes boring."),
            call(
                "card",
                vec![
                    named("accent", lit_str("#D4A373")),
                    pos(call(
                        "text",
                        vec![
                            pos(lit_str("Enable 'Daily journal'?")),
                            named("bold", lit_bool(true)),
                            named("size", lit_f64(16.0)),
                        ],
                    )),
                    pos(call(
                        "text",
                        vec![
                            pos(lit_str("This rule would fire 3 times right now:")),
                            named("size", lit_f64(14.0)),
                        ],
                    )),
                    pos(column_gap(
                        vec![
                            muted_line("→ create '2026-07-07' in Journals"),
                            muted_line("→ create '2026-07-08' in Journals"),
                            muted_line("→ create '2026-07-09' in Journals"),
                        ],
                        4.0,
                    )),
                    pos(row(
                        vec![action_button("Cancel"), action_button("Confirm & fire")],
                        12.0,
                    )),
                ],
            ),
        ],
    );

    column_gap(
        vec![rule_cards, provenance, automations, action_bar, dry_run],
        24.0,
    )
}

/// Interpret a mode expression into a ReactiveViewModel.
pub fn mode_view_model(expr: &RenderExpr) -> crate::reactive_view_model::ReactiveViewModel {
    let services = crate::reactive::StubBuilderServices::new();
    crate::reactive::interpret_pure(expr, &[], &services)
}

// ── Standalone interpretation ────────────────────────────────────────────

/// Interpret the design gallery into a `ReactiveViewModel` using
/// `StubBuilderServices`.
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

/// Interpret the original widget gallery into a `ReactiveViewModel` using
/// `StubBuilderServices`.
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
            ("id".into(), Value::String("block:root".into())),
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
            ("id".into(), Value::String("block:child1".into())),
            ("parent_id".into(), Value::String("block:root".into())),
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
            ("id".into(), Value::String("block:child2".into())),
            ("parent_id".into(), Value::String("block:root".into())),
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
    fn actions_mode_has_five_sections_and_renders() {
        let expr = actions_mode_expr();
        match &expr {
            RenderExpr::FunctionCall { name, args } => {
                assert_eq!(name, "column");
                // 5 sections + a trailing `gap` named arg.
                let sections = args
                    .iter()
                    .filter(|a| matches!(&a.value, RenderExpr::FunctionCall { name, .. } if name == "section"))
                    .count();
                assert_eq!(sections, 5, "expected 5 Actions-mode sections");
            }
            other => panic!("expected column FunctionCall, got {other:?}"),
        }
        // Interprets into a non-empty tree via the shadow pipeline.
        let vm = mode_view_model(&expr);
        let snap = vm.snapshot();
        assert!(!matches!(snap.kind, crate::view_model::ViewKind::Empty));

        // Provenance is hover-only (action-ux.md): the "created by …" caption
        // is wrapped in an `on_hover` node whose trigger is the ⚙ affordance.
        fn find_on_hover(vm: &crate::view_model::ViewModel) -> Vec<&crate::view_model::ViewModel> {
            let mut out = vec![];
            if vm.widget_name() == Some("on_hover") {
                out.push(vm);
            }
            for child in vm.children() {
                out.extend(find_on_hover(child));
            }
            out
        }
        let on_hovers = find_on_hover(&snap);
        assert!(
            !on_hovers.is_empty(),
            "expected at least one on_hover node in Actions-mode snapshot"
        );
        // Each on_hover carries a trigger child plus the revealed content.
        assert!(
            on_hovers.iter().all(|n| n.children().len() >= 2),
            "on_hover should carry a trigger child and content child"
        );
    }

    /// Root-cause guard for the "hover never reveals" bug: the on_hover node
    /// carries its own `hovered` `Mutable<bool>`, and the reveal only survives
    /// if the SAME node instance is reused across renders. The GPUI gallery
    /// used to rebuild this tree every frame, so `.on_hover` flipped a Mutable
    /// that the very next repaint discarded — the content could never show.
    ///
    /// This test pins both halves of that mechanism:
    ///  - reusing the node preserves a flipped `hovered` (what the fixed
    ///    gallery does by caching `Arc<ReactiveViewModel>` across renders),
    ///  - a freshly rebuilt tree resets `hovered` to its `false` seed (why the
    ///    per-frame rebuild hid the reveal).
    #[test]
    fn on_hover_state_survives_only_when_node_is_reused() {
        fn find_on_hover(
            node: &crate::reactive_view_model::ReactiveViewModel,
        ) -> Option<&crate::reactive_view_model::ReactiveViewModel> {
            if node.widget_name().as_deref() == Some("on_hover") {
                return Some(node);
            }
            node.children.iter().find_map(|c| find_on_hover(c))
        }

        let expr = actions_mode_expr();

        let vm = mode_view_model(&expr);
        let node = find_on_hover(&vm).expect("actions mode has an on_hover node");
        let hovered = node
            .hovered
            .clone()
            .expect("on_hover node seeds a `hovered` Mutable");
        assert!(!hovered.get(), "hover seeds `false`");

        // Simulate `.on_hover(true)` firing on the persisted node.
        hovered.set(true);
        let same = find_on_hover(&vm).expect("on_hover node still present");
        assert!(
            same.hovered.as_ref().unwrap().get(),
            "a reused node keeps the flipped hover state across a re-render"
        );

        // Rebuilding from scratch — the old per-frame behaviour — loses it.
        let rebuilt = mode_view_model(&expr);
        let fresh = find_on_hover(&rebuilt).expect("on_hover node present in rebuild");
        assert!(
            !fresh.hovered.as_ref().unwrap().get(),
            "a freshly rebuilt tree resets hover to false — the per-frame rebuild is exactly what \
             hid the reveal"
        );
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

    #[test]
    fn board_mode_groups_cards_into_lanes() {
        let expr = board_mode_expr();
        let vm = mode_view_model(&expr);

        assert_eq!(vm.widget_name().as_deref(), Some("board"));
        assert_eq!(vm.prop_str("lane_field").as_deref(), Some("status"));

        let lane_titles: Vec<String> = vm
            .children
            .iter()
            .map(|lane| {
                assert_eq!(lane.widget_name().as_deref(), Some("board_lane"));
                assert!(
                    lane.children
                        .iter()
                        .all(|c| c.widget_name().as_deref() == Some("card")),
                    "every lane child should be a card"
                );
                lane.prop_str("title").unwrap_or_default()
            })
            .collect();

        assert_eq!(lane_titles, vec!["To Do", "In Progress", "Done"]);
    }

    /// Snapshot fidelity (#4, plan quirky-dazzling-map Part B): a board node
    /// must snapshot as `ViewKind::Board` carrying `lane_field` — not collapse
    /// into `ViewKind::List` like before. Children stay flat (rows, not
    /// lanes) — that's the documented fidelity limit, not a bug.
    #[test]
    fn board_mode_snapshot_is_board_not_list() {
        let expr = board_mode_expr();
        let vm = mode_view_model(&expr);
        match vm.snapshot().kind {
            crate::view_model::ViewKind::Board { lane_field, .. } => {
                assert_eq!(lane_field, "status");
            }
            other => panic!("board snapshot collapsed to {:?}", other.tag()),
        }
    }

    #[test]
    fn board_lane_order_default_is_lexicographic() {
        // Without `lane_order`, lanes should sort lexicographically — gives
        // determinism across reloads even when no explicit order is set.
        let row = |status: &str, title: &str| {
            Value::Object(HashMap::from([
                ("title".to_string(), Value::String(title.into())),
                ("status".to_string(), Value::String(status.into())),
            ]))
        };
        let rows = Value::Array(vec![
            row("Bravo", "b1"),
            row("Charlie", "c1"),
            row("Alpha", "a1"),
        ]);
        let card_template = call("card", vec![pos(call("text", vec![pos(col_ref("title"))]))]);
        let expr = call(
            "board",
            vec![
                named("item_template", card_template),
                named("lane_field", lit_str("status")),
                named("rows", RenderExpr::Literal { value: rows }),
            ],
        );
        let vm = mode_view_model(&expr);
        let titles: Vec<String> = vm
            .children
            .iter()
            .map(|l| l.prop_str("title").unwrap_or_default())
            .collect();
        assert_eq!(titles, vec!["Alpha", "Bravo", "Charlie"]);
    }

    #[test]
    fn board_empty_lane_value_uses_default_label() {
        let row = |status: &str, title: &str| {
            Value::Object(HashMap::from([
                ("title".to_string(), Value::String(title.into())),
                ("status".to_string(), Value::String(status.into())),
            ]))
        };
        let rows = Value::Array(vec![row("", "untriaged"), row("Done", "shipped")]);
        let card_template = call("card", vec![pos(call("text", vec![pos(col_ref("title"))]))]);
        let expr = call(
            "board",
            vec![
                named("item_template", card_template),
                named("lane_field", lit_str("status")),
                named("rows", RenderExpr::Literal { value: rows }),
            ],
        );
        let vm = mode_view_model(&expr);
        let titles: Vec<String> = vm
            .children
            .iter()
            .map(|l| l.prop_str("title").unwrap_or_default())
            .collect();
        // Default label "No status" sorts before "Done" lexicographically.
        assert_eq!(titles, vec!["Done", "No status"]);
    }

    #[test]
    fn board_card_carries_row_id_when_present() {
        let row = |id: &str, status: &str| {
            Value::Object(HashMap::from([
                ("id".to_string(), Value::String(id.into())),
                ("status".to_string(), Value::String(status.into())),
                ("title".to_string(), Value::String(format!("t-{id}"))),
            ]))
        };
        let rows = Value::Array(vec![row("row-1", "Done"), row("row-2", "Done")]);
        let card_template = call("card", vec![pos(call("text", vec![pos(col_ref("title"))]))]);
        let expr = call(
            "board",
            vec![
                named("item_template", card_template),
                named("lane_field", lit_str("status")),
                named("rows", RenderExpr::Literal { value: rows }),
            ],
        );
        let vm = mode_view_model(&expr);
        let lane = vm.children.first().expect("one lane");
        let row_ids: Vec<String> = lane
            .children
            .iter()
            .map(|c| c.prop_str("row_id").unwrap_or_default())
            .collect();
        assert_eq!(row_ids, vec!["row-1", "row-2"]);
    }

    #[test]
    fn board_lane_width_arg_lands_as_board_prop() {
        // `lane_width` accepts a literal number and surfaces as a top-level
        // board prop so the GPUI renderer can read it. (Computed values
        // arrive pre-resolved through the same code path.)
        let row = || {
            Value::Object(HashMap::from([(
                "status".to_string(),
                Value::String("Done".into()),
            )]))
        };
        let rows = Value::Array(vec![row()]);
        let card_template = call("card", vec![pos(call("text", vec![pos(lit_str("hi"))]))]);
        let expr = call(
            "board",
            vec![
                named("item_template", card_template),
                named("lane_field", lit_str("status")),
                named("lane_width", lit_f64(320.0)),
                named("rows", RenderExpr::Literal { value: rows }),
            ],
        );
        let vm = mode_view_model(&expr);
        assert_eq!(vm.prop_f64("lane_width"), Some(320.0));
    }

    #[test]
    fn board_lane_width_absent_when_arg_omitted() {
        // No arg → no prop on the VM → GPUI falls through to its default.
        let row = || {
            Value::Object(HashMap::from([(
                "status".to_string(),
                Value::String("Done".into()),
            )]))
        };
        let rows = Value::Array(vec![row()]);
        let card_template = call("card", vec![pos(call("text", vec![pos(lit_str("hi"))]))]);
        let expr = call(
            "board",
            vec![
                named("item_template", card_template),
                named("lane_field", lit_str("status")),
                named("rows", RenderExpr::Literal { value: rows }),
            ],
        );
        let vm = mode_view_model(&expr);
        assert_eq!(vm.prop_f64("lane_width"), None);
    }

    #[test]
    fn board_card_carries_sort_key_when_present() {
        let row = |id: &str, sort_key: &str| {
            Value::Object(HashMap::from([
                ("id".to_string(), Value::String(id.into())),
                ("status".to_string(), Value::String("Done".into())),
                ("title".to_string(), Value::String(format!("t-{id}"))),
                ("sort_key".to_string(), Value::String(sort_key.into())),
            ]))
        };
        let rows = Value::Array(vec![row("row-1", "A0"), row("row-2", "B0")]);
        let card_template = call("card", vec![pos(call("text", vec![pos(col_ref("title"))]))]);
        let expr = call(
            "board",
            vec![
                named("item_template", card_template),
                named("lane_field", lit_str("status")),
                named("rows", RenderExpr::Literal { value: rows }),
            ],
        );
        let vm = mode_view_model(&expr);
        let lane = vm.children.first().expect("one lane");
        let sort_keys: Vec<String> = lane
            .children
            .iter()
            .map(|c| c.prop_str("sort_key").unwrap_or_default())
            .collect();
        assert_eq!(sort_keys, vec!["A0", "B0"]);
    }

    #[test]
    fn board_state_accent_drives_card_accent_per_row() {
        // Proves the end-to-end flow:
        //   board(item_template: card(accent: state_accent(col("status")), ...))
        // The state_accent value-fn runs during card-template arg evaluation
        // for each row, so each card gets the palette color matching its lane.
        let row = |status: &str| {
            Value::Object(HashMap::from([
                ("status".to_string(), Value::String(status.into())),
                ("title".to_string(), Value::String(format!("t-{status}"))),
            ]))
        };
        let rows = Value::Array(vec![row("Done"), row("In Progress"), row("Blocked")]);
        let card_template = call(
            "card",
            vec![
                named("accent", call("state_accent", vec![pos(col_ref("status"))])),
                pos(call("text", vec![pos(col_ref("title"))])),
            ],
        );
        let expr = call(
            "board",
            vec![
                named("item_template", card_template),
                named("lane_field", lit_str("status")),
                named("rows", RenderExpr::Literal { value: rows }),
            ],
        );
        let vm = mode_view_model(&expr);

        let mut accent_by_status: HashMap<String, String> = HashMap::new();
        for lane in vm.children.iter() {
            let title = lane.prop_str("title").unwrap_or_default();
            let card = lane.children.first().expect("one card per lane");
            let accent = card.prop_str("accent").unwrap_or_default();
            accent_by_status.insert(title, accent);
        }
        assert_eq!(
            accent_by_status.get("Done").map(String::as_str),
            Some("#7D9D7D")
        );
        assert_eq!(
            accent_by_status.get("In Progress").map(String::as_str),
            Some("#D4A373"),
        );
        assert_eq!(
            accent_by_status.get("Blocked").map(String::as_str),
            Some("#C97064"),
        );
    }
}

#[cfg(test)]
mod board_yaml_parse_tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use holon_api::Value;
    use holon_api::widget_spec::DataRow;

    use crate::reactive;
    use crate::reactive::BuilderServices;

    #[test]
    fn collection_profile_board_render_parses_and_interprets() {
        crate::shadow_builders::register_render_dsl_widget_names();
        let src = r#"board(#{lane_field: col("lane_field"), item_template: card(#{accent: state_accent(col("task_state"))}, text(col("content"), #{bold: true}))})"#;
        let expr = holon_api::render_dsl::parse_render_dsl(src).expect("YAML render parses");
        let services = reactive::StubBuilderServices::new();
        let vm = reactive::interpret_pure(&expr, &[], &services);
        assert_eq!(vm.widget_name().as_deref(), Some("board"));
    }

    #[test]
    fn board_handles_real_block_rows_without_panicking() {
        crate::shadow_builders::register_render_dsl_widget_names();
        let src = r#"board(#{lane_field: col("lane_field"), item_template: card(#{accent: state_accent(col("task_state"))}, text(col("content"), #{bold: true}))})"#;
        let expr = holon_api::render_dsl::parse_render_dsl(src).expect("YAML render parses");
        let services = reactive::StubBuilderServices::new();

        let make_row = |id: &str, task_state: Option<&str>, content: &str| -> Arc<DataRow> {
            let mut m: HashMap<String, Value> = HashMap::new();
            m.insert("id".into(), Value::String(id.into()));
            m.insert("content".into(), Value::String(content.into()));
            m.insert(
                "task_state".into(),
                match task_state {
                    Some(s) => Value::String(s.into()),
                    None => Value::Null,
                },
            );
            m.insert("sort_key".into(), Value::String("A0".into()));
            m.insert("lane_field".into(), Value::String("task_state".into()));
            Arc::new(m)
        };

        let rows = vec![
            make_row("block:r1", None, "Sync status indicators"),
            make_row("block:r2", Some(""), "Empty state row"),
            make_row("block:r3", Some("DOING"), "Drag persistence wiring"),
            make_row("block:r4", Some("DONE"), "State accent value-fn"),
            make_row("block:r5", Some("BLOCKED"), "Within-lane reorder"),
        ];

        let vm = reactive::interpret_pure(&expr, &rows, &services);
        assert_eq!(vm.widget_name().as_deref(), Some("board"));
        let titles: Vec<String> = vm
            .children
            .iter()
            .map(|l| l.prop_str("title").unwrap_or_default())
            .collect();
        assert_eq!(titles, vec!["BLOCKED", "DOING", "DONE", "No status"]);
        for lane in vm.children.iter() {
            assert!(
                !lane.children.is_empty(),
                "lane {:?} has no cards",
                lane.prop_str("title")
            );
            for card in lane.children.iter() {
                assert_eq!(card.widget_name().as_deref(), Some("card"));
            }
        }
    }

    #[test]
    fn board_streaming_path_creates_one_lane_per_distinct_value() {
        // Build a board over a `data_source` (streaming) instead of inline
        // rows. Each lane should be a `board_lane` VM with a `slot` whose
        // content is a `streaming_collection` of cards filtered to that
        // lane via `LaneFilteredProvider`.
        use holon_api::ReactiveRowProvider;

        use crate::value_fns::synthetic::SyntheticRows;

        crate::shadow_builders::register_render_dsl_widget_names();
        let src =
            r#"board(#{lane_field: "task_state", item_template: card(text(col("content")))})"#;
        let expr = holon_api::render_dsl::parse_render_dsl(src).expect("YAML render parses");

        let make_row = |id: &str, ts: Option<&str>, content: &str| -> Arc<DataRow> {
            let mut m: HashMap<String, Value> = HashMap::new();
            m.insert("id".into(), Value::String(id.into()));
            m.insert("content".into(), Value::String(content.into()));
            m.insert(
                "task_state".into(),
                match ts {
                    Some(s) => Value::String(s.into()),
                    None => Value::Null,
                },
            );
            m.insert("sort_key".into(), Value::String("A0".into()));
            Arc::new(m)
        };
        let rows: Vec<Arc<DataRow>> = vec![
            make_row("block:r1", Some("DOING"), "first doing"),
            make_row("block:r2", Some("DONE"), "first done"),
            make_row("block:r3", Some("DOING"), "second doing"),
        ];
        let provider: Arc<dyn ReactiveRowProvider> = Arc::new(SyntheticRows::from_rows(rows));

        let services = reactive::StubBuilderServices::new();
        let ctx = crate::RenderContext {
            data_source: Some(provider),
            ..Default::default()
        };
        let vm = services.interpret(&expr, &ctx);

        assert_eq!(vm.widget_name().as_deref(), Some("board"));
        // Streaming path: the board carries a `Grouped` collection view —
        // ONE driver subscribes to upstream and atomically rebuilds the
        // lane list per event. Verify the wiring is in place; actual lane
        // population requires a running tokio runtime to poll the driver
        // (covered end-to-end by the GPUI integration tests, not here).
        let view = vm
            .collection
            .as_ref()
            .expect("streaming board carries a Grouped collection view");
        assert_eq!(
            view.layout().as_ref().map(|l| l.name().to_string()),
            Some("board".to_string()),
            "view layout should be `board`"
        );
        // The board's `children` Vec is empty — children come from the
        // grouped driver writing into `view.items`.
        assert!(
            vm.children.is_empty(),
            "streaming board doesn't use static children"
        );
    }
}
