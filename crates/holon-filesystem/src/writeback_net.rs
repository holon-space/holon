//! The per-file write-back net (ADR 0032 §9.5). The subject is the
//! document's URI. The controller runs the effects; this module holds the
//! places, the transitions and the reads of the marking.
//!
//! - `OnDisk{hash}`: the file as `ReadDisk` last read it.
//! - `Base{hash, authority}`: the text Holon last wrote or ingested, and the
//!   `Authority` hash that text holds.
//! - `Authority{hash}`: the hash of the store's render of the document, an
//!   outside write on every block change.
//! - `Refused{seen, authority}`: the file holds no content while the store
//!   holds the document. A sink, left by new file content or a manual fire.
//! - `Behind` (derived): the file or the store moved away from the base.
//!   `CatchUp` ingests the file or writes the render until it is not.

use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_net::exec::Body;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::Executor;
use holon_net::exec::NetDef;
use holon_net::exec::Output;
use holon_net::exec::Reader;
use holon_net::exec::Receiver;
use holon_net::exec::Subject;
use holon_net::exec::TransitionDef;
use holon_net::exec::Trigger;

pub const DISK_CHANGED: &str = "DiskChanged";
pub const ON_DISK: &str = "OnDisk";
pub const BASE: &str = "Base";
pub const AUTHORITY: &str = "Authority";
pub const REFUSED: &str = "Refused";
pub const BEHIND: &str = "Behind";

pub const READ_DISK: &str = "ReadDisk";
pub const CATCH_UP: &str = "CatchUp";
pub const RESUME: &str = "Resume";
pub const ACCEPT_REMOVAL: &str = "AcceptRemoval";
pub const REWRITE_FROM_STORE: &str = "RewriteFromStore";

/// `OnDisk.hash` of a file that does not exist.
pub const ABSENT: &str = "absent";

/// `Base.authority` after an ingest: no `Authority` hash matches it, so the
/// next `CatchUp` renders the store and writes or confirms the file.
pub const INGESTED: &str = "ingested";

/// `Authority.hash` and `Base.authority` of a document whose store has not
/// rendered since the net first saw it. A `Base` at `SEED` and an `Authority`
/// at `SEED` agree, so only the file can make it `Behind`.
pub const SEED: &str = "seed";

/// `Base.authority` after a read of the file the store refused: the next
/// `CatchUp` tries the write, which the quarantine stops, so only new file
/// content fires it again.
pub const REFUSED_READ: &str = "refused-read";

/// A write the disk does not take is tried again after this long, so the
/// file is written within one backoff after the disk takes writes again.
pub const CATCH_UP_BACKOFF: Duration = Duration::from_millis(100);
/// The count bound of one `CatchUp` run: ten minutes of retries. After it,
/// only a new input (a block change, a new file content) fires it again.
pub const CATCH_UP_ATTEMPTS: u32 = 6000;

pub fn content<const N: usize>(fields: [(&str, &str); N]) -> Content {
    fields
        .into_iter()
        .map(|(k, v)| (k.to_string(), Value::String(v.to_string())))
        .collect()
}

pub fn output<const N: usize>(place: &str, doc: &Subject, fields: [(&str, &str); N]) -> Output {
    Output::new(place, doc, content(fields))
}

/// The field `field` of a token's content; panics when it is missing, as
/// every place of this net has fixed fields.
pub fn field<'c>(content: &'c Content, field: &str) -> &'c str {
    match content.get(field) {
        Some(Value::String(s)) => s,
        other => panic!("write-back net content has no text field '{field}': {other:?}"),
    }
}

fn behind(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    let disk = r.text(ON_DISK, s, "hash")?;
    let base = r.text(BASE, s, "hash")?;
    let base_authority = r.text(BASE, s, "authority")?;
    let authority = r.text(AUTHORITY, s, "hash")?;
    (disk != base || authority != base_authority).then(|| {
        content([
            ("disk", disk.as_str()),
            ("base", base.as_str()),
            ("base_authority", base_authority.as_str()),
            ("authority", authority.as_str()),
        ])
    })
}

fn disk_changed(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    r.present(DISK_CHANGED, s).then(Content::new)
}

fn resume(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    let seen = r.text(REFUSED, s, "seen")?;
    let disk = r.text(ON_DISK, s, "hash")?;
    (disk != seen).then(Content::new)
}

fn resumed(_: &Subject, _: &Content) -> Result<Vec<Output>, holon_net::exec::Cause> {
    Ok(Vec::new())
}

fn refused(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    let seen = r.text(REFUSED, s, "seen")?;
    let at_refusal = r.text(REFUSED, s, "authority")?;
    let authority = r.text(AUTHORITY, s, "hash")?;
    Some(content([
        ("seen", seen.as_str()),
        ("at_refusal", at_refusal.as_str()),
        ("authority", authority.as_str()),
    ]))
}

pub fn net() -> NetDef {
    NetDef::default()
        .place(DISK_CHANGED, Class::Process)
        .place(ON_DISK, Class::Process)
        .place(BASE, Class::Process)
        .place(AUTHORITY, Class::Data)
        .place(REFUSED, Class::Process)
        .derived(BEHIND, Arc::new(behind))
        .transition(
            TransitionDef::new(
                READ_DISK,
                Trigger::Auto,
                Arc::new(disk_changed),
                Body::Effectful(Receiver::Keyed),
            )
            .consumes(DISK_CHANGED)
            .reads(ON_DISK)
            .fact(ON_DISK)
            .retry(CATCH_UP_BACKOFF, 3),
        )
        .transition(
            TransitionDef::new(
                CATCH_UP,
                Trigger::Auto,
                Arc::new(behind),
                Body::Effectful(Receiver::Keyed),
            )
            .reads(ON_DISK)
            .reads(BASE)
            .reads(AUTHORITY)
            .inhibits(REFUSED)
            .inhibits(DISK_CHANGED)
            .fact(BASE)
            .fact(ON_DISK)
            .fact(REFUSED)
            .fact(DISK_CHANGED)
            .retry(CATCH_UP_BACKOFF, CATCH_UP_ATTEMPTS),
        )
        .transition(
            TransitionDef::new(RESUME, Trigger::Auto, Arc::new(resume), Body::pure(resumed))
                .consumes(REFUSED)
                .reads(ON_DISK),
        )
        .transition(manual(ACCEPT_REMOVAL))
        .transition(manual(REWRITE_FROM_STORE))
}

fn manual(name: &str) -> TransitionDef {
    TransitionDef::new(
        name,
        Trigger::Manual,
        Arc::new(refused),
        Body::Effectful(Receiver::None),
    )
    .consumes(REFUSED)
    .reads(ON_DISK)
    .reads(AUTHORITY)
    .reads(BASE)
    .fact(BASE)
    .fact(ON_DISK)
    .fact(DISK_CHANGED)
}

fn token_field(exec: &Executor, place: &str, doc: &str, field: &str) -> Option<String> {
    match exec.tokens().token(place, doc)?.content.get(field)? {
        Value::String(s) => Some(s.clone()),
        other => panic!("write-back net {place} of {doc}: field '{field}' is {other:?}"),
    }
}

/// Whether the file of `doc` is behind its store or its store behind the
/// file, by the same rule as the derived `Behind` place.
pub fn is_behind(exec: &Executor, doc: &str) -> bool {
    let read = |place, field| token_field(exec, place, doc, field);
    match (
        read(ON_DISK, "hash"),
        read(BASE, "hash"),
        read(BASE, "authority"),
        read(AUTHORITY, "hash"),
    ) {
        (Some(disk), Some(base), Some(base_authority), Some(authority)) => {
            disk != base || authority != base_authority
        }
        _ => false,
    }
}

/// The documents whose file is not settled: a pending disk read, a running
/// or retrying `CatchUp`, or `Behind` without a failure mark that stops it.
pub fn unsettled(exec: &Executor) -> Vec<String> {
    let mut docs: Vec<String> = exec
        .tokens()
        .subjects(BASE)
        .into_iter()
        .filter(|doc| {
            exec.tokens().token(DISK_CHANGED, doc).is_some()
                || exec.why_not(READ_DISK, doc).is_none()
                || matches!(
                    exec.why_not(CATCH_UP, doc),
                    None | Some(
                        holon_net::exec::WhyNot::InFlight
                            | holon_net::exec::WhyNot::RetryPending { .. }
                    )
                )
        })
        .map(|doc| doc.to_string())
        .collect();
    docs.sort();
    docs
}

/// Each document whose last `CatchUp` failed on a write the disk did not
/// take, as `<doc> (<cause>)`: its file is behind the store.
pub fn failed_writebacks(exec: &Executor) -> Vec<String> {
    exec.marks()
        .filter(|(t, _, mark)| *t == CATCH_UP && mark.cause.retryable)
        .map(|(_, doc, mark)| format!("{doc} ({})", mark.cause.text))
        .collect()
}

pub fn refused_docs(exec: &Executor) -> Vec<String> {
    exec.tokens()
        .subjects(REFUSED)
        .into_iter()
        .map(|d| d.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use holon_net::exec::EffectRequest;
    use holon_net::exec::EffectResult;
    use holon_net::exec::OccurrenceId;
    use holon_net::exec::Settled;

    use super::*;

    const DOC: &str = "doc:a";

    /// The executor with the effects each call quiesced into queued, as
    /// `holon_net_shell::Shell` queues them: [`one`] hands back every effect
    /// the net asked for since the last step.
    struct Net {
        exec: Executor,
        pending: Vec<EffectRequest>,
        now: Duration,
    }

    impl std::ops::Deref for Net {
        type Target = Executor;

        fn deref(&self) -> &Executor {
            &self.exec
        }
    }

    impl Net {
        fn write(&mut self, place: &str, subject: &str, content: Content) {
            let quiesced = self
                .exec
                .write(place, subject, content, self.now)
                .expect("the net terminates");
            self.pending.extend(quiesced.requests);
        }

        fn complete(
            &mut self,
            attempt: holon_net::exec::AttemptId,
            result: EffectResult,
            at: Duration,
        ) -> Result<(), holon_net::exec::ExecError> {
            self.now = at;
            let quiesced = self.exec.complete(attempt, result, at)?;
            self.pending.extend(quiesced.requests);
            Ok(())
        }
    }

    fn seeded() -> Net {
        let mut net = Net {
            exec: Executor::new(net()).expect("the write-back net loads"),
            pending: Vec::new(),
            now: Duration::ZERO,
        };
        net.write(ON_DISK, DOC, content([("hash", "h1")]));
        net.write(BASE, DOC, content([("hash", "h1"), ("authority", "a1")]));
        net.write(AUTHORITY, DOC, content([("hash", "a1")]));
        net
    }

    fn one(net: &mut Net, at: u64) -> Vec<EffectRequest> {
        net.now = Duration::from_millis(at);
        let new = net.exec.step(net.now).expect("the net terminates");
        let mut requests = std::mem::take(&mut net.pending);
        requests.extend(new);
        requests
    }

    #[test]
    fn an_in_sync_file_fires_nothing() {
        let mut exec = seeded();
        assert!(one(&mut exec, 0).is_empty());
        assert!(unsettled(&exec).is_empty());
    }

    /// Q3.a: the write lands, then a user edit reaches the store before the
    /// record. The base fact commits; the firing settles over changed inputs
    /// and the newer authority keeps the file Behind, so nothing is lost.
    #[test]
    fn an_edit_between_the_write_and_the_record_is_applied_over_changed_inputs() {
        let mut exec = seeded();
        exec.write(AUTHORITY, DOC, content([("hash", "a2")]));
        let reqs = one(&mut exec, 0);
        assert_eq!(reqs.len(), 1);
        assert_eq!(&*reqs[0].transition, CATCH_UP);
        exec.write(AUTHORITY, DOC, content([("hash", "a3")]));
        let doc = Subject::new(DOC);
        exec.complete(
            reqs[0].attempt,
            EffectResult::Done(vec![output(
                BASE,
                &doc,
                [("hash", "h2"), ("authority", "a2")],
            )]),
            Duration::from_millis(1),
        )
        .unwrap();
        assert!(matches!(
            exec.settled(&reqs[0].occurrence).unwrap(),
            Some(Settled::OverChangedInputs { .. })
        ));
        assert_eq!(token_field(&exec, BASE, DOC, "hash").as_deref(), Some("h2"));
        assert!(is_behind(&exec, DOC));
        assert_eq!(one(&mut exec, 2).len(), 1, "the newer edit is written next");
    }

    /// Q3.a: the store's write of the record's own result reaches the net as
    /// an `Authority` write with the hash the base already holds. It is no
    /// change, so it fires nothing.
    #[test]
    fn the_records_own_row_write_is_not_a_foreign_change() {
        let mut exec = seeded();
        exec.write(AUTHORITY, DOC, content([("hash", "a2")]));
        let reqs = one(&mut exec, 0);
        let doc = Subject::new(DOC);
        exec.complete(
            reqs[0].attempt,
            EffectResult::Done(vec![
                output(BASE, &doc, [("hash", "h2"), ("authority", "a2")]),
                output(ON_DISK, &doc, [("hash", "h2")]),
            ]),
            Duration::from_millis(1),
        )
        .unwrap();
        assert!(!is_behind(&exec, DOC));
        let version = exec.tokens().version(AUTHORITY, DOC);
        exec.write(AUTHORITY, DOC, content([("hash", "a2")]));
        assert_eq!(exec.tokens().version(AUTHORITY, DOC), version);
        assert!(one(&mut exec, 2).is_empty());
    }

    #[test]
    fn a_refused_file_waits_for_new_content_and_a_stale_disk_change_only_rereads() {
        let mut exec = seeded();
        let doc = Subject::new(DOC);
        exec.write(REFUSED, DOC, content([("seen", "h1"), ("authority", "a1")]));
        exec.write(AUTHORITY, DOC, content([("hash", "a2")]));
        assert!(
            one(&mut exec, 0).is_empty(),
            "CatchUp is inhibited by Refused"
        );
        exec.write(DISK_CHANGED, DOC, Content::new());
        let reqs = one(&mut exec, 1);
        assert_eq!(&*reqs[0].transition, READ_DISK);
        exec.complete(
            reqs[0].attempt,
            EffectResult::Done(vec![output(ON_DISK, &doc, [("hash", "h1")])]),
            Duration::from_millis(2),
        )
        .unwrap();
        assert!(
            one(&mut exec, 3).is_empty(),
            "the same content fires nothing"
        );
        exec.write(ON_DISK, DOC, content([("hash", "h3")]));
        let reqs = one(&mut exec, 4);
        assert!(refused_docs(&exec).is_empty(), "Resume consumed Refused");
        assert_eq!(&*reqs[0].transition, CATCH_UP);
    }

    #[test]
    fn a_write_the_disk_refuses_retries_after_the_backoff() {
        let mut exec = seeded();
        exec.write(AUTHORITY, DOC, content([("hash", "a2")]));
        let reqs = one(&mut exec, 0);
        exec.complete(
            reqs[0].attempt,
            EffectResult::Failed(holon_net::exec::Cause::retryable("disk full")),
            Duration::from_millis(1),
        )
        .unwrap();
        assert_eq!(
            failed_writebacks(&exec),
            vec!["doc:a (disk full)".to_string()]
        );
        assert!(one(&mut exec, 50).is_empty(), "no retry before the backoff");
        assert_eq!(unsettled(&exec), vec![DOC.to_string()]);
        let backoff = CATCH_UP_BACKOFF.as_millis() as u64;
        let retry = one(&mut exec, 1 + backoff);
        assert_eq!(retry.len(), 1);
        assert_eq!(&*retry[0].transition, CATCH_UP);
    }

    /// 322/E3.a2: a disk read replaces the `OnDisk` token it has read, so
    /// `ReadDisk` declares the read arc that makes the rewrite its own
    /// compare-and-set. Without the arc the fact refuses as a blind put.
    #[test]
    fn a_read_disk_whose_hash_differs_rewrites_on_disk() {
        let mut exec = seeded();
        let doc = Subject::new(DOC);
        exec.write(DISK_CHANGED, DOC, Content::new());
        let reqs = one(&mut exec, 0);
        assert_eq!(&*reqs[0].transition, READ_DISK);
        exec.complete(
            reqs[0].attempt,
            EffectResult::Done(vec![output(ON_DISK, &doc, [("hash", "h2")])]),
            Duration::from_millis(1),
        )
        .unwrap();
        assert_eq!(
            token_field(&exec, ON_DISK, DOC, "hash").as_deref(),
            Some("h2"),
            "the new disk content replaces the one ReadDisk read"
        );
        assert!(is_behind(&exec, DOC), "the file moved away from the base");
    }

    /// 322/E3.a2: the manual remedy rewrites the `Base` it has read, so
    /// `manual` declares the read arc on `Base`. Without the arc the remedy
    /// panics on its own base fact, and the user's repair action is dead.
    #[test]
    fn the_rewrite_from_store_remedy_rewrites_an_occupied_base() {
        let mut exec = seeded();
        let doc = Subject::new(DOC);
        exec.write(REFUSED, DOC, content([("seen", "h1"), ("authority", "a1")]));
        let quiesced = exec
            .exec
            .fire(
                REWRITE_FROM_STORE,
                DOC,
                OccurrenceId::caller("rewrite:doc:a:0"),
                Content::new(),
                Duration::from_millis(1),
            )
            .expect("the remedy is enabled for a refused file");
        let reqs = quiesced.requests;
        assert_eq!(&*reqs[0].transition, REWRITE_FROM_STORE);
        exec.complete(
            reqs[0].attempt,
            EffectResult::Done(vec![
                output(BASE, &doc, [("hash", "h2"), ("authority", "a1")]),
                output(ON_DISK, &doc, [("hash", "h2")]),
            ]),
            Duration::from_millis(2),
        )
        .unwrap();
        assert_eq!(
            token_field(&exec, BASE, DOC, "hash").as_deref(),
            Some("h2"),
            "the store's render replaces the base the remedy read"
        );
        assert!(
            refused_docs(&exec).is_empty(),
            "the remedy consumed Refused"
        );
        assert!(!is_behind(&exec, DOC));
    }
}
