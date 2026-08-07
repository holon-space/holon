//! What SEVERITY the degradation ladder discloses at, and under which rung
//! name.
//!
//! The rungs that keep every content byte are a disclosed degraded mode, not a
//! failure: emitting ERROR for them cost a fresh root-cause per sighting and
//! armed a false red in the keystone, whose `inv-no-observed-errors` reads any
//! ERROR as a swallowed problem. The rung that cannot preserve the bytes keeps
//! ERROR — that one IS a failure.
//!
//! The companion locks in `render_marks_fixed_point_pbt.rs` pin the rung and
//! the bytes; this file pins only how loudly each rung speaks.

use std::sync::Arc;
use std::sync::Mutex;

use holon_api::EntityRef;
use holon_api::EntityUri;
use holon_api::InlineMark;
use holon_api::MarkSpan;
use holon_api::block::Block;
use holon_org_format::RenderFidelity;
use holon_org_format::render_block_content_checked;
use tracing::Level;
use tracing::field::Field;
use tracing::field::Visit;
use tracing_subscriber::layer::Context;
use tracing_subscriber::layer::Layer;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::registry::Registry;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Captured {
    level: Level,
    rung: Option<String>,
}

#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<Captured>>>);

struct RungVisitor(Option<String>);

impl Visit for RungVisitor {
    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "rung" {
            self.0 = Some(value.to_string());
        }
    }
}

impl<S: tracing::Subscriber> Layer<S> for Capture {
    fn on_event(&self, event: &tracing::Event<'_>, _: Context<'_, S>) {
        let mut visitor = RungVisitor(None);
        event.record(&mut visitor);
        self.0.lock().expect("capture poisoned").push(Captured {
            level: *event.metadata().level(),
            rung: visitor.0,
        });
    }
}

/// Render `content` + `marks` under a capturing subscriber and return the rung
/// the ladder landed on together with every event it emitted.
///
/// `block_id` must be unique per call: the disclosure is loud once per
/// (block, reason) per process and silent after, so a reused id would capture
/// the DEBUG repeat instead of the first report.
fn render_capturing(
    block_id: &str,
    content: &str,
    marks: &[MarkSpan],
) -> (RenderFidelity, Vec<Captured>) {
    let capture = Capture::default();
    let subscriber = Registry::default().with(capture.clone());
    let mut block = Block::new_text(
        EntityUri::block(block_id),
        EntityUri::block("parent"),
        content,
    );
    block.marks = Some(marks.to_vec());
    let fidelity =
        tracing::subscriber::with_default(subscriber, || render_block_content_checked(&block).1);
    let events = capture.0.lock().expect("capture poisoned").clone();
    (fidelity, events)
}

fn span(start: usize, end: usize, mark: InlineMark) -> MarkSpan {
    MarkSpan { start, end, mark }
}

fn link(start: usize, end: usize) -> MarkSpan {
    MarkSpan {
        start,
        end,
        mark: InlineMark::Link {
            target: EntityRef::External {
                url: "https://example.com".to_string(),
            },
            label: String::new(),
        },
    }
}

/// The `#24` shape: a `Code` mark between literal `~` delimiters. Every content
/// byte survives, so this must WARN — an ERROR here is the escape that was
/// root-caused four times and would red the keystone for correct behaviour.
#[test]
fn the_protective_rung_warns_and_names_itself() {
    let (fidelity, events) = render_capturing(
        "severity-protective",
        "nested a ~b~ c",
        &[span(10, 11, InlineMark::Code)],
    );

    assert_eq!(fidelity, RenderFidelity::ProtectiveDropped);
    assert_eq!(
        events,
        vec![Captured {
            level: Level::WARN,
            rung: Some("ProtectiveDropped".to_string()),
        }],
        "the content-preserving rung must disclose at WARN and name its rung"
    );
}

/// The rung above it, same requirement: nothing protective was given up, so the
/// disclosure is even less of an emergency.
#[test]
fn the_styling_rung_warns_and_names_itself() {
    let (fidelity, events) = render_capturing(
        "severity-styling",
        "nested a ~b~ c",
        &[span(10, 11, InlineMark::Bold)],
    );

    assert_eq!(fidelity, RenderFidelity::StylingDropped);
    assert_eq!(
        events,
        vec![Captured {
            level: Level::WARN,
            rung: Some("StylingDropped".to_string()),
        }]
    );
}

/// The other half of the teeth. A `Link` mark over content that ALREADY holds
/// raw link syntax falls through every rung of pass 1: org has no nested
/// `[[…]]`, and dropping the mark does not rescue it either, because the raw
/// syntax left in the content is itself adopted as a link on re-parse. The
/// bytes on disk therefore no longer re-parse to what the store holds, which is
/// a failure and not a degraded mode, so it stays ERROR. Without this row the
/// severity mapping could be satisfied by demoting everything.
#[test]
fn the_content_unpreserved_rung_still_errors_and_names_itself() {
    let (fidelity, events) =
        render_capturing("severity-unpreserved", "see [[a][b]] here", &[link(0, 17)]);

    assert_eq!(
        fidelity,
        RenderFidelity::ContentUnpreserved,
        "this shape must reach pass 2, otherwise the ERROR arm is untested"
    );
    assert_eq!(
        events,
        vec![Captured {
            level: Level::ERROR,
            rung: Some("Unrepresentable".to_string()),
        }],
        "a rung that loses content bytes must stay ERROR"
    );
}

/// An exact render says nothing at all — the disclosure is not chatter.
#[test]
fn an_exact_render_discloses_nothing() {
    let (fidelity, events) = render_capturing(
        "severity-exact",
        "lit ~x~ and b",
        &[span(12, 13, InlineMark::Code)],
    );

    assert_eq!(fidelity, RenderFidelity::Exact);
    assert!(events.is_empty(), "unexpected disclosure: {events:?}");
}
