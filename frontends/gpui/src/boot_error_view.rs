//! The screen a mobile boot shows in place of the app when the boot fails.
//! It uses nothing from the failed container, only the error itself.

use std::path::Path;

use gpui::*;
use holon_app::BootError;

/// The screen's text: the attributed report with its whole cause chain, and
/// the vault the boot was opening.
pub fn boot_error_screen_text(err: &BootError, vault: Option<&Path>) -> String {
    let vault = vault.map_or_else(|| "none".to_string(), |p| p.display().to_string());
    format!(
        "Holon could not start.\n\n{}\nvault: {vault}\n",
        err.structured_report()
    )
}

/// Puts `err` on screen through `show`. `exit` runs only when `show` fails:
/// with no screen left to disclose on, ending the process is the last resort.
pub fn show_boot_error(
    err: &BootError,
    vault: Option<&Path>,
    show: impl FnOnce(String) -> anyhow::Result<()>,
    exit: impl FnOnce(anyhow::Error),
) {
    if let Err(e) = show(boot_error_screen_text(err, vault)) {
        exit(e);
    }
}

/// Opens a window that shows `text`. `top_inset` keeps the text below the
/// status bar.
pub fn open_boot_error_window(cx: &mut App, text: String, top_inset: f32) -> anyhow::Result<()> {
    cx.open_window(WindowOptions::default(), move |_, cx| {
        cx.new(|_| BootErrorScreen { text, top_inset })
    })
    .map(|_| ())
}

struct BootErrorScreen {
    text: String,
    top_inset: f32,
}

impl Render for BootErrorScreen {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("boot-error")
            .size_full()
            .overflow_y_scroll()
            .bg(white())
            .text_color(black())
            .p_4()
            .pt(px(self.top_inset + 16.0))
            .child(self.text.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::path::PathBuf;

    use holon_app::BootComponent;
    use holon_app::BootError;
    use holon_app::BootStage;
    use proptest::prelude::*;

    use super::show_boot_error;

    fn any_component() -> impl Strategy<Value = BootComponent> {
        prop_oneof![
            Just(BootComponent::Config),
            Just(BootComponent::EpochGuard),
            Just(BootComponent::Turso),
            Just(BootComponent::Loro),
            Just(BootComponent::OrgSync),
            Just(BootComponent::Operations),
            Just(BootComponent::McpIntegrations),
            Just(BootComponent::Session),
            Just(BootComponent::Platform),
        ]
    }

    fn any_stage() -> impl Strategy<Value = BootStage> {
        prop_oneof![
            Just(BootStage::ConfigLoad),
            Just(BootStage::EpochGuard),
            Just(BootStage::ContainerConfigure),
            Just(BootStage::EngineResolve),
            Just(BootStage::SessionResolve),
        ]
    }

    /// A boot error whose cause chain is `links`, outermost first.
    fn boot_error(component: BootComponent, stage: BootStage, links: &[String]) -> BootError {
        let (root, contexts) = links.split_last().expect("a chain has a root cause");
        let source = contexts
            .iter()
            .rev()
            .fold(anyhow::anyhow!("{root}"), |e, c| e.context(c.clone()));
        BootError::new(component, stage, source)
    }

    /// The parts of a boot error: `BootError` is not `Clone`, so each case
    /// builds its own.
    fn any_boot_error_parts() -> impl Strategy<Value = (BootComponent, BootStage, Vec<String>)> {
        (
            any_component(),
            any_stage(),
            prop::collection::vec("[^\n]{1,40}", 1..5),
        )
    }

    fn any_vault() -> impl Strategy<Value = Option<PathBuf>> {
        prop::option::of("/[a-z0-9/._-]{1,40}".prop_map(PathBuf::from))
    }

    /// Any boot error reaches the screen with its attribution, every link of
    /// its cause chain and the vault, and the process does not end.
    #[test]
    fn every_boot_error_is_shown_and_the_process_lives() {
        let mut runner = proptest::test_runner::TestRunner::default();
        runner
            .run(
                &(any_boot_error_parts(), any_vault()),
                |((component, stage, links), vault)| {
                    let err = boot_error(component, stage, &links);
                    let mut shown = None;
                    let exited = Cell::new(false);
                    show_boot_error(
                        &err,
                        vault.as_deref(),
                        |text| {
                            shown = Some(text);
                            Ok(())
                        },
                        |_| exited.set(true),
                    );
                    prop_assert!(!exited.get(), "a shown boot error must not end the process");
                    let text = shown.expect("the boot error reaches the screen");
                    prop_assert!(text.contains(&format!("component={component}")), "{text}");
                    prop_assert!(text.contains(&format!("stage={stage}")), "{text}");
                    for link in &links {
                        prop_assert!(text.contains(link.as_str()), "missing {link:?} in {text}");
                    }
                    if let Some(vault) = &vault {
                        prop_assert!(text.contains(&vault.display().to_string()), "{text}");
                    }
                    Ok(())
                },
            )
            .unwrap();
    }

    /// Only a screen that fails to open ends the process, with that failure
    /// as the reason.
    #[test]
    fn a_screen_that_fails_to_open_ends_the_process() {
        let mut runner = proptest::test_runner::TestRunner::default();
        runner
            .run(
                &(any_boot_error_parts(), any_vault(), "[^\n]{1,40}"),
                |((component, stage, links), vault, reason)| {
                    let err = boot_error(component, stage, &links);
                    let mut exit_reason = None;
                    show_boot_error(
                        &err,
                        vault.as_deref(),
                        |_| Err(anyhow::anyhow!("{reason}")),
                        |e| exit_reason = Some(e.to_string()),
                    );
                    prop_assert_eq!(exit_reason, Some(reason));
                    Ok(())
                },
            )
            .unwrap();
    }
}
