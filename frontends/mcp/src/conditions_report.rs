//! What the app is currently disclosing, as text an agent can read.
//!
//! The `conditions_list` MCP tool's whole body, factored out so it is testable
//! without standing up a server: the tool resolves the bus and calls
//! [`report`].
//!
//! Read-only by construction — it takes a snapshot and formats it. Raising or
//! clearing a condition is the app's business, never an agent's.

use holon_api::ConditionBus;
use holon_api::condition_source::placement_name;
use holon_api::condition_source::severity_name;

/// The report for a wired bus, or the reason there is none.
///
/// A missing bus reports itself rather than returning an empty list. The two
/// are opposite facts — "nothing is degraded" and "nobody can tell you whether
/// anything is degraded" — and an agent that cannot distinguish them will read
/// a mute harness as a healthy app.
pub fn report(bus: Option<&ConditionBus>) -> String {
    let Some(bus) = bus else {
        return "[no condition bus is wired into this server — it cannot tell you whether \
                anything is degraded. This is a headless/unwired run, NOT an all-clear.]"
            .to_string();
    };

    let conditions = bus.current();
    if conditions.is_empty() {
        return format!(
            "No conditions in effect. ({} subscriber(s) listening.)",
            bus.subscriber_count()
        );
    }

    let mut out = format!(
        "{} condition(s) in effect, oldest first. ({} subscriber(s) listening.)\n",
        conditions.len(),
        bus.subscriber_count()
    );
    for condition in &conditions {
        let profile = condition.reason.profile();
        let detail = condition.reason.detail(&condition.subject);
        out.push_str(&format!(
            "\n- [{}] {} — {}\n  subject: {}\n  kind: {}\n  placement: {}\n",
            severity_name(profile.severity()),
            profile.label(),
            detail.headline,
            condition.subject,
            condition.reason.condition_kind(),
            placement_name(profile.placement()),
        ));
        for line in &detail.body {
            out.push_str(&format!("  | {line}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use holon_api::Condition;
    use holon_api::ConditionKind;

    use super::*;

    fn ingest_failure(subject: &str) -> Condition {
        Condition {
            subject: subject.to_string(),
            reason: ConditionKind::VaultIngestFailed {
                format: "org".to_string(),
                reason: "unreadable".to_string(),
            },
        }
    }

    #[test]
    fn an_unwired_server_says_so_instead_of_reporting_all_clear() {
        let text = report(None);
        assert!(text.contains("no condition bus is wired"), "text={text}");
        assert!(
            text.contains("NOT an all-clear"),
            "an agent must not read a mute harness as a healthy app: text={text}"
        );
    }

    #[test]
    fn an_empty_bus_is_distinguishable_from_an_unwired_one() {
        let text = report(Some(&ConditionBus::new()));
        assert!(text.starts_with("No conditions in effect."), "text={text}");
    }

    #[test]
    fn a_raised_condition_reads_back_with_its_profile_and_detail() {
        let bus = ConditionBus::new();
        bus.emit(ingest_failure("notes.org"));

        let text = report(Some(&bus));
        assert!(text.contains("[error]"), "severity: text={text}");
        assert!(
            text.contains("File sync degraded (bad vault file)"),
            "the profile's label: text={text}"
        );
        assert!(
            text.contains("notes.org: unreadable"),
            "the instance detail: text={text}"
        );
        assert!(
            text.contains("kind: vault-ingest-failed"),
            "the stable kind: text={text}"
        );
        assert!(text.contains("placement: toast"), "text={text}");
    }

    #[test]
    fn body_lines_survive_whole() {
        let bus = ConditionBus::new();
        bus.emit(Condition {
            subject: "todoist".to_string(),
            reason: ConditionKind::IntegrationNeedsAuth {
                integration: "todoist".to_string(),
                auth_url: "https://example.invalid/authorize?token=abcdef".to_string(),
            },
        });

        let text = report(Some(&bus));
        assert!(
            text.contains("| https://example.invalid/authorize?token=abcdef"),
            "a URL the user must open is never truncated: text={text}"
        );
    }

    #[test]
    fn a_cleared_condition_leaves_the_report() {
        let bus = ConditionBus::new();
        let condition = ingest_failure("notes.org");
        bus.emit(condition.clone());
        bus.clear(&condition.condition_key());

        let text = report(Some(&bus));
        assert!(text.starts_with("No conditions in effect."), "text={text}");
    }

    #[test]
    fn the_report_lists_conditions_in_raise_order() {
        let bus = ConditionBus::new();
        bus.emit(ingest_failure("zeta.org"));
        bus.emit(ingest_failure("alpha.org"));

        let text = report(Some(&bus));
        let zeta = text.find("zeta.org").expect("zeta is listed");
        let alpha = text.find("alpha.org").expect("alpha is listed");
        assert!(
            zeta < alpha,
            "oldest first, not alphabetical by subject: text={text}"
        );
    }
}
