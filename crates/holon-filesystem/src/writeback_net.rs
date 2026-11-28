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
    holon_net::exec::new_net()
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
    match exec.tokens().token(place, doc)?.record().get(field)? {
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

/// Why the file of a document and its store do not agree yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotConverged {
    /// The net holds no `OnDisk`, `Base` or `Authority` token for it.
    TokensMissing,
    /// A disk read is owed, running or waiting for its retry.
    ReadingDisk,
    /// The disk read failed for good: only a new change of the file reads
    /// it again.
    ReadFailed,
    /// This catch-up is enabled, running or waiting for its retry.
    CatchingUp(&'static str),
    /// The file or the store moved away from the base, and no catch-up can
    /// land without new input.
    Behind,
}

/// Whether the file of `doc`, its base and the store's render agree and the
/// net owes `doc` no work. A pure read of the marking. Owed work is checked
/// first, so no other reason hides it.
pub fn converged(exec: &Executor, doc: &str) -> Result<(), NotConverged> {
    if owed(exec, READ_DISK, doc) {
        return Err(NotConverged::ReadingDisk);
    }
    if let Some(t) = CATCH_UP.into_iter().find(|t| owed(exec, t, doc)) {
        return Err(NotConverged::CatchingUp(t));
    }
    let present = |place| exec.tokens().token(place, doc).is_some();
    if ![ON_DISK, BASE, AUTHORITY].into_iter().all(present) {
        return Err(NotConverged::TokensMissing);
    }
    if exec.tokens().token(DISK_CHANGED, doc).is_some() {
        return Err(NotConverged::ReadFailed);
    }
    if is_behind(exec, doc) {
        return Err(NotConverged::Behind);
    }
    Ok(())
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
            matches!(
                converged(exec, doc),
                Err(NotConverged::ReadingDisk | NotConverged::CatchingUp(_))
            )
        })
        .map(|doc| doc.to_string())
        .collect();
    docs.sort();
    docs
}

/// The documents a shutdown waits for: the [`unsettled`] ones, except where
/// the only catch-up that can still land is an ingest whose run already
/// failed. An ingest writes no file, so the file keeps the text the next boot
/// ingests, and [`failed_writebacks`] names the document.
pub fn stop_waits_for(exec: &Executor) -> Vec<String> {
    unsettled(exec)
        .into_iter()
        .filter(|doc| {
            let ingest_failed = exec
                .retrying()
                .any(|(t, d, _)| t == CATCH_UP_INGEST && d.as_str() == doc)
                || matches!(
                    why_not(exec, CATCH_UP_INGEST, doc),
                    Some(
                        holon_net::exec::WhyNot::RetryPending { .. }
                            | holon_net::exec::WhyNot::FailedOnThisInput(_)
                    )
                );
            !ingest_failed || owed(exec, READ_DISK, doc) || write_may_land(exec, doc)
        })
        .collect()
}

/// When the earliest pending retry of a disk read or a catch-up of `doc`
/// comes due.
pub fn retry_due(exec: &Executor, doc: &str) -> Option<Duration> {
    std::iter::once(READ_DISK)
        .chain(CATCH_UP)
        .filter_map(|t| match why_not(exec, t, doc) {
            Some(holon_net::exec::WhyNot::RetryPending { due }) => Some(due),
            _ => None,
        })
        .min()
}

/// The transition is enabled, running or waiting for its retry on `doc`.
fn owed(exec: &Executor, transition: &str, doc: &str) -> bool {
    matches!(
        why_not(exec, transition, doc),
        None | Some(
            holon_net::exec::WhyNot::InFlight | holon_net::exec::WhyNot::RetryPending { .. }
        )
    )
}

/// A pending write retry fires only over a file at its base.
fn write_may_land(exec: &Executor, doc: &str) -> bool {
    use holon_net::exec::WhyNot;
    match why_not(exec, CATCH_UP_WRITE, doc) {
        None | Some(WhyNot::InFlight) => true,
        Some(WhyNot::RetryPending { .. }) => {
            token_field(exec, ON_DISK, doc, "hash") == token_field(exec, BASE, doc, "hash")
        }
        Some(
            WhyNot::Needs { .. }
            | WhyNot::Inhibited(_)
            | WhyNot::Guard
            | WhyNot::FailedOnThisInput(_)
            | WhyNot::ReadsUndeclared(_)
            | WhyNot::DerivedCycle(_)
            | WhyNot::BindsAnswerInput,
        ) => false,
    }
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
            unsettled(&exec).iter().any(|i| i == DOC),
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

    mod stop {
        use std::collections::BTreeMap;

        use holon_net::exec::Cause;
        use proptest::prelude::*;

        use super::*;

        pub(super) const DOCS: [&str; 2] = ["doc:a", "doc:b"];
        /// The tries of one disk read; after them it fails for good.
        pub(super) const READ_TRIES: u32 = 3;

        #[derive(Debug, Clone, Copy)]
        pub(super) enum Outcome {
            Done,
            Retryable,
            Definite,
        }

        #[derive(Debug, Clone)]
        pub(super) enum Step {
            StoreMoves(usize),
            DiskMoves(usize),
            /// The file goes back to the text of its base.
            DiskReverts(usize),
            /// Completes the outstanding effect at this index modulo their
            /// count.
            Complete(usize, Outcome),
            Advance(u64),
            /// The store goes back to the authority of its base.
            StoreReverts(usize),
            /// The file changes and the watcher reports it; the net reads it.
            FileChanges(usize),
        }

        pub(super) fn step() -> impl Strategy<Value = Step> {
            let outcome = prop_oneof![
                Just(Outcome::Done),
                Just(Outcome::Retryable),
                Just(Outcome::Definite)
            ];
            prop_oneof![
                1 => (0..DOCS.len()).prop_map(Step::StoreMoves),
                1 => (0..DOCS.len()).prop_map(Step::DiskMoves),
                1 => (0..DOCS.len()).prop_map(Step::DiskReverts),
                3 => (any::<usize>(), outcome).prop_map(|(i, o)| Step::Complete(i, o)),
                2 => (1u64..250).prop_map(Step::Advance),
                1 => (0..DOCS.len()).prop_map(Step::StoreReverts),
                1 => (0..DOCS.len()).prop_map(Step::FileChanges),
            ]
        }

        /// What the test itself saw of each document's catch-up, apart from
        /// the net's own reads.
        #[derive(Debug, Default)]
        struct Seen {
            /// An ingest attempt failed and no ingest landed since.
            ingest_failed: bool,
            /// The last write attempt failed retryably and no write ran since.
            write_retry_pending: bool,
            /// The failed tries of the disk read of the last reported change;
            /// at [`READ_TRIES`] its `DiskChanged` stays and inhibits every
            /// catch-up until the file changes again.
            read_failures: u32,
        }

        struct Model {
            net: Net,
            outstanding: Vec<EffectRequest>,
            seen: BTreeMap<&'static str, Seen>,
            counter: u32,
        }

        impl Model {
            fn new() -> Self {
                let mut net = Net {
                    exec: Executor::new(net()).expect("the write-back net loads"),
                    pending: Vec::new(),
                    now: Duration::ZERO,
                };
                for doc in DOCS {
                    net.write(ON_DISK, doc, content([("hash", "h0")]));
                    net.write(BASE, doc, content([("hash", "h0"), ("authority", "a0")]));
                    net.write(AUTHORITY, doc, content([("hash", "a0")]));
                }
                Self {
                    net,
                    outstanding: Vec::new(),
                    seen: DOCS.iter().map(|d| (*d, Seen::default())).collect(),
                    counter: 0,
                }
            }

            fn fresh(&mut self, prefix: &str) -> String {
                self.counter += 1;
                format!("{prefix}{}", self.counter)
            }

            fn collect(&mut self, requests: Vec<EffectRequest>) {
                self.outstanding.extend(requests);
            }

            fn running(&self, transition: &str, doc: &str) -> bool {
                self.outstanding
                    .iter()
                    .any(|r| &*r.transition == transition && r.subject.as_str() == doc)
            }

            fn apply(&mut self, step: &Step) {
                let at = self.net.now.as_millis() as u64;
                match *step {
                    Step::StoreMoves(d) => {
                        let hash = self.fresh("a");
                        self.net
                            .write(AUTHORITY, DOCS[d], content([("hash", &hash)]));
                    }
                    Step::DiskReverts(d) => {
                        let base = token_field(&self.net, BASE, DOCS[d], "hash")
                            .expect("every document has a base");
                        self.net.write(ON_DISK, DOCS[d], content([("hash", &base)]));
                    }
                    Step::DiskMoves(d) => {
                        let hash = self.fresh("h");
                        self.net.write(ON_DISK, DOCS[d], content([("hash", &hash)]));
                    }
                    Step::StoreReverts(d) => {
                        let base = token_field(&self.net, BASE, DOCS[d], "authority")
                            .expect("every document has a base");
                        self.net
                            .write(AUTHORITY, DOCS[d], content([("hash", &base)]));
                    }
                    Step::FileChanges(d) => {
                        // A catch-up may still run when the watcher reports;
                        // a running read already took the last report.
                        if self.running(READ_DISK, DOCS[d]) {
                            return;
                        }
                        // shortcut: a report while `DiskChanged` is still held
                        // is not new input to the net, so a read that failed
                        // for good stays failed; skipped until a disk-change
                        // report is a new level.
                        if self.net.tokens().token(DISK_CHANGED, DOCS[d]).is_some() {
                            return;
                        }
                        self.net.write(DISK_CHANGED, DOCS[d], Content::new());
                    }
                    Step::Complete(i, outcome) => {
                        if self.outstanding.is_empty() {
                            return;
                        }
                        let req = self.outstanding.remove(i % self.outstanding.len());
                        let doc = DOCS[doc_index(&req)];
                        let subject = Subject::new(doc);
                        let read = self.fresh("h");
                        let seen = self.seen.get_mut(doc).unwrap();
                        if &*req.transition == READ_DISK {
                            seen.read_failures = match outcome {
                                Outcome::Done => 0,
                                Outcome::Retryable => seen.read_failures + 1,
                                Outcome::Definite => READ_TRIES,
                            };
                        }
                        let result = match (outcome, &*req.transition) {
                            (Outcome::Done, CATCH_UP_INGEST) => {
                                seen.ingest_failed = false;
                                let disk = field(&req.input, "disk");
                                EffectResult::Done(vec![
                                    output(
                                        BASE,
                                        &subject,
                                        [("hash", disk), ("authority", INGESTED)],
                                    ),
                                    output(ON_DISK, &subject, [("hash", disk)]),
                                ])
                            }
                            (Outcome::Done, CATCH_UP_WRITE) => {
                                seen.write_retry_pending = false;
                                let hash = format!("w-{}", field(&req.input, "authority"));
                                EffectResult::Done(vec![
                                    output(
                                        BASE,
                                        &subject,
                                        [
                                            ("hash", &hash),
                                            ("authority", field(&req.input, "authority")),
                                        ],
                                    ),
                                    output(ON_DISK, &subject, [("hash", &hash)]),
                                ])
                            }
                            (Outcome::Retryable | Outcome::Definite, t) => {
                                if t == CATCH_UP_INGEST {
                                    seen.ingest_failed = true;
                                } else if t == CATCH_UP_WRITE {
                                    seen.write_retry_pending =
                                        matches!(outcome, Outcome::Retryable);
                                }
                                EffectResult::Failed(match outcome {
                                    Outcome::Retryable => Cause::retryable("disk busy"),
                                    _ => Cause::definite("no such document"),
                                })
                            }
                            (Outcome::Done, READ_DISK) => EffectResult::Done(vec![output(
                                ON_DISK,
                                &subject,
                                [("hash", &read)],
                            )]),
                            (_, other) => panic!("the generator fires no {other}"),
                        };
                        self.net
                            .complete(req.attempt, result, Duration::from_millis(at + 1))
                            .expect("the attempt is outstanding");
                    }
                    Step::Advance(ms) => {
                        let requests = one(&mut self.net, at + ms);
                        self.collect(requests);
                        return;
                    }
                }
                let now = self.net.now.as_millis() as u64;
                let requests = one(&mut self.net, now);
                self.collect(requests);
            }

            fn check(&self) -> Result<(), TestCaseError> {
                running_catch_ups_stay_unsettled(&self.net)?;
                let unsettled = unsettled(&self.net);
                let stop = stop_waits_for(&self.net);
                let failed = failed_writebacks(&self.net);
                for doc in DOCS {
                    let seen = &self.seen[doc];
                    let ingest_failed = seen.ingest_failed;
                    let ingest_running = self.running(CATCH_UP_INGEST, doc);
                    let write_running = self.running(CATCH_UP_WRITE, doc);
                    // A write retry re-checks its guard when it comes due: the
                    // disk at its base and the store away from the base's
                    // authority.
                    let read = |place, field| token_field(&self.net, place, doc, field);
                    let write_can_land = write_running
                        || (seen.write_retry_pending
                            && seen.read_failures < READ_TRIES
                            && read(ON_DISK, "hash") == read(BASE, "hash")
                            && read(AUTHORITY, "hash") != read(BASE, "authority"));
                    let owed = unsettled.iter().any(|d| d == doc);
                    let waited = stop.iter().any(|d| d == doc);
                    prop_assert!(!waited || owed, "{doc}: the stop waits only for owed work");
                    if owed && (write_can_land || !ingest_failed) {
                        prop_assert!(
                            waited,
                            "{doc}: a catch-up that can still land holds the stop ({seen:?}, \
                             net: {})",
                            self.net_state(doc)
                        );
                    }
                    if owed && !waited {
                        prop_assert!(
                            failed.iter().any(|f| f.starts_with(&format!("{doc} ("))),
                            "{doc}: a document the stop leaves behind is named failed: {failed:?}"
                        );
                    }
                    // A running attempt may still land, so the stop may wait
                    // one attempt; a pending write retry may hold it until the
                    // retry comes due.
                    if ingest_failed
                        && !write_running
                        && !seen.write_retry_pending
                        && !ingest_running
                        && !self.running(READ_DISK, doc)
                        && self.net.tokens().token(DISK_CHANGED, doc).is_none()
                    {
                        prop_assert!(
                            !waited,
                            "{doc}: between attempts, a failed ingest holds no stop ({})",
                            self.net_state(doc)
                        );
                    }
                }
                Ok(())
            }
        }

        impl Model {
            fn net_state(&self, doc: &str) -> String {
                let retrying: Vec<_> = self
                    .net
                    .retrying()
                    .filter(|(_, d, _)| d.as_str() == doc)
                    .map(|(t, _, c)| format!("{t}: {}", c.text))
                    .collect();
                format!(
                    "ingest {:?}, write {:?}, retrying {retrying:?}",
                    why_not(&self.net, CATCH_UP_INGEST, doc),
                    why_not(&self.net, CATCH_UP_WRITE, doc)
                )
            }
        }

        fn doc_index(req: &EffectRequest) -> usize {
            DOCS.iter()
                .position(|d| *d == req.subject.as_str())
                .unwrap_or_else(|| panic!("the generator owns no {}", req.subject.as_str()))
        }

        fn run(steps: &[Step]) -> Result<(), TestCaseError> {
            let mut model = Model::new();
            model.check()?;
            for step in steps {
                model.apply(step);
                model.check()?;
            }
            Ok(())
        }

        /// The stop waits for every write that can still land and for every
        /// ingest with no failure on record; between attempts it waits for
        /// no failed ingest, and what it leaves behind is named failed.
        #[test]
        fn the_stop_waits_for_what_can_still_land() {
            use Outcome::*;
            use Step::*;
            let fixed = [
                // A disk read runs while the ingest of the file waits for its
                // retry.
                vec![DiskMoves(1), Complete(0, Retryable), FileChanges(1)],
                // A change reported again while its read waits for a retry: the
                // generator skips the second report.
                vec![
                    FileChanges(0),
                    Complete(0, Retryable),
                    Advance(12),
                    FileChanges(0),
                    Advance(88),
                ],
                // A write retry is due over a file back at its base after its
                // ingest failed.
                vec![
                    StoreMoves(0),
                    DiskMoves(0),
                    Complete(1, Retryable),
                    DiskReverts(0),
                    Complete(0, Retryable),
                ],
                // A store edit after a failed ingest starts a fresh attempt.
                vec![DiskMoves(0), Complete(0, Retryable), StoreMoves(0)],
                // A store edit while the retry runs keeps the failed run.
                vec![
                    DiskMoves(0),
                    Complete(0, Retryable),
                    StoreMoves(1),
                    Advance(99),
                    Complete(0, Done),
                    StoreMoves(0),
                    StoreMoves(0),
                ],
                // A write and an ingest that run side by side both fail.
                vec![
                    StoreMoves(0),
                    DiskMoves(0),
                    Complete(0, Retryable),
                    Advance(99),
                    Complete(0, Retryable),
                ],
                // A write retry is due over a file whose ingest failed.
                vec![
                    DiskMoves(0),
                    Complete(0, Done),
                    Complete(0, Retryable),
                    DiskMoves(0),
                    Complete(0, Retryable),
                ],
                // A write retry is due over a file whose ingest failed for
                // good.
                vec![
                    StoreMoves(0),
                    StoreMoves(0),
                    StoreMoves(1),
                    DiskMoves(0),
                    Complete(18388249744759922208, Retryable),
                    Complete(3439441474208776445, Definite),
                ],
                // New file content while a failed ingest's retry runs.
                vec![
                    DiskMoves(1),
                    StoreMoves(0),
                    Complete(1, Done),
                    DiskMoves(0),
                    Complete(1, Retryable),
                    Advance(100),
                    DiskMoves(0),
                ],
                // A write retry is due after the store went back to the base's
                // authority.
                vec![
                    DiskMoves(0),
                    DiskReverts(0),
                    Complete(0, Done),
                    DiskReverts(0),
                    StoreReverts(0),
                    Complete(8462056904535353347, Retryable),
                    Advance(99),
                    Complete(0, Retryable),
                ],
                // A write retry is pending over a file at its base, and the
                // store went back to the base's authority.
                vec![
                    StoreMoves(0),
                    StoreReverts(0),
                    Complete(0, Retryable),
                    DiskMoves(0),
                    DiskReverts(0),
                    Complete(0, Retryable),
                ],
                // A disk read fails for good while a write runs.
                vec![
                    StoreMoves(0),
                    StoreMoves(0),
                    FileChanges(0),
                    DiskMoves(1),
                    StoreMoves(0),
                    Complete(11448191397739138975, Definite),
                ],
                // A write retry is pending over a file at its base after its
                // ingest failed, while the disk read failed for good: its
                // `DiskChanged` inhibits the write, which cannot land.
                vec![
                    DiskMoves(1),
                    Complete(0, Done),
                    Complete(0, Retryable),
                    StoreMoves(0),
                    DiskMoves(1),
                    StoreReverts(0),
                    Complete(556302317005511942, Retryable),
                    Advance(97),
                    Complete(0, Retryable),
                    FileChanges(1),
                    Complete(0, Definite),
                    DiskReverts(1),
                ],
            ];
            for steps in &fixed {
                if let Err(e) = run(steps) {
                    panic!("{steps:?}: {e}");
                }
            }
            proptest::test_runner::TestRunner::new(ProptestConfig {
                cases: 512,
                failure_persistence: None,
                ..ProptestConfig::default()
            })
            .run(&prop::collection::vec(step(), 1..40), |steps| run(&steps))
            .unwrap_or_else(|e| panic!("{e}"));
        }
    }

    /// No document leaves [`unsettled`] while a catch-up of it runs.
    fn running_catch_ups_stay_unsettled(
        exec: &Executor,
    ) -> Result<(), proptest::test_runner::TestCaseError> {
        let unsettled = unsettled(exec);
        for doc in exec.tokens().subjects(BASE) {
            for t in CATCH_UP {
                if why_not(exec, t, doc.as_str()) == Some(holon_net::exec::WhyNot::InFlight) {
                    proptest::prop_assert!(
                        unsettled.iter().any(|d| d == doc.as_str()),
                        "{} left unsettled while {t} runs: {:?}",
                        doc.as_str(),
                        converged(exec, doc.as_str())
                    );
                }
            }
        }
        Ok(())
    }

    /// An effect fails while a catch-up of the same document runs or waits
    /// for its retry: the document stays unsettled.
    #[test]
    fn a_failed_effect_beside_an_owed_catch_up_stays_unsettled() {
        use holon_net::exec::Cause;
        for failing in [READ_DISK, ACCEPT_REMOVAL, REWRITE_FROM_STORE] {
            for definite in [true, false] {
                for catch_up in CATCH_UP {
                    for catch_up_fails in [false, true] {
                        let case = format!(
                            "{failing} fails (definite: {definite}) beside {catch_up} \
                             (failed: {catch_up_fails})"
                        );
                        let mut exec = seeded();
                        let remedy = (failing != READ_DISK).then(|| {
                            exec.write(
                                REFUSED,
                                DOC,
                                content([("seen", "h1"), ("authority", "a1")]),
                            );
                            let quiesced = exec
                                .exec
                                .fire(
                                    failing,
                                    DOC,
                                    OccurrenceId::caller("remedy:doc:a:0"),
                                    Content::new(),
                                    Duration::ZERO,
                                )
                                .expect("the remedy is enabled for a refused file");
                            exec.pending.extend(quiesced.requests);
                            one(&mut exec, 0)
                                .into_iter()
                                .find(|r| &*r.transition == failing)
                                .expect("the remedy runs")
                        });
                        if catch_up == CATCH_UP_WRITE {
                            exec.write(AUTHORITY, DOC, content([("hash", "a2")]));
                        } else {
                            exec.write(ON_DISK, DOC, content([("hash", "h2")]));
                        }
                        let running = one(&mut exec, 0);
                        let running = running
                            .iter()
                            .find(|r| &*r.transition == catch_up)
                            .unwrap_or_else(|| panic!("{case}: the catch-up runs"));
                        if catch_up_fails {
                            exec.complete(
                                running.attempt,
                                EffectResult::Failed(Cause::retryable("disk full")),
                                Duration::from_millis(1),
                            )
                            .unwrap();
                        }
                        let effect = remedy.unwrap_or_else(|| {
                            exec.write(DISK_CHANGED, DOC, Content::new());
                            one(&mut exec, 2)
                                .into_iter()
                                .find(|r| &*r.transition == READ_DISK)
                                .expect("the read runs")
                        });
                        let cause = if definite {
                            Cause::definite("no such document")
                        } else {
                            Cause::retryable("busy")
                        };
                        exec.complete(
                            effect.attempt,
                            EffectResult::Failed(cause),
                            Duration::from_millis(3),
                        )
                        .unwrap();
                        assert!(
                            owed(&exec, catch_up, DOC),
                            "{case}: the catch-up is still owed: {:?}",
                            why_not(&exec, catch_up, DOC)
                        );
                        let read_owed = failing == READ_DISK && !definite;
                        let reason = if read_owed {
                            NotConverged::ReadingDisk
                        } else {
                            NotConverged::CatchingUp(catch_up)
                        };
                        assert_eq!(converged(&exec, DOC), Err(reason), "{case}");
                        assert_eq!(unsettled(&exec), vec![DOC.to_string()], "{case}");
                        running_catch_ups_stay_unsettled(&exec)
                            .unwrap_or_else(|e| panic!("{case}: {e}"));
                    }
                }
            }
        }
    }

    /// [`converged`] against a model that keeps the file, the store's render
    /// and the base as texts of its own. A text is its own hash.
    mod converge {
        use std::collections::BTreeMap;
        use std::collections::BTreeSet;

        use holon_net::exec::Cause;
        use proptest::prelude::*;

        use super::stop::DOCS;
        use super::stop::Outcome;
        use super::stop::READ_TRIES;
        use super::stop::Step;
        use super::stop::step;
        use super::*;

        /// The executor alone on the test's clock, or behind the shell that
        /// production drives.
        trait Drive {
            fn write(&mut self, place: &str, doc: &str, content: Content);
            fn step(&mut self, now: Duration) -> Vec<EffectRequest>;
            fn complete(&mut self, req: &EffectRequest, result: EffectResult, now: Duration);
            fn exec<R>(&self, f: impl FnOnce(&Executor) -> R) -> R;
        }

        impl Drive for Net {
            fn write(&mut self, place: &str, doc: &str, content: Content) {
                Net::write(self, place, doc, content);
            }

            fn step(&mut self, now: Duration) -> Vec<EffectRequest> {
                one(self, now.as_millis() as u64)
            }

            fn complete(&mut self, req: &EffectRequest, result: EffectResult, now: Duration) {
                Net::complete(self, req.attempt, result, now).expect("the attempt is outstanding");
            }

            fn exec<R>(&self, f: impl FnOnce(&Executor) -> R) -> R {
                f(&self.exec)
            }
        }

        impl Drive for holon_net_shell::Shell {
            fn write(&mut self, place: &str, doc: &str, content: Content) {
                holon_net_shell::Shell::write(self, place, doc, content);
            }

            fn step(&mut self, _: Duration) -> Vec<EffectRequest> {
                holon_net_shell::Shell::step(self).expect("the net terminates")
            }

            fn complete(&mut self, req: &EffectRequest, result: EffectResult, _: Duration) {
                holon_net_shell::Shell::complete(self, req, result)
                    .expect("the attempt is outstanding");
            }

            fn exec<R>(&self, f: impl FnOnce(&Executor) -> R) -> R {
                self.read(f)
            }
        }

        #[derive(Debug)]
        struct Doc {
            file: String,
            store: String,
            /// The text Holon last wrote or ingested.
            base: String,
            /// The render the base holds; unknown after an ingest until a
            /// write confirms it.
            base_render: Option<String>,
            /// Bumped when the base changes: an effect's base lands only over
            /// the base it read.
            base_version: u32,
            /// The file as the net's `OnDisk` holds it.
            disk_seen: String,
            /// Bumped on each write of `OnDisk`: an effect's `OnDisk` lands
            /// only over the `OnDisk` it read.
            disk_version: u32,
            /// The watcher reported a change no disk read has taken yet.
            unread: bool,
            /// The failed tries of the disk read of the last reported change.
            read_failures: u32,
            /// A catch-up reported a file change while the disk read ran.
            read_overtaken: bool,
        }

        struct Model<D> {
            net: D,
            backoff: Duration,
            now: Duration,
            docs: BTreeMap<&'static str, Doc>,
            /// Each effect the net asked for, with the base and disk versions
            /// it read.
            outstanding: Vec<(EffectRequest, u32, u32)>,
            /// When the retry of a failed (transition, document) comes due.
            retries: BTreeMap<(String, &'static str), Duration>,
            counter: u32,
        }

        impl<D: Drive> Model<D> {
            fn new(mut net: D, backoff: Duration) -> Self {
                let mut docs = BTreeMap::new();
                for doc in DOCS {
                    net.write(AUTHORITY, doc, content([("hash", "t0")]));
                    net.write(BASE, doc, content([("hash", "t0"), ("authority", "t0")]));
                    net.write(ON_DISK, doc, content([("hash", "t0")]));
                    docs.insert(
                        doc,
                        Doc {
                            file: "t0".into(),
                            store: "t0".into(),
                            base: "t0".into(),
                            base_render: Some("t0".into()),
                            base_version: 0,
                            disk_seen: "t0".into(),
                            disk_version: 0,
                            unread: false,
                            read_failures: 0,
                            read_overtaken: false,
                        },
                    );
                }
                let mut model = Self {
                    net,
                    backoff,
                    now: Duration::ZERO,
                    docs,
                    outstanding: Vec::new(),
                    retries: BTreeMap::new(),
                    counter: 0,
                };
                model.step_net();
                model
            }

            fn fresh(&mut self) -> String {
                self.counter += 1;
                format!("t{}", self.counter)
            }

            fn render(&mut self, doc: &'static str) {
                let store = self.docs[doc].store.clone();
                self.net.write(AUTHORITY, doc, content([("hash", &store)]));
            }

            fn step_net(&mut self) {
                for req in self.net.step(self.now) {
                    let doc = doc_of(&req);
                    self.retries.remove(&(req.transition.to_string(), doc));
                    if &*req.transition == READ_DISK {
                        self.docs.get_mut(doc).unwrap().unread = false;
                    }
                    let d = &self.docs[doc];
                    let read = (d.base_version, d.disk_version);
                    self.outstanding.push((req, read.0, read.1));
                }
                let now = self.now;
                self.retries.retain(|_, due| *due > now);
            }

            fn rebase(&mut self, doc: &'static str, read: u32, base: &str, render: Option<&str>) {
                let d = self.docs.get_mut(doc).unwrap();
                if d.base_version != read
                    || (d.base.as_str(), d.base_render.as_deref()) == (base, render)
                {
                    return;
                }
                d.base = base.to_string();
                d.base_render = render.map(str::to_string);
                d.base_version += 1;
            }

            /// A write of an equal `OnDisk` leaves the version where it is.
            fn put_disk(&mut self, doc: &'static str, text: &str) {
                let d = self.docs.get_mut(doc).unwrap();
                if d.disk_seen != text {
                    d.disk_seen = text.to_string();
                    d.disk_version += 1;
                }
            }

            fn write_disk(&mut self, doc: &'static str, text: &str) {
                self.put_disk(doc, text);
                self.net.write(ON_DISK, doc, content([("hash", text)]));
            }

            fn land_disk(&mut self, doc: &'static str, read: u32, text: &str) {
                if self.docs[doc].disk_version == read {
                    self.put_disk(doc, text);
                }
            }

            fn apply(&mut self, step: &Step) {
                match *step {
                    Step::StoreMoves(d) => {
                        let text = self.fresh();
                        self.docs.get_mut(DOCS[d]).unwrap().store = text;
                        self.render(DOCS[d]);
                    }
                    Step::StoreReverts(d) => {
                        let Some(render) = self.docs[DOCS[d]].base_render.clone() else {
                            return;
                        };
                        self.docs.get_mut(DOCS[d]).unwrap().store = render;
                        self.render(DOCS[d]);
                    }
                    Step::DiskMoves(d) => {
                        let text = self.fresh();
                        self.docs.get_mut(DOCS[d]).unwrap().file = text.clone();
                        self.write_disk(DOCS[d], &text);
                    }
                    Step::DiskReverts(d) => {
                        let doc = self.docs.get_mut(DOCS[d]).unwrap();
                        doc.file = doc.base.clone();
                        let text = doc.file.clone();
                        self.write_disk(DOCS[d], &text);
                    }
                    Step::FileChanges(d) => {
                        // A catch-up may still run when the watcher reports;
                        // a running read already took the last report.
                        if self.busy(READ_DISK, DOCS[d]) {
                            return;
                        }
                        // shortcut: a report while `DiskChanged` is still held
                        // is not new input to the net, so a read that failed
                        // for good stays failed; skipped until a disk-change
                        // report is a new level.
                        if self
                            .net
                            .exec(|exec| exec.tokens().token(DISK_CHANGED, DOCS[d]).is_some())
                        {
                            return;
                        }
                        let text = self.fresh();
                        let doc = self.docs.get_mut(DOCS[d]).unwrap();
                        doc.file = text;
                        doc.unread = true;
                        doc.read_failures = 0;
                        self.net.write(DISK_CHANGED, DOCS[d], Content::new());
                    }
                    Step::Complete(i, outcome) => {
                        if self.outstanding.is_empty() {
                            return;
                        }
                        let (req, read, disk) = self.outstanding.remove(i % self.outstanding.len());
                        // shortcut: a `DiskChanged` that a catch-up outputs
                        // while the read runs is not new input to the net
                        // either (see `FileChanges`), so such a read succeeds;
                        // drawn as Done until a disk-change report is a new
                        // level.
                        let doc = doc_of(&req);
                        let outcome =
                            if &*req.transition == READ_DISK && self.docs[doc].read_overtaken {
                                Outcome::Done
                            } else {
                                outcome
                            };
                        self.complete(&req, read, disk, outcome);
                    }
                    Step::Advance(ms) => self.now += Duration::from_millis(ms),
                }
                self.step_net();
            }

            fn complete(&mut self, req: &EffectRequest, read: u32, disk: u32, outcome: Outcome) {
                let doc = doc_of(req);
                let subject = Subject::new(doc);
                if &*req.transition == READ_DISK {
                    self.docs.get_mut(doc).unwrap().read_overtaken = false;
                }
                let failed = |cause| {
                    let cause = match cause {
                        Outcome::Retryable => Cause::retryable("disk busy"),
                        _ => Cause::definite("no such document"),
                    };
                    EffectResult::Failed(cause)
                };
                if !matches!(outcome, Outcome::Done) {
                    self.net.complete(req, failed(outcome), self.now);
                    let read = &*req.transition == READ_DISK;
                    if read {
                        let d = self.docs.get_mut(doc).unwrap();
                        d.read_failures = match outcome {
                            Outcome::Retryable => d.read_failures + 1,
                            _ => READ_TRIES,
                        };
                    }
                    if matches!(outcome, Outcome::Retryable)
                        && !(read && self.docs[doc].read_failures >= READ_TRIES)
                    {
                        self.retries
                            .insert((req.transition.to_string(), doc), self.now + self.backoff);
                    }
                    return;
                }
                let file = self.docs[doc].file.clone();
                match &*req.transition {
                    READ_DISK => {
                        self.docs.get_mut(doc).unwrap().read_failures = 0;
                        let result =
                            EffectResult::Done(vec![output(ON_DISK, &subject, [("hash", &file)])]);
                        self.net.complete(req, result, self.now);
                        self.land_disk(doc, disk, &file);
                    }
                    CATCH_UP_INGEST => {
                        let result = EffectResult::Done(vec![
                            output(BASE, &subject, [("hash", &file), ("authority", INGESTED)]),
                            output(ON_DISK, &subject, [("hash", &file)]),
                        ]);
                        self.net.complete(req, result, self.now);
                        self.land_disk(doc, disk, &file);
                        self.rebase(doc, read, &file, None);
                        self.docs.get_mut(doc).unwrap().store = file;
                        self.render(doc);
                    }
                    CATCH_UP_WRITE if file != field(&req.input, "disk") => {
                        let reading = self.busy(READ_DISK, doc);
                        let d = self.docs.get_mut(doc).unwrap();
                        // shortcut: a report over a read that failed for good
                        // is not new input either (see `FileChanges`).
                        d.unread |= d.read_failures < READ_TRIES;
                        d.read_overtaken |= reading;
                        let result = EffectResult::Done(vec![output(DISK_CHANGED, &subject, [])]);
                        self.net.complete(req, result, self.now);
                    }
                    // The write lands the render the store held when the net
                    // asked for it; a text is its own hash here.
                    CATCH_UP_WRITE => {
                        let render = field(&req.input, "authority").to_string();
                        self.docs.get_mut(doc).unwrap().file = render.clone();
                        let result = EffectResult::Done(vec![
                            output(BASE, &subject, [("hash", &render), ("authority", &render)]),
                            output(ON_DISK, &subject, [("hash", &render)]),
                        ]);
                        self.net.complete(req, result, self.now);
                        self.land_disk(doc, disk, &render);
                        self.rebase(doc, read, &render, Some(&render));
                    }
                    other => panic!("the generator fires no {other}"),
                }
            }

            fn busy(&self, transition: &str, doc: &str) -> bool {
                self.outstanding
                    .iter()
                    .any(|(r, ..)| &*r.transition == transition && r.subject.as_str() == doc)
                    || self
                        .retries
                        .contains_key(&(transition.to_string(), doc_named(doc)))
            }

            fn expected(&self, doc: &'static str) -> Result<(), NotConverged> {
                let d = &self.docs[doc];
                if d.unread || self.busy(READ_DISK, doc) {
                    return Err(NotConverged::ReadingDisk);
                }
                if let Some(t) = CATCH_UP.into_iter().find(|t| self.busy(t, doc)) {
                    return Err(NotConverged::CatchingUp(t));
                }
                if d.read_failures >= READ_TRIES {
                    return Err(NotConverged::ReadFailed);
                }
                let agree = d.disk_seen == d.file
                    && d.file == d.store
                    && d.base == d.file
                    && d.base_render.as_deref() == Some(d.store.as_str());
                if !agree {
                    return Err(NotConverged::Behind);
                }
                Ok(())
            }

            fn observed(&self) -> Vec<Result<(), NotConverged>> {
                self.net
                    .exec(|exec| DOCS.iter().map(|doc| converged(exec, doc)).collect())
            }

            fn check(&self) -> Result<(), TestCaseError> {
                self.net.exec(running_catch_ups_stay_unsettled)?;
                let observed = self.observed();
                for (doc, observed) in DOCS.iter().zip(observed) {
                    prop_assert_eq!(
                        observed,
                        self.expected(doc),
                        "{}: {:?}, outstanding {:?}, retries {:?}",
                        doc,
                        self.docs[doc],
                        self.outstanding
                            .iter()
                            .map(|(r, ..)| format!("{} {}", r.transition, r.subject.as_str()))
                            .collect::<BTreeSet<_>>(),
                        self.retries
                    );
                }
                prop_assert_eq!(
                    self.net.exec(|exec| converged(exec, "doc:unseen")),
                    Err(NotConverged::TokensMissing)
                );
                Ok(())
            }
        }

        fn doc_named(doc: &str) -> &'static str {
            DOCS.into_iter()
                .find(|d| *d == doc)
                .unwrap_or_else(|| panic!("the generator owns no {doc}"))
        }

        fn doc_of(req: &EffectRequest) -> &'static str {
            doc_named(req.subject.as_str())
        }

        fn run(steps: &[Step]) -> Result<(), TestCaseError> {
            let mut model = Model::new(seeded_net(), CATCH_UP_BACKOFF);
            model.check()?;
            for step in steps {
                model.apply(step);
                model.check()?;
            }
            Ok(())
        }

        fn seeded_net() -> Net {
            Net {
                exec: Executor::new(net()).expect("the write-back net loads"),
                pending: Vec::new(),
                now: Duration::ZERO,
            }
        }

        /// On the shell, every operation that changes what [`converged`]
        /// says of a document wakes a waiter. The backoff outlasts the test,
        /// as the shell's clock is the wall clock.
        fn run_on_shell(steps: &[Step]) -> Result<(), TestCaseError> {
            let backoff = Duration::from_secs(3600);
            let shell = holon_net_shell::Shell::new(net_with_backoff(backoff))
                .expect("the write-back net loads");
            let mut wake = shell.subscribe();
            let mut model = Model::new(shell, backoff);
            model.check()?;
            for step in steps {
                let before = model.observed();
                wake.mark_unchanged();
                model.apply(step);
                model.check()?;
                let after = model.observed();
                prop_assert!(
                    before == after
                        || wake
                            .has_changed()
                            .expect("the shell owns the change counter"),
                    "{:?} moved the predicate from {:?} to {:?} without a wake",
                    step,
                    before,
                    after
                );
            }
            Ok(())
        }

        fn cases(f: fn(&[Step]) -> Result<(), TestCaseError>, fixed: &[Vec<Step>]) {
            for steps in fixed {
                if let Err(e) = f(steps) {
                    panic!("{steps:?}: {e}");
                }
            }
            proptest::test_runner::TestRunner::new(ProptestConfig {
                cases: 512,
                failure_persistence: None,
                ..ProptestConfig::default()
            })
            .run(&prop::collection::vec(step(), 1..40), |steps| f(&steps))
            .unwrap_or_else(|e| panic!("{e}"));
        }

        /// The predicate holds exactly when the file, the store's render and
        /// the base hold one text and no read or catch-up is owed.
        #[test]
        fn converged_is_the_file_and_the_store_agreeing_with_nothing_owed() {
            cases(
                run,
                // A disk read that outlasts a quiet window, then its catch-up.
                &[
                    vec![
                        Step::FileChanges(0),
                        Step::Advance(150),
                        Step::Complete(0, Outcome::Done),
                        Step::Advance(150),
                        Step::Complete(0, Outcome::Done),
                    ],
                    vec![
                        Step::StoreMoves(0),
                        Step::DiskMoves(0),
                        Step::DiskMoves(0),
                        Step::Complete(2650211736674996942, Outcome::Done),
                        Step::StoreMoves(0),
                    ],
                    vec![
                        Step::StoreMoves(0),
                        Step::FileChanges(0),
                        Step::StoreMoves(0),
                        Step::Complete(272034239675812356, Outcome::Done),
                        Step::Complete(0, Outcome::Definite),
                        Step::StoreMoves(0),
                    ],
                    vec![
                        Step::FileChanges(1),
                        Step::Complete(0, Outcome::Definite),
                        Step::FileChanges(1),
                    ],
                    vec![
                        Step::FileChanges(1),
                        Step::StoreMoves(1),
                        Step::StoreMoves(0),
                        Step::Complete(1986577108582112715, Outcome::Retryable),
                        Step::Complete(4455978334375579752, Outcome::Done),
                        Step::Advance(100),
                    ],
                    vec![
                        Step::FileChanges(0),
                        Step::FileChanges(0),
                        Step::Complete(0, Outcome::Retryable),
                        Step::Advance(42),
                        Step::Advance(58),
                    ],
                    vec![
                        Step::FileChanges(1),
                        Step::StoreMoves(1),
                        Step::DiskReverts(0),
                        Step::Advance(1),
                        Step::Complete(17687519835443213, Outcome::Done),
                        Step::Complete(0, Outcome::Retryable),
                        Step::Advance(100),
                    ],
                    vec![
                        Step::StoreMoves(0),
                        Step::FileChanges(0),
                        Step::Complete(8538321675475211017, Outcome::Definite),
                    ],
                ],
            );
        }

        #[test]
        fn every_change_of_the_predicate_wakes_its_waiters() {
            cases(
                run_on_shell,
                &[
                    vec![Step::FileChanges(0)],
                    vec![Step::FileChanges(0), Step::Complete(0, Outcome::Definite)],
                    vec![
                        Step::FileChanges(0),
                        Step::StoreMoves(0),
                        Step::Complete(15015742719820283155, Outcome::Done),
                        Step::Complete(0, Outcome::Definite),
                    ],
                    vec![
                        Step::StoreMoves(1),
                        Step::StoreMoves(0),
                        Step::DiskMoves(0),
                        Step::Complete(6118294481779761572, Outcome::Retryable),
                        Step::DiskReverts(0),
                        Step::DiskReverts(0),
                        Step::Complete(5184919082893520185, Outcome::Done),
                        Step::Complete(658760044849253869, Outcome::Definite),
                    ],
                    vec![
                        Step::StoreMoves(0),
                        Step::FileChanges(0),
                        Step::Complete(8538321675475211017, Outcome::Definite),
                    ],
                ],
            );
        }
    }
}
