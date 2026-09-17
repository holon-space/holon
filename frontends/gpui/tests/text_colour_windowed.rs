//! A layout colour must reach the paint as the theme colour its token names.
//!
//! The GPUI frontend had five colour resolvers, each with its own string match
//! and its own catch-all. `text`'s knew `muted` / `warning` / `success` and
//! painted `foreground` for everything else, so the shipped
//! `icon(..., #{color: "primary"})` and `card(#{accent: "primary"})` in
//! `assets/default/types/block_profile.yaml` painted body foreground and an
//! unrelated grey: the author got a colour they never asked for and nothing
//! said so.
//!
//! A geometry-only assertion cannot see this. The rows are present, sized,
//! and readable; only the pixel is wrong. So this rung reads the colour the
//! paint actually landed on (`ElementInfo::painted_fg`, recorded by the GPUI
//! `text` builder through `with_painted_colors`) and compares it against the
//! ACTIVE theme.
//!
//! Two properties, both theme-relative:
//!
//! 1. Each token paints the `gpui_component` theme slot it names. The expected
//!    mapping is written out here rather than shared with the renderer, so a
//!    change on either side fails: the contract is "token X means slot Y", and
//!    a test that imported the renderer's own match would assert nothing.
//! 2. Distinct tokens paint distinct colours in ONE frame. `primary` against
//!    `foreground` is the pair the old catch-all conflated, so this is the
//!    assertion that goes red on the defect rather than on a theme change.
//!
//! Run: `cargo nextest run -p holon-gpui --features pbt --test
//! text_colour_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).
//!
//! @pbt kind windowed
//! @pbt covers theme-token-colour-reaches-the-paint — a `text(#{color: token})`
//!   paints the theme slot the token names, and two tokens paint two colours in
//!   one frame
//! @pbt slips-if-removed every resolver keeps its own catch-all, so
//!   `color: "primary"` paints body foreground while every headless rung and
//!   every geometry invariant stays green

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::Hsla;
use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_api::theme_token::ThemeToken;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::theme::Rgba8;
use holon_gpui::geometry::hsla_to_rgba8;
use support::BoundsSnapshot;
use support::render_fixture_sized;

/// One row-bound `text` carrying `token` as its colour.
///
/// Row-bound (an `id` plus a `field`) because that is what makes the widget
/// track itself into `BoundsRegistry` — a static label paints correctly but
/// records nothing, and this rung has nothing to read.
fn text_row(name: &str, token: ThemeToken) -> ReactiveViewModel {
    let mut props = HashMap::new();
    props.insert("content".to_string(), Value::String(name.to_string()));
    props.insert("field".to_string(), Value::String("content".to_string()));
    props.insert(
        "color".to_string(),
        Value::String(token.as_str().to_string()),
    );

    let mut vm = ReactiveViewModel::from_widget("text", props);
    let mut data = HashMap::new();
    data.insert("id".to_string(), Value::String(format!("block:{name}")));
    data.insert("content".to_string(), Value::String(name.to_string()));
    vm.data = Mutable::new(Arc::new(data)).read_only();
    vm
}

/// The element id the `text` builder mints for a tracked row of this name.
fn el_id(name: &str) -> String {
    format!("text-block:{name}-content")
}

fn frame(tokens: &[ThemeToken]) -> Arc<ReactiveViewModel> {
    let mut column = ReactiveViewModel::from_widget("column", HashMap::new());
    column.children = tokens
        .iter()
        .map(|token| Arc::new(text_row(token.as_str(), *token)))
        .collect();
    Arc::new(column)
}

/// The theme slot each token means. THE CONTRACT, spelled out.
fn expected_slot(cx: &mut TestAppContext, token: ThemeToken) -> Hsla {
    cx.update(|cx| {
        use gpui_component::theme::ActiveTheme;
        let t = &cx.theme().colors;
        match token {
            ThemeToken::Accent => t.accent,
            ThemeToken::Error => t.danger,
            ThemeToken::Foreground => t.foreground,
            ThemeToken::Info => t.info,
            ThemeToken::Muted | ThemeToken::Secondary => t.muted_foreground,
            ThemeToken::Primary => t.primary,
            ThemeToken::Success => t.success,
            ThemeToken::Warning => t.warning,
        }
    })
}

fn painted_fg(snap: &BoundsSnapshot, name: &str) -> Rgba8 {
    snap.entries
        .iter()
        .find(|(id, _)| id == &el_id(name))
        .map(|(_, info)| info)
        .unwrap_or_else(|| {
            panic!(
                "no tracked element for {name} (expected `{}`). Tracked: {:?}\n{}",
                el_id(name),
                snap.entries.iter().map(|(id, _)| id).collect::<Vec<_>>(),
                snap.dump()
            )
        })
        .painted_fg
        .unwrap_or_else(|| {
            panic!(
                "{name} recorded no painted foreground, so this rung cannot judge its colour. \
                 The `text` builder must declare what it paints via `with_painted_colors`."
            )
        })
}

/// Every token paints its own theme slot, in one frame.
#[gpui::test]
fn a_token_paints_the_theme_slot_it_names(cx: &mut TestAppContext) {
    let tokens = ThemeToken::ALL;
    let snap = render_fixture_sized(cx, frame(&tokens), size(px(900.0), px(600.0)));

    for token in tokens {
        let got = painted_fg(&snap, token.as_str());
        let want = hsla_to_rgba8(expected_slot(cx, token));
        assert_eq!(
            got,
            want,
            "`text(#{{color: {:?}}})` painted {got:?}, but that token means the theme's {:?} slot \
             ({want:?}). A resolver with a catch-all paints one colour for every name it does not \
             know, which is how `color: \"primary\"` came to paint body foreground.",
            token.as_str(),
            token,
        );
    }
}

/// The pair the old catch-all conflated, asserted against each other in a
/// single frame so a theme swap cannot make this pass or fail on its own.
#[gpui::test]
fn distinct_tokens_paint_distinct_colours_in_one_frame(cx: &mut TestAppContext) {
    let tokens = [
        ThemeToken::Primary,
        ThemeToken::Foreground,
        ThemeToken::Muted,
    ];
    let snap = render_fixture_sized(cx, frame(&tokens), size(px(900.0), px(600.0)));

    let primary = painted_fg(&snap, "primary");
    let foreground = painted_fg(&snap, "foreground");
    let muted = painted_fg(&snap, "muted");

    assert_ne!(
        primary, foreground,
        "`color: \"primary\"` and `color: \"foreground\"` painted the same colour ({primary:?}). \
         They are different theme slots, so one of them is being resolved by a catch-all."
    );
    assert_ne!(
        muted, foreground,
        "`color: \"muted\"` and `color: \"foreground\"` painted the same colour ({muted:?})."
    );
    assert_ne!(
        primary, muted,
        "`color: \"primary\"` and `color: \"muted\"` painted the same colour ({primary:?})."
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;

/// `accent` and `primary` read two DIFFERENT theme slots, and this pins that
/// they stay that way.
///
/// They can still paint the same pixel, and in production they do: the windowed
/// fixture runs on `gpui_component`'s default theme, while production calls
/// `apply_holon_theme` (`frontends/gpui/src/lib.rs`), which fills the `accent`
/// slot from holon's `primary` because that slot also colours the component
/// library's own chrome. Two names resolving to one pixel is therefore a
/// property of the THEME, not of the mapping: what this rung guards is that the
/// mapping keeps reading the two slots apart, so a theme that separates them
/// separates them here too.
#[gpui::test]
fn accent_and_primary_read_two_different_slots(cx: &mut TestAppContext) {
    let tokens = [ThemeToken::Accent, ThemeToken::Primary];
    let snap = render_fixture_sized(cx, frame(&tokens), size(px(900.0), px(600.0)));

    assert_ne!(
        painted_fg(&snap, "accent"),
        painted_fg(&snap, "primary"),
        "`accent` and `primary` painted one colour, so one of them is reading the other's slot. \
         In this theme the two slots differ; a mapping that conflates them is the defect this \
         rung exists to catch."
    );
}
