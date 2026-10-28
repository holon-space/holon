use super::*;

/// The user deletes a copy by hand, so every file stays in the body.
#[test]
fn a_block_in_two_files_names_every_file() {
    let detail = ConditionKind::BlockInTwoFiles {
        owner_file: "/vault/DayPage.org".into(),
        copy_files: vec!["/vault/Overview.org".into()],
    }
    .detail("block:bulk-0-0");
    assert!(
        detail.body.iter().any(|l| l.ends_with("Overview.org"))
            && detail.body.iter().any(|l| l.ends_with("DayPage.org")),
        "every file stays in the body: {detail:?}"
    );
}

fn previous_run(message: &str) -> ConditionDetail {
    ConditionKind::PreviousRunPanicked {
        message: message.to_string(),
        thread: "main".to_string(),
        earlier: crate::EarlierPanics {
            panics: Vec::new(),
            dropped: None,
        },
    }
    .detail("boot.rs:1:1")
}

#[test]
fn a_line_break_after_the_only_line_cuts_nothing() {
    for message in ["short message\n", "short message\r\n"] {
        let headline = previous_run(message).headline;
        assert!(!headline.contains('…'), "{message:?} -> {headline}");
    }
    let headline = previous_run("first line\nsecond line").headline;
    assert!(headline.ends_with("first line…"), "{headline}");
}

/// The doorbell is one line plus a way in: the records themselves are
/// read in the crash history.
#[test]
fn the_previous_run_doorbell_is_its_headline_and_opens_the_crash_history() {
    let kind = ConditionKind::PreviousRunPanicked {
        message: "run 2 broke".to_string(),
        thread: "main".to_string(),
        earlier: crate::EarlierPanics {
            panics: vec![crate::EarlierPanic {
                location: "a.rs:1:1".to_string(),
                message: "run 1 broke".to_string(),
            }],
            dropped: None,
        },
    };
    let detail = kind.detail("b.rs:2:2");
    assert!(
        detail.body.is_empty(),
        "the doorbell has no body: {detail:?}"
    );
    assert!(
        detail.headline.contains("b.rs:2:2") && detail.headline.contains("run 2 broke"),
        "the headline names the newest site and message: {}",
        detail.headline
    );
    assert!(
        kind.profile()
            .remedies()
            .contains(&crate::RemedySlot::OpenSettings(
                crate::SettingsSection::CrashHistory
            )),
        "the doorbell opens the crash history: {:?}",
        kind.profile().remedies()
    );
}

/// A window that cuts the headline short cuts its end, so the count and
/// the site come before the message.
#[test]
fn the_previous_run_headline_names_the_site_before_the_message() {
    let headline = previous_run("called `Result::unwrap()` on an `Err` value").headline;
    let site = headline.find("boot.rs:1:1").expect("the site is named");
    let message = headline.find("called").expect("the message is named");
    assert!(site < message, "{headline}");
}

fn needs_auth() -> ConditionKind {
    ConditionKind::IntegrationNeedsAuth {
        integration: "linear".to_string(),
        auth_url: "https://linear.app/oauth".to_string(),
    }
}

/// The in-session grant does not exist yet, so the text must not promise
/// one and must name the one thing the user can do.
#[test]
fn needs_auth_says_the_sign_in_cannot_complete_and_names_the_off_switch() {
    let detail = needs_auth().detail("linear");
    assert_eq!(
        detail.headline,
        "linear needs a sign-in that this build cannot complete yet. Switch the integration \
             off (Settings > Integrations) to remove this message."
    );
    assert!(detail.body.is_empty(), "{:?}", detail.body);
}

/// Keeps the text truthful: switching the integration off is the event
/// that clears the condition.
#[test]
fn switching_the_integration_off_clears_needs_auth() {
    assert_eq!(
        needs_auth().profile().all_clear(),
        crate::condition_profile::AllClear::NextSuccessOf(
            crate::condition_profile::ClearingEvent::IntegrationSwitchedOff
        )
    );
}
