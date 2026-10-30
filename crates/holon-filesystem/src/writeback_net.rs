//! The per-file write-back net (ADR 0032 §9.5). The subject is the
//! document's URI. The controller runs the effects; this module holds the
//! places, the transitions and the reads of the marking.
//!
//! - `OnDisk{hash}`: the file as `ReadDisk` last read it. A register
//!   (322/E3.b): the disk read and the catch-up replace the token. Every
//!   transition that rewrites it also reads it, so each rewrite is a
//!   compare-and-set against the token that firing read (322/E3.c).
//! - `Base{hash, authority}`: the text Holon last wrote or ingested, and the
//!   `Authority` hash that text holds. A register too, read by every leg that
//!   rewrites it.
//! - `Authority{hash}`: the hash of the store's render of the document, an
//!   outside write on every block change.
//! - `Refused{seen, authority}`: the file holds no content while the store
//!   holds the document. A sink, left by new file content or a manual fire.
//! - `Behind` (derived): the file or the store moved away from the base.
//!   `CatchUpIngest` ingests a file that moved; `CatchUpWrite` writes the
//!   render of a store that moved over a file at its base. Each keeps its own
//!   retry, so a write the disk does not take never holds back an ingest.

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
pub const CATCH_UP_INGEST: &str = "CatchUpIngest";
pub const CATCH_UP_WRITE: &str = "CatchUpWrite";
pub const CATCH_UP: [&str; 2] = [CATCH_UP_INGEST, CATCH_UP_WRITE];
pub const RESUME: &str = "Resume";
pub const ACCEPT_REMOVAL: &str = "AcceptRemoval";
pub const REWRITE_FROM_STORE: &str = "RewriteFromStore";

/// `OnDisk.hash` of a file that does not exist.
pub const ABSENT: &str = "absent";

/// `Base.authority` after an ingest: no `Authority` hash matches it, so the
/// next `CatchUpWrite` renders the store and writes or confirms the file.
pub const INGESTED: &str = "ingested";

/// `Authority.hash` and `Base.authority` of a document whose store has not
/// rendered since the net first saw it. A `Base` at `SEED` and an `Authority`
/// at `SEED` agree, so only the file can make it `Behind`.
pub const SEED: &str = "seed";

/// `Base.authority` after a read of the file the store refused: the next
/// `CatchUpWrite` tries the write, which the quarantine stops, so only new file
/// content fires it again.
pub const REFUSED_READ: &str = "refused-read";

/// A write the disk does not take is tried again after this long, so the
/// file is written within one backoff after the disk takes writes again.
pub const CATCH_UP_BACKOFF: Duration = Duration::from_millis(100);
/// The count bound of one catch-up run: ten minutes of retries. After it,
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

fn disk_moved(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    behind(r, s).filter(|c| field(c, "disk") != field(c, "base"))
}

fn store_moved(r: &mut Reader<'_>, s: &Subject) -> Option<Content> {
    behind(r, s).filter(|c| field(c, "disk") == field(c, "base"))
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
    net_with_backoff(CATCH_UP_BACKOFF)
}

/// [`net`] with `backoff` between the tries of a catch-up the disk does not
/// take, so a test can hold a retry pending for as long as it runs.
pub fn net_with_backoff(backoff: Duration) -> NetDef {
    NetDef::default()
        .place(DISK_CHANGED, Class::Process)
        .register(ON_DISK, Class::Process)
        .register(BASE, Class::Process)
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
            .retry(backoff, 3),
        )
        .transition(catch_up(CATCH_UP_INGEST, disk_moved, backoff))
        .transition(catch_up(CATCH_UP_WRITE, store_moved, backoff))
        .transition(
            TransitionDef::new(RESUME, Trigger::Auto, Arc::new(resume), Body::pure(resumed))
                .consumes(REFUSED)
                .reads(ON_DISK),
        )
        .transition(manual(ACCEPT_REMOVAL))
        .transition(manual(REWRITE_FROM_STORE))
}

fn catch_up(
    name: &str,
    guard: fn(&mut Reader<'_>, &Subject) -> Option<Content>,
    backoff: Duration,
) -> TransitionDef {
    TransitionDef::new(
        name,
        Trigger::Auto,
        Arc::new(guard),
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
    .retry(backoff, CATCH_UP_ATTEMPTS)
}

/// A remedy the user fires on a refusal. It reads `Base` and `OnDisk` as well
/// as consuming `Refused`, so its rewrite of both is a compare-and-set
/// against what it read: a disk change during the remedy wins over it
/// (322/E3.c).
fn manual(name: &str) -> TransitionDef {
    TransitionDef::new(
        name,
        Trigger::Manual,
        Arc::new(refused),
        Body::Effectful(Receiver::None),
    )
    .consumes(REFUSED)
    .reads(ON_DISK)
    .reads(BASE)
    .reads(AUTHORITY)
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

pub fn why_not(exec: &Executor, transition: &str, doc: &str) -> Option<holon_net::exec::WhyNot> {
    exec.why_not(transition, doc)
        .unwrap_or_else(|e| panic!("the write-back net holds its transitions: {e}"))
}

/// The documents whose file is not settled: a pending disk read, a running
/// or retrying catch-up, or `Behind` without a failure mark that stops it.
pub fn unsettled(exec: &Executor) -> Vec<String> {
    let mut docs: Vec<String> = exec
        .tokens()
        .subjects(BASE)
        .into_iter()
        .filter(|doc| {
            exec.tokens().token(DISK_CHANGED, doc).is_some()
                || why_not(exec, READ_DISK, doc).is_none()
                || CATCH_UP.iter().any(|t| {
                    matches!(
                        why_not(exec, t, doc),
                        None | Some(
                            holon_net::exec::WhyNot::InFlight
                                | holon_net::exec::WhyNot::RetryPending { .. }
                        )
                    )
                })
        })
        .map(|doc| doc.to_string())
        .collect();
    docs.sort();
    docs
}

/// Each document whose last catch-up failed on a disk that did not take it,
/// as `<doc> (<cause>)`: its file is behind the store.
pub fn failed_writebacks(exec: &Executor) -> Vec<String> {
    let marked = exec
        .marks()
        .filter(|(_, _, mark)| mark.cause.retryable)
        .map(|(t, doc, mark)| (t, doc, &mark.cause));
    let retrying = exec
        .retrying()
        .filter(|(t, doc, _)| exec.mark(t, doc).is_none());
    marked
        .chain(retrying)
        .filter(|(t, _, _)| CATCH_UP.contains(t))
        .map(|(_, doc, cause)| format!("{doc} ({})", cause.text))
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
    use holon_net::exec::Route;
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
        seeded_with(CATCH_UP_BACKOFF)
    }

    fn seeded_with(backoff: Duration) -> Net {
        let mut net = Net {
            exec: Executor::new(net_with_backoff(backoff)).expect("the write-back net loads"),
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
        assert_eq!(&*reqs[0].transition, CATCH_UP_WRITE);
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
            "the catch-up is inhibited by Refused"
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
        assert_eq!(&*reqs[0].transition, CATCH_UP_INGEST);
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
        assert_eq!(&*retry[0].transition, CATCH_UP_WRITE);
    }

    /// A shutdown that reads the failed list while a retry runs still finds
    /// the document whose write has not landed.
    #[test]
    fn a_write_whose_retry_is_running_is_still_named_failed() {
        let mut exec = seeded();
        exec.write(AUTHORITY, DOC, content([("hash", "a2")]));
        let reqs = one(&mut exec, 0);
        exec.complete(
            reqs[0].attempt,
            EffectResult::Failed(holon_net::exec::Cause::retryable("disk full")),
            Duration::from_millis(1),
        )
        .unwrap();
        let retry = one(&mut exec, 1 + CATCH_UP_BACKOFF.as_millis() as u64);
        assert_eq!(retry.len(), 1, "the retry is running");
        assert_eq!(
            failed_writebacks(&exec),
            vec!["doc:a (disk full)".to_string()]
        );
    }

    /// An ingest that failed on a long backoff, as each test below starts.
    fn failed_ingest() -> Net {
        let mut exec = seeded_with(Duration::from_secs(60));
        exec.write(ON_DISK, DOC, content([("hash", "h2")]));
        let reqs = one(&mut exec, 0);
        assert_eq!(&*reqs[0].transition, CATCH_UP_INGEST);
        exec.complete(
            reqs[0].attempt,
            EffectResult::Failed(holon_net::exec::Cause::retryable("store busy")),
            Duration::from_millis(1),
        )
        .unwrap();
        exec
    }

    /// Q6.a: new file content starts from zero tries, so it does not wait
    /// for the backoff of an ingest that failed on older content.
    #[test]
    fn new_file_content_is_ingested_before_the_backoff_of_a_failed_ingest_ends() {
        let mut exec = failed_ingest();
        exec.write(ON_DISK, DOC, content([("hash", "h3")]));
        let reqs = one(&mut exec, 2);
        assert_eq!(reqs.len(), 1, "the new file content fires at once");
        assert_eq!(&*reqs[0].transition, CATCH_UP_INGEST);
        assert_eq!(field(&reqs[0].input, "disk"), "h3");
    }

    #[test]
    fn the_same_file_content_waits_for_the_backoff_of_its_failed_ingest() {
        let mut exec = failed_ingest();
        exec.write(AUTHORITY, DOC, content([("hash", "a1")]));
        assert!(
            one(&mut exec, 59_000).is_empty(),
            "no retry before the backoff"
        );
        let retry = one(&mut exec, 60_001);
        assert_eq!(retry.len(), 1, "the retry fires when the backoff ends");
        assert_eq!(field(&retry[0].input, "disk"), "h2");
    }

    /// A write the disk does not take keeps its retry pending; a new file
    /// content is ingested at once, not after the write's backoff.
    #[test]
    fn a_pending_write_retry_does_not_hold_back_the_ingest_of_new_file_content() {
        let mut exec = seeded();
        exec.write(AUTHORITY, DOC, content([("hash", "a2")]));
        let reqs = one(&mut exec, 0);
        assert_eq!(&*reqs[0].transition, CATCH_UP_WRITE);
        exec.complete(
            reqs[0].attempt,
            EffectResult::Failed(holon_net::exec::Cause::retryable("disk full")),
            Duration::from_millis(1),
        )
        .unwrap();
        exec.write(ON_DISK, DOC, content([("hash", "h2")]));
        let reqs = one(&mut exec, 2);
        assert_eq!(reqs.len(), 1, "the new file content fires at once");
        assert_eq!(&*reqs[0].transition, CATCH_UP_INGEST);
    }

    /// 322/E3.b: a disk read replaces the `OnDisk` token, which the net
    /// declares a register. Declared `Single` instead, the fact refuses as a
    /// blind put and every disk read of changed content panics.
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

    /// 322/E3.b: the manual remedy rewrites `Base`, a register. Declared
    /// `Single` instead, the remedy panics on its own base fact and the
    /// user's repair action is dead.
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

    /// 322/E3.c: `ReadDisk` reads `OnDisk`, so its fact is a compare-and-set
    /// against the token it read. While the effect is in flight the file
    /// vanishes and `forget_file_state` writes `OnDisk{hash: absent}`; the
    /// newer token wins, the fact is withheld and the occurrence settles
    /// `OverChangedInputs`, so `CatchUpWrite` does not rewrite a file the user
    /// deleted.
    #[test]
    fn a_read_disk_fact_loses_to_the_absent_token_written_while_it_ran() {
        let mut exec = seeded();
        let doc = Subject::new(DOC);
        exec.write(DISK_CHANGED, DOC, Content::new());
        let reqs = one(&mut exec, 0);
        assert_eq!(&*reqs[0].transition, READ_DISK);
        exec.write(ON_DISK, DOC, content([("hash", ABSENT)]));
        assert_eq!(
            token_field(&exec, ON_DISK, DOC, "hash").as_deref(),
            Some(ABSENT),
            "the vanished file is the incumbent token"
        );
        exec.complete(
            reqs[0].attempt,
            EffectResult::Done(vec![output(ON_DISK, &doc, [("hash", "h2")])]),
            Duration::from_millis(1),
        )
        .unwrap();
        assert_eq!(
            token_field(&exec, ON_DISK, DOC, "hash").as_deref(),
            Some(ABSENT),
            "a fact must not drop the token written while its effect was in \
             flight"
        );
        let settled = exec
            .settled(&reqs[0].occurrence)
            .expect("the occurrence is known");
        let Some(Settled::OverChangedInputs { routes, .. }) = &settled else {
            panic!("the withheld fact is not disclosed: {settled:?}");
        };
        assert_eq!(
            routes
                .iter()
                .map(|r| (r.place.as_str().to_string(), r.route.clone()))
                .collect::<Vec<_>>(),
            vec![(ON_DISK.to_string(), Route::Withheld)],
            "the withheld fact is the disclosed route of the occurrence"
        );
        assert!(
            unsettled(&exec).contains(&DOC.to_string()),
            "the document is unsettled, so the net reconciles the deletion"
        );
    }

    /// 322/E3.c: a withheld fact is never lost. The agent writes the file
    /// while `CatchUpWrite` runs, so the ingest runs beside it: each leg's
    /// fact loses its compare-and-set to the row the other leg moved, and
    /// the level-triggered `Behind` fires `CatchUp` again until the file and
    /// the store hold the agent's text.
    #[test]
    fn a_withheld_fact_re_fires_catch_up_until_the_agent_text_settles() {
        let mut exec = seeded();
        let doc = Subject::new(DOC);
        let withheld = |exec: &Net, req: &EffectRequest| {
            let settled = exec
                .settled(&req.occurrence)
                .expect("the occurrence is known");
            let Some(Settled::OverChangedInputs { routes, .. }) = &settled else {
                panic!(
                    "{} settled without a withheld fact: {settled:?}",
                    req.transition
                );
            };
            routes
                .iter()
                .filter(|r| r.route == Route::Withheld)
                .map(|r| r.place.as_str().to_string())
                .collect::<Vec<_>>()
        };

        exec.write(AUTHORITY, DOC, content([("hash", "a2")]));
        let write = one(&mut exec, 0);
        assert_eq!(&*write[0].transition, CATCH_UP_WRITE);
        exec.write(DISK_CHANGED, DOC, Content::new());
        let read = one(&mut exec, 1);
        assert_eq!(&*read[0].transition, READ_DISK);
        exec.complete(
            read[0].attempt,
            EffectResult::Done(vec![output(ON_DISK, &doc, [("hash", "agent")])]),
            Duration::from_millis(2),
        )
        .unwrap();
        let ingest = one(&mut exec, 3);
        assert_eq!(&*ingest[0].transition, CATCH_UP_INGEST);

        exec.complete(
            write[0].attempt,
            EffectResult::Done(vec![
                output(BASE, &doc, [("hash", "h2"), ("authority", "a2")]),
                output(ON_DISK, &doc, [("hash", "h2")]),
            ]),
            Duration::from_millis(4),
        )
        .unwrap();
        assert_eq!(withheld(&exec, &write[0]), vec![ON_DISK.to_string()]);
        exec.complete(
            ingest[0].attempt,
            EffectResult::Done(vec![
                output(BASE, &doc, [("hash", "agent"), ("authority", INGESTED)]),
                output(ON_DISK, &doc, [("hash", "agent")]),
            ]),
            Duration::from_millis(5),
        )
        .unwrap();
        assert_eq!(withheld(&exec, &ingest[0]), vec![BASE.to_string()]);
        assert_eq!(
            token_field(&exec, ON_DISK, DOC, "hash").as_deref(),
            Some("agent"),
            "the agent's file content survives the write-back's fact"
        );

        let refire = one(&mut exec, 6);
        assert_eq!(
            refire.iter().map(|r| &*r.transition).collect::<Vec<_>>(),
            vec![CATCH_UP_INGEST],
            "the withheld base fact leaves the file Behind, so CatchUp fires again"
        );
        exec.complete(
            refire[0].attempt,
            EffectResult::Done(vec![
                output(BASE, &doc, [("hash", "agent"), ("authority", INGESTED)]),
                output(ON_DISK, &doc, [("hash", "agent")]),
            ]),
            Duration::from_millis(7),
        )
        .unwrap();
        exec.write(AUTHORITY, DOC, content([("hash", "a3")]));
        let confirm = one(&mut exec, 8);
        assert_eq!(&*confirm[0].transition, CATCH_UP_WRITE);
        exec.complete(
            confirm[0].attempt,
            EffectResult::Done(vec![
                output(BASE, &doc, [("hash", "agent"), ("authority", "a3")]),
                output(ON_DISK, &doc, [("hash", "agent")]),
            ]),
            Duration::from_millis(9),
        )
        .unwrap();

        assert!(one(&mut exec, 10).is_empty());
        assert!(unsettled(&exec).is_empty(), "the document is settled");
        assert!(!is_behind(&exec, DOC));
        assert_eq!(
            token_field(&exec, BASE, DOC, "hash").as_deref(),
            Some("agent"),
            "the base holds the agent's text"
        );
    }
}
