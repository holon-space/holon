//! Net semantics revision 2 (#171, experiment #315): the storage lifecycle on
//! the REAL executor with fake providers behind
//! [`holon_api::storage_provider::StorageProvider`].
//!
//! The net: `ProviderTries` carries the try count per provider (token data, no
//! engine retry attribute); `Storage` holds the primary (one token per subject
//! IS capacity 1); `SecondaryStorage` is the standby slot; `SwapStorage` takes
//! BOTH store tokens in one firing, applies the final bounded delta while it
//! holds them and puts them back exchanged; `RetireStandby` closes the demoted
//! store. `ApplyWrite` also takes the `Storage` token, so no write falls
//! between the two stores.
//!
//! The naive twin is a NET variant, not an executor twin: with
//! `HOLON_NET_SWAP=naive` the swap keeps the ruled guard
//! (`lag <= FINAL_DELTA_MAX`) but leaves the catch-up outside the hold, so the
//! store that takes over is up to `FINAL_DELTA_MAX` writes behind.

mod exec_support;

use std::collections::BTreeMap;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use exec_support::Driver;
use exec_support::Reads;
use exec_support::Spec;
use exec_support::Transition;
use exec_support::bind;
use exec_support::content;
use exec_support::text;
use holon_api::Value;
use holon_api::storage_provider::ProviderFailure;
use holon_api::storage_provider::StorageProvider;
use holon_api::storage_provider::StorageVersion;
use holon_net::exec::Body;
use holon_net::exec::Cause;
use holon_net::exec::Class;
use holon_net::exec::Content;
use holon_net::exec::EffectRequest;
use holon_net::exec::EffectResult;
use holon_net::exec::OccurrenceId;
use holon_net::exec::Output;
use holon_net::exec::Receiver;
use holon_net::exec::Subject;
use holon_net::exec::Trigger;
use proptest::prelude::*;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use proptest_state_machine::prop_state_machine;

const VAULT: &str = "vault";
const TURSO: &str = "turso";
const LORO: &str = "loro";
/// Loro is the source of the projection pair and always opens; Turso is the
/// projection and the preferred primary.
const TURSO_PRIORITY: i64 = 20;
const LORO_PRIORITY: i64 = 10;
const MAX_TRIES: i64 = 3;
/// The lag the swap guard accepts, and therefore the budget of the final
/// catch-up under the hold (ruling NETREV2-contract.a).
const FINAL_DELTA_MAX: u64 = 2;
/// Background catch-up replays this many commits per firing, so a standby
/// reaches the swap guard's bound from above rather than in one jump.
const CATCH_UP_BATCH: u64 = 1;
const RUN_TIMER_SECS: u64 = 20;
/// A run that timed out is abandoned this long after, which is what lets the
/// next attempt open a fresh run.
const ABANDON_SECS: u64 = 3;
/// A `Keyed` receiver must declare a retry count bound. The try count is token
/// data here, so the bound is one attempt: every failure leaves through the
/// failure port and the net decides whether to attempt again.
const NO_ENGINE_RETRY: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------- the fakes

/// The source's commit sequence, shared by the pair: every write appends one
/// commit and a standby replays commits to catch up.
#[derive(Default)]
struct Source {
    commits: Mutex<u64>,
}

impl Source {
    fn tip(&self) -> u64 {
        *self
            .commits
            .lock()
            .expect("the source lock is not poisoned")
    }

    fn append(&self) -> u64 {
        let mut c = self
            .commits
            .lock()
            .expect("the source lock is not poisoned");
        *c += 1;
        *c
    }
}

/// What a fake provider does wrong, injected per store.
#[derive(Clone, Copy, Default)]
struct Faults {
    /// The first N opens fail definitely (a held lock, a refused connection).
    definite_open_failures: u32,
    /// Refuses to replay more than this many commits in ONE call. That breaks
    /// the postcondition, so it reports a definite failure instead of a short
    /// count.
    replay_cap_per_call: Option<u64>,
}

struct FakeStore {
    name: &'static str,
    source: Arc<Source>,
    faults: Faults,
    state: Mutex<FakeState>,
}

#[derive(Default)]
struct FakeState {
    open: bool,
    closed: bool,
    applied: u64,
    opens: u32,
    /// Writes this store accepted as the primary.
    writes: u64,
}

impl FakeStore {
    fn new(name: &'static str, source: Arc<Source>, faults: Faults) -> Arc<Self> {
        Arc::new(Self {
            name,
            source,
            faults,
            state: Mutex::new(FakeState::default()),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, FakeState> {
        self.state.lock().expect("the store lock is not poisoned")
    }

    fn applied(&self) -> u64 {
        self.lock().applied
    }

    fn writes(&self) -> u64 {
        self.lock().writes
    }

    /// The primary accepts a write: it appends a commit to the source and is
    /// itself at the new tip.
    fn apply_write(&self) -> StorageVersion {
        let mut s = self.lock();
        assert!(
            s.open && !s.closed,
            "{} took a write while not open",
            self.name
        );
        assert_eq!(
            s.applied,
            self.source.tip(),
            "{} took a write while {} commits behind the source",
            self.name,
            self.source.tip() - s.applied
        );
        let tip = self.source.append();
        s.applied = tip;
        s.writes += 1;
        StorageVersion(tip)
    }
}

#[async_trait::async_trait]
impl StorageProvider for FakeStore {
    fn name(&self) -> &str {
        self.name
    }

    async fn open(&self) -> Result<StorageVersion, ProviderFailure> {
        let mut s = self.lock();
        if s.closed {
            // A retired store's handle is spent. The net may still settle an
            // attempt that fired before the retirement.
            return Err(ProviderFailure::Definite(anyhow::anyhow!(
                "{}: the store was retired",
                self.name
            )));
        }
        // Idempotent by design: a run may have more than one attempt in
        // flight, so the net can ask an open store to open again.
        if s.open {
            return Ok(StorageVersion(s.applied));
        }
        s.opens += 1;
        let definite = self.faults.definite_open_failures;
        if s.opens <= definite {
            return Err(ProviderFailure::Definite(anyhow::anyhow!(
                "{}: the store lock is held (open {} of {definite})",
                self.name,
                s.opens
            )));
        }
        s.open = true;
        Ok(StorageVersion(s.applied))
    }

    fn version(&self) -> StorageVersion {
        let s = self.lock();
        assert!(
            s.open && !s.closed,
            "{}: version() on a store that is not open",
            self.name
        );
        StorageVersion(s.applied)
    }

    async fn catch_up(
        &self,
        target: StorageVersion,
        max_writes: NonZeroU64,
    ) -> Result<StorageVersion, ProviderFailure> {
        let mut s = self.lock();
        assert!(
            s.open && !s.closed,
            "{}: catch_up on a store that is not open",
            self.name
        );
        assert!(
            target.0 <= self.source.tip(),
            "{}: asked to catch up to {} past the source tip {}",
            self.name,
            target.0,
            self.source.tip()
        );
        let want = target.0.saturating_sub(s.applied).min(max_writes.get());
        if self
            .faults
            .replay_cap_per_call
            .is_some_and(|cap| want > cap)
        {
            return Err(ProviderFailure::Definite(anyhow::anyhow!(
                "{}: cannot replay {want} commits in one call",
                self.name
            )));
        }
        s.applied += want;
        Ok(StorageVersion(s.applied))
    }

    async fn close(&self) -> Result<(), ProviderFailure> {
        let mut s = self.lock();
        assert!(s.open && !s.closed, "{} was closed twice", self.name);
        s.open = false;
        s.closed = true;
        Ok(())
    }
}

/// The pair under test, addressed by the provider name the net carries.
struct Pair {
    source: Arc<Source>,
    stores: BTreeMap<&'static str, Arc<FakeStore>>,
    rt: tokio::runtime::Runtime,
}

impl Pair {
    fn new(turso: Faults) -> Self {
        let source = Arc::new(Source::default());
        let stores = BTreeMap::from([
            (TURSO, FakeStore::new(TURSO, source.clone(), turso)),
            (
                LORO,
                FakeStore::new(LORO, source.clone(), Faults::default()),
            ),
        ]);
        Self {
            source,
            stores,
            rt: tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("a current-thread runtime"),
        }
    }

    fn store(&self, name: &str) -> &Arc<FakeStore> {
        self.stores
            .get(name)
            .unwrap_or_else(|| panic!("no fake store named '{name}'"))
    }
}

// ------------------------------------------------------------------ the net

fn int_at(r: &mut dyn Reads, place: &str, subject: &str, field: &str) -> Option<i64> {
    match r.field(place, subject, field)? {
        Value::Integer(i) => Some(i),
        other => panic!("{place}.{field} is {other:?}, not an integer"),
    }
}

fn int_of(c: &Content, field: &str) -> i64 {
    exec_support::int(c, field)
}

fn str_of(c: &Content, field: &str) -> String {
    exec_support::string(c, field)
}

fn provider_token(name: &str, version: i64, priority: i64) -> Content {
    content(&[
        ("provider", text(name)),
        ("version", Value::Integer(version)),
        ("priority", Value::Integer(priority)),
    ])
}

fn tries_token(turso: i64, loro: i64) -> Content {
    content(&[
        ("turso", Value::Integer(turso)),
        ("loro", Value::Integer(loro)),
    ])
}

/// The standing wish for a store. A failure port carries only the cause, and a
/// run timer does NOT roll the consumed inputs back, so the try counter
/// re-posts the request together with the raised count.
fn storage_request() -> Content {
    content(&[("want", text("a store"))])
}

fn reposted(s: &Subject, tries: &Content) -> Vec<Output> {
    vec![
        Output::new("ProviderTries", s, tries.clone()),
        Output::new("StorageRequest", s, storage_request()),
    ]
}

fn naive_swap() -> bool {
    std::env::var("HOLON_NET_SWAP").as_deref() == Ok("naive")
}

/// `HOLON_NET_BOOT=blocking` restores the ladder that waits for ALL of the
/// preferred store's tries before the fallback opens, which is what REV2-boot.a
/// forbids.
fn blocking_boot() -> bool {
    std::env::var("HOLON_NET_BOOT").as_deref() == Ok("blocking")
}

/// A background try of the preferred store, carrying its own try count so the
/// transition needs no read arc on `ProviderTries` (a read that changes during
/// an attempt withholds the outputs AND eats the consumed request).
fn standby_request(tries: i64) -> Content {
    content(&[("provider", text(TURSO)), ("tries", Value::Integer(tries))])
}

/// Reads both store tokens and the swap's arguments out of the marking. `None`
/// — the guard is false — when either slot is empty (a write or another
/// exclusive holder has the token), the standby is not strictly preferred, or
/// the lag is above the bound the final delta can close.
fn swap_bind(r: &mut dyn Reads, s: &Subject) -> Option<Content> {
    let primary = r.text("Storage", s, "provider")?;
    let secondary = r.text("SecondaryStorage", s, "provider")?;
    let pv = int_at(r, "Storage", s, "version")?;
    let pp = int_at(r, "Storage", s, "priority")?;
    let sv = int_at(r, "SecondaryStorage", s, "version")?;
    let sp = int_at(r, "SecondaryStorage", s, "priority")?;
    if sp <= pp || pv - sv > FINAL_DELTA_MAX as i64 {
        return None;
    }
    Some(content(&[
        ("primary", text(&primary)),
        ("primary_version", Value::Integer(pv)),
        ("primary_priority", Value::Integer(pp)),
        ("provider", text(&secondary)),
        ("secondary_version", Value::Integer(sv)),
        ("secondary_priority", Value::Integer(sp)),
    ]))
}

/// The exchange both swap variants perform. The two-phase swap calls it with
/// the version its held final catch-up reached, the naive one with the version
/// the standby happened to have.
fn exchanged(s: &Subject, input: &Content, new_primary_version: i64) -> Vec<Output> {
    vec![
        Output::new(
            "Storage",
            s,
            provider_token(
                &str_of(input, "provider"),
                new_primary_version,
                int_of(input, "secondary_priority"),
            ),
        ),
        Output::new(
            "RetiringStorage",
            s,
            provider_token(
                &str_of(input, "primary"),
                int_of(input, "primary_version"),
                int_of(input, "primary_priority"),
            ),
        ),
    ]
}

fn net() -> Spec {
    let swap = Transition::new(
        "SwapStorage",
        Trigger::Auto,
        bind!(|r, s| swap_bind(r, s)),
        if naive_swap() {
            // The naive variant: the catch-up stays outside the hold, so the
            // store that takes over keeps the version it reached in the
            // background.
            Body::pure(|s, input| Ok(exchanged(s, input, int_of(input, "secondary_version"))))
        } else {
            Body::Effectful(Receiver::Keyed)
        },
    )
    .retry(NO_ENGINE_RETRY, 1)
    .consumes("Storage")
    .consumes("SecondaryStorage");

    Spec::default()
        .place("StorageOpened", Class::Process)
        .place("StorageRequest", Class::Process)
        .place("StandbyRequest", Class::Process)
        .place("ProviderTries", Class::Process)
        .place("Storage", Class::Process)
        .place("SecondaryStorage", Class::Process)
        .place("RetiringStorage", Class::Process)
        .place("TursoFailed", Class::Process)
        .place("LoroFailed", Class::Process)
        .place("WriteQueue", Class::Process)
        .place("WriteDone", Class::Process)
        // Turso goes first, but only ONCE: boot never blocks on its retries
        // (REV2-boot.a). A run timer ends a slow first attempt and counts that
        // try through the same failure port, which releases the fallback.
        .transition(
            Transition::new(
                "MakeTursoReady",
                Trigger::Auto,
                bind!(|r, s| {
                    r.present("StorageRequest", s).then_some(())?;
                    let tries = int_at(r, "ProviderTries", s, "turso")?;
                    let bound = if blocking_boot() { MAX_TRIES } else { 1 };
                    (tries < bound).then(|| content(&[("provider", text(TURSO))]))
                }),
                Body::Effectful(Receiver::Keyed),
            )
            .retry(NO_ENGINE_RETRY, 1)
            .consumes("StorageRequest")
            .reads("ProviderTries")
            // The standing fact that a store was opened, NOT the token slot: an
            // exclusive holder (a write, a swap) empties `Storage` for the
            // length of its hold, which must not look like "there is no store".
            .inhibits("StorageOpened")
            .on_failure("TursoFailed")
            .run_timer(Duration::from_secs(RUN_TIMER_SECS))
            .abandon(Duration::from_secs(ABANDON_SECS))
            .on_timeout("TursoFailed"),
        )
        // A failure port carries only the cause, and a definite failure rolls
        // the inputs back unchanged. So the try count is raised here, by
        // rewriting the tries token — which is also what re-enables the
        // attempt.
        .transition(
            Transition::new(
                "CountTursoTry",
                Trigger::Auto,
                bind!(|r, s| {
                    let turso = int_at(r, "ProviderTries", s, "turso")?;
                    let loro = int_at(r, "ProviderTries", s, "loro")?;
                    Some(tries_token(turso + 1, loro))
                }),
                Body::pure(|s, input| {
                    let mut out = reposted(s, input);
                    let turso = int_of(input, "turso");
                    // Turso's further tries run in the background, into the
                    // standby slot, so they never delay the fallback.
                    if !blocking_boot() && turso < MAX_TRIES {
                        out.push(Output::new("StandbyRequest", s, standby_request(turso)));
                    }
                    Ok(out)
                }),
            )
            .consumes("TursoFailed")
            .reads("ProviderTries"),
        )
        // The fallback opens as soon as Turso's FIRST try failed: a guard,
        // because the executor has no priority among the transitions competing
        // for one place.
        .transition(
            Transition::new(
                "MakeLoroReady",
                Trigger::Auto,
                bind!(|r, s| {
                    r.present("StorageRequest", s).then_some(())?;
                    let turso = int_at(r, "ProviderTries", s, "turso")?;
                    let loro = int_at(r, "ProviderTries", s, "loro")?;
                    let after = if blocking_boot() { MAX_TRIES } else { 1 };
                    (loro < MAX_TRIES && turso >= after)
                        .then(|| content(&[("provider", text(LORO))]))
                }),
                Body::Effectful(Receiver::Keyed),
            )
            .retry(NO_ENGINE_RETRY, 1)
            .consumes("StorageRequest")
            .reads("ProviderTries")
            .inhibits("StorageOpened")
            .on_failure("LoroFailed")
            .run_timer(Duration::from_secs(RUN_TIMER_SECS))
            .abandon(Duration::from_secs(ABANDON_SECS))
            .on_timeout("LoroFailed"),
        )
        .transition(
            Transition::new(
                "CountLoroTry",
                Trigger::Auto,
                bind!(|r, s| {
                    let turso = int_at(r, "ProviderTries", s, "turso")?;
                    let loro = int_at(r, "ProviderTries", s, "loro")?;
                    Some(tries_token(turso, loro + 1))
                }),
                Body::pure(|s, input| Ok(reposted(s, input))),
            )
            .consumes("LoroFailed")
            .reads("ProviderTries"),
        )
        // A manual reset of the try counts, so a user can ask for the preferred
        // store again after it gave up.
        .transition(
            Transition::new(
                "RetryProvider",
                Trigger::Manual,
                bind!(|r, s| r.present("ProviderTries", s).then(|| tries_token(0, 0))),
                Body::pure(|s, input| Ok(vec![Output::new("ProviderTries", s, input.clone())])),
            )
            .reads("ProviderTries"),
        )
        // The fallback asks for the preferred store in the background; it opens
        // into the standby slot, never into the occupied primary slot.
        .transition(
            Transition::new(
                "MakeTursoStandby",
                Trigger::Auto,
                bind!(|r, s| {
                    let provider = r.text("StandbyRequest", s, "provider")?;
                    let tries = int_at(r, "StandbyRequest", s, "tries")?;
                    // The count travels in the request, so this transition needs
                    // no read arc on `ProviderTries`.
                    (tries < MAX_TRIES).then(|| {
                        content(&[
                            ("provider", text(&provider)),
                            ("tries", Value::Integer(tries)),
                        ])
                    })
                }),
                Body::Effectful(Receiver::Keyed),
            )
            .retry(NO_ENGINE_RETRY, 1)
            .consumes("StandbyRequest")
            .inhibits("SecondaryStorage")
            .on_failure("TursoFailed")
            .run_timer(Duration::from_secs(RUN_TIMER_SECS))
            .abandon(Duration::from_secs(ABANDON_SECS))
            .on_timeout("TursoFailed"),
        )
        // Background catch-up: it does NOT hold the primary token, so writes
        // run while it replays. It stops at the swap guard's bound.
        .transition(
            Transition::new(
                "CatchUpSecondary",
                Trigger::Auto,
                bind!(|r, s| {
                    let secondary = r.text("SecondaryStorage", s, "provider")?;
                    let pv = int_at(r, "Storage", s, "version")?;
                    let sv = int_at(r, "SecondaryStorage", s, "version")?;
                    let prio = int_at(r, "SecondaryStorage", s, "priority")?;
                    (pv - sv > FINAL_DELTA_MAX as i64).then(|| {
                        content(&[
                            ("provider", text(&secondary)),
                            ("target", Value::Integer(pv)),
                            ("priority", Value::Integer(prio)),
                        ])
                    })
                }),
                Body::Effectful(Receiver::Keyed),
            )
            .retry(NO_ENGINE_RETRY, 1)
            .reads("SecondaryStorage")
            .reads("Storage"),
        )
        .transition(swap)
        .transition(
            Transition::new(
                "RetireStandby",
                Trigger::Auto,
                bind!(|r, s| {
                    let demoted = r.text("RetiringStorage", s, "provider")?;
                    Some(content(&[("provider", text(&demoted))]))
                }),
                Body::Effectful(Receiver::Keyed),
            )
            .retry(NO_ENGINE_RETRY, 1)
            .consumes("RetiringStorage"),
        )
        // A write is an exclusive holder: it takes the store token for its
        // duration, so no swap can fall between a write and its store.
        .transition(
            Transition::new(
                "ApplyWrite",
                Trigger::Auto,
                bind!(|r, s| {
                    let provider = r.text("Storage", s, "provider")?;
                    let prio = int_at(r, "Storage", s, "priority")?;
                    let seq = int_at(r, "WriteQueue", s, "seq")?;
                    Some(content(&[
                        ("provider", text(&provider)),
                        ("seq", Value::Integer(seq)),
                        ("priority", Value::Integer(prio)),
                    ]))
                }),
                Body::Effectful(Receiver::Keyed),
            )
            .retry(NO_ENGINE_RETRY, 1)
            .consumes("WriteQueue")
            .consumes("Storage"),
        )
}

// ------------------------------------------------------- playing the effects

fn opened(req: &EffectRequest, place: &str, provider: &str, v: StorageVersion) -> Vec<Output> {
    let prio = if provider == TURSO {
        TURSO_PRIORITY
    } else {
        LORO_PRIORITY
    };
    let mut out = vec![Output::new(
        place,
        &req.subject,
        provider_token(provider, v.0 as i64, prio),
    )];
    if place == "Storage" {
        out.push(Output::new(
            "StorageOpened",
            &req.subject,
            content(&[("provider", text(provider))]),
        ));
    }
    // In the blocking variant the preferred store has used up its tries by the
    // time the fallback opens, so the standby request starts here. The
    // non-blocking net posts it at the first failure instead.
    if blocking_boot() && place == "Storage" && provider == LORO {
        out.push(Output::new(
            "StandbyRequest",
            &req.subject,
            standby_request(0),
        ));
    }
    out
}

fn failed(e: ProviderFailure) -> EffectResult {
    match e {
        ProviderFailure::Definite(e) => EffectResult::Failed(Cause::definite(e.to_string())),
        ProviderFailure::Unknown(e) => EffectResult::Unknown(Cause::definite(e.to_string())),
    }
}

/// Runs one effect request against the fakes. This is the only place that calls
/// the provider trait: the executor itself knows nothing about stores.
fn run_effect(pair: &Pair, req: &EffectRequest) -> EffectResult {
    let input = &req.input;
    let provider = str_of(input, "provider");
    let store = pair.store(&provider);
    match &*req.transition {
        "MakeTursoReady" | "MakeLoroReady" | "MakeTursoStandby" => {
            let place = if req.transition.as_ref() == "MakeTursoStandby" {
                "SecondaryStorage"
            } else {
                "Storage"
            };
            match pair.rt.block_on(store.open()) {
                Ok(v) => EffectResult::Done(opened(req, place, &provider, v)),
                Err(e) => failed(e),
            }
        }
        "CatchUpSecondary" => {
            let target = StorageVersion(int_of(input, "target") as u64);
            let batch = NonZeroU64::new(CATCH_UP_BATCH).expect("a batch of at least one");
            match pair.rt.block_on(store.catch_up(target, batch)) {
                Ok(v) => EffectResult::Done(vec![Output::new(
                    "SecondaryStorage",
                    &req.subject,
                    provider_token(&provider, v.0 as i64, int_of(input, "priority")),
                )]),
                Err(e) => failed(e),
            }
        }
        // Phase two of the swap: both store tokens are held, so `target` cannot
        // move and a budget at the guard's bound MUST reach it. A shortfall is
        // a broken postcondition, reported as a definite failure of the swap
        // attempt — never a silent release of the hold.
        "SwapStorage" => {
            let target = StorageVersion(int_of(input, "primary_version") as u64);
            let budget = NonZeroU64::new(FINAL_DELTA_MAX).expect("a bound of at least one");
            match pair.rt.block_on(store.catch_up(target, budget)) {
                Ok(v) if v == target => {
                    EffectResult::Done(exchanged(&req.subject, input, v.0 as i64))
                }
                Ok(v) => EffectResult::Failed(Cause::definite(format!(
                    "{provider} reached {} of the frozen target {} with a budget of \
                     {FINAL_DELTA_MAX}: the provider broke its postcondition",
                    v.0, target.0
                ))),
                Err(e) => failed(e),
            }
        }
        "RetireStandby" => match pair.rt.block_on(store.close()) {
            Ok(()) => EffectResult::Done(vec![]),
            Err(e) => failed(e),
        },
        "ApplyWrite" => {
            let v = store.apply_write();
            EffectResult::Done(vec![
                Output::new(
                    "Storage",
                    &req.subject,
                    provider_token(&provider, v.0 as i64, int_of(input, "priority")),
                ),
                Output::new(
                    "WriteDone",
                    &req.subject,
                    content(&[("seq", Value::Integer(int_of(input, "seq")))]),
                ),
            ])
        }
        other => panic!("no fake effect for transition '{other}'"),
    }
}

/// Boots the net: a zeroed try count and a request for a store.
fn booted(turso: Faults) -> (Driver, Pair) {
    let pair = Pair::new(turso);
    let mut d = Driver::new(net());
    d.write("ProviderTries", VAULT, tries_token(0, 0))
        .expect("the net quiesces");
    d.write("StorageRequest", VAULT, storage_request())
        .expect("the net quiesces");
    (d, pair)
}

fn token_content<'a>(d: &'a Driver, place: &str) -> Option<&'a Content> {
    d.exec().tokens().token(place, VAULT).map(|t| &t.content)
}

/// Settles the oldest pending request of ONE transition. The order of the
/// queue is the effect runner's business, not the net's, so a test that needs a
/// particular interleaving picks the request it answers.
fn settle(d: &mut Driver, pair: &Pair, transition: &str) {
    let i = d
        .pending
        .iter()
        .position(|r| r.transition.as_ref() == transition)
        .unwrap_or_else(|| panic!("no pending request of '{transition}'"));
    let req = d.pending.remove(i).expect("the index was found above");
    let result = run_effect(pair, &req);
    d.complete(req.attempt, result).expect("the net quiesces");
}

// ----------------------------------------- property 1, the deterministic red

/// Property 1, the witness: a standby that is behind at swap time must be
/// caught up WHILE the swap holds both store tokens. The naive net exchanges
/// without that final delta and loses the writes in the gap.
///
/// `HOLON_NET_SWAP=naive` makes this red.
#[test]
fn a_swap_applies_the_final_delta_while_it_holds_the_stores() {
    if blocking_boot() {
        // The blocking variant is a harness for property 2 only: there the
        // preferred store keeps the boot slot, so this scenario has no standby.
        return;
    }
    let (mut d, pair) = booted(Faults {
        definite_open_failures: 1,
        ..Faults::default()
    });

    // Turso refuses its first open; Loro takes over at once and Turso's next
    // try goes to the standby slot.
    settle(&mut d, &pair, "MakeTursoReady");
    settle(&mut d, &pair, "MakeLoroReady");
    assert_eq!(
        str_of(token_content(&d, "Storage").expect("a primary"), "provider"),
        LORO,
        "the fallback serves"
    );

    // Turso's standby open is slow: five writes land on Loro while it is in
    // flight, so the standby arrives five commits behind.
    for seq in 1..=5 {
        d.write(
            "WriteQueue",
            VAULT,
            content(&[("seq", Value::Integer(seq))]),
        )
        .expect("the net quiesces");
        settle(&mut d, &pair, "ApplyWrite");
    }
    settle(&mut d, &pair, "MakeTursoStandby");
    assert_eq!(pair.store(TURSO).applied(), 0, "the standby opened behind");

    // Background catch-up narrows the lag to the swap guard's bound, one commit
    // per firing, and only then does the swap become enabled.
    for _ in 0..3 {
        settle(&mut d, &pair, "CatchUpSecondary");
    }
    assert_eq!(
        pair.store(TURSO).applied(),
        5 - FINAL_DELTA_MAX,
        "the background catch-up stops at the guard's bound"
    );

    if !naive_swap() {
        settle(&mut d, &pair, "SwapStorage");
    }
    let primary = token_content(&d, "Storage").expect("a primary");
    assert_eq!(str_of(primary, "provider"), TURSO, "the standby swapped in");
    assert_eq!(
        pair.store(TURSO).applied(),
        pair.source.tip(),
        "no_lost_write: the new primary is {} commits behind the source",
        pair.source.tip() - pair.store(TURSO).applied()
    );
    assert_eq!(
        int_of(primary, "version") as u64,
        pair.source.tip(),
        "no_lost_write: the primary token carries a stale version"
    );

    // Retirement is the last holder of the demoted store.
    settle(&mut d, &pair, "RetireStandby");
    assert!(
        token_content(&d, "RetiringStorage").is_none(),
        "the demoted store is retired"
    );
    assert_eq!(pair.store(LORO).writes(), 5, "Loro took the first 5 writes");
}

/// Ruled (NETREV2-contract.a): under the hold the target is frozen, so a budget
/// at the guard's bound MUST reach it. A provider that cannot replay that many
/// commits fails the swap attempt definitely; the hold goes back to its slots
/// and the standby does NOT take over.
#[test]
fn a_swap_that_cannot_apply_the_final_delta_fails_instead_of_releasing_the_hold() {
    if blocking_boot() {
        // The blocking variant is a harness for property 2 only: there the
        // preferred store keeps the boot slot, so this scenario has no standby.
        return;
    }
    if naive_swap() {
        // The naive variant has no catch-up inside the hold, so the rule this
        // test pins does not exist there.
        return;
    }
    let (mut d, pair) = booted(Faults {
        definite_open_failures: 1,
        // One commit per call: enough for the background batch, short of the
        // final delta.
        replay_cap_per_call: Some(1),
    });
    settle(&mut d, &pair, "MakeTursoReady");
    settle(&mut d, &pair, "MakeLoroReady");
    for seq in 1..=3 {
        d.write(
            "WriteQueue",
            VAULT,
            content(&[("seq", Value::Integer(seq))]),
        )
        .expect("the net quiesces");
        settle(&mut d, &pair, "ApplyWrite");
    }
    settle(&mut d, &pair, "MakeTursoStandby");
    settle(&mut d, &pair, "CatchUpSecondary");
    assert_eq!(pair.store(TURSO).applied(), 1, "the lag is now the bound");

    settle(&mut d, &pair, "SwapStorage");
    assert_eq!(
        str_of(token_content(&d, "Storage").expect("a primary"), "provider"),
        LORO,
        "the failed swap left the primary in place"
    );
    assert_eq!(
        str_of(
            token_content(&d, "SecondaryStorage").expect("a standby"),
            "provider"
        ),
        TURSO,
        "the failed swap returned the standby to its slot"
    );
    assert!(
        token_content(&d, "RetiringStorage").is_none(),
        "a failed swap retires nothing"
    );
    assert_eq!(pair.store(LORO).applied(), 3, "no write was lost");
}

// ----------------------------------------- property 1, over any interleaving

#[derive(Clone, Debug)]
enum Op {
    /// An outside write request.
    Enqueue,
    /// The effect runner answers the oldest pending request.
    Settle,
    /// Time passes, so a run timer can end a slow attempt.
    Tick(u64),
    /// The user asks for the preferred provider again.
    Retry,
}

struct Ref;

impl ReferenceStateMachine for Ref {
    type State = ();
    type Transition = Op;

    fn init_state() -> BoxedStrategy<()> {
        Just(()).boxed()
    }

    fn transitions(_: &()) -> BoxedStrategy<Op> {
        prop_oneof![
            4 => Just(Op::Enqueue),
            8 => Just(Op::Settle),
            1 => (1u64..30).prop_map(Op::Tick),
            1 => Just(Op::Retry),
        ]
        .boxed()
    }

    fn apply(state: (), _: &Op) {
        state
    }
}

struct Sut {
    d: Driver,
    pair: Pair,
    seq: i64,
}

struct Test;

impl StateMachineTest for Test {
    type SystemUnderTest = Sut;
    type Reference = Ref;

    fn init_test(_: &()) -> Sut {
        // Turso refuses its first open, so Loro takes over at once and Turso
        // comes back through the standby slot.
        let (d, pair) = booted(Faults {
            definite_open_failures: 1,
            ..Faults::default()
        });
        Sut { d, pair, seq: 0 }
    }

    fn apply(mut sut: Sut, _: &(), op: Op) -> Sut {
        match op {
            // The queue place holds one token per subject, so a second request
            // would overwrite the first OUTSIDE the net. The producer waits.
            Op::Enqueue if sut.d.exec().tokens().token("WriteQueue", VAULT).is_none() => {
                sut.seq += 1;
                sut.d.write(
                    "WriteQueue",
                    VAULT,
                    content(&[("seq", Value::Integer(sut.seq))]),
                )
            }
            Op::Enqueue => Ok(()),
            Op::Settle => {
                let pair = &sut.pair;
                sut.d.complete_oldest(|req| run_effect(pair, req))
            }
            Op::Tick(secs) => sut.d.tick(secs),
            Op::Retry => sut
                .d
                .fire("RetryProvider", VAULT, OccurrenceId::caller("retry"))
                .map(|_| ())
                .or(Ok(())),
        }
        .expect("the net quiesces");
        sut
    }

    fn check_invariants(sut: &Sut, _: &()) {
        let quiet = sut.d.pending.is_empty();
        let primary = token_content(&sut.d, "Storage");

        // no_lost_write: while nothing is in flight, the store in the primary
        // slot has applied every commit of the source. The naive swap breaks
        // this: it installs a standby that is still behind.
        if quiet && let Some(t) = primary {
            let name = str_of(t, "provider");
            let store = sut.pair.store(&name);
            assert_eq!(
                store.applied(),
                sut.pair.source.tip(),
                "no_lost_write: the primary '{name}' has applied {} of the source's {} commits",
                store.applied(),
                sut.pair.source.tip()
            );
            assert_eq!(
                int_of(t, "version") as u64,
                sut.pair.source.tip(),
                "no_lost_write: the primary token of '{name}' carries a stale version"
            );
        }

        // swap_runs: the preferred store never sits in the standby slot at
        // quiescence while a worse one serves and the lag is within the bound.
        if quiet && let (Some(p), Some(s)) = (primary, token_content(&sut.d, "SecondaryStorage")) {
            let lag = int_of(p, "version") - int_of(s, "version");
            assert!(
                int_of(s, "priority") <= int_of(p, "priority") || lag > FINAL_DELTA_MAX as i64,
                "swap_runs: '{}' is preferred and within the bound, yet '{}' still serves",
                str_of(s, "provider"),
                str_of(p, "provider")
            );
        }

        // one_writer: the two stores together took exactly the source's commits.
        let took = sut.pair.store(TURSO).writes() + sut.pair.store(LORO).writes();
        assert_eq!(
            took,
            sut.pair.source.tip(),
            "one_writer: the stores took {took} writes for {} commits",
            sut.pair.source.tip()
        );
    }
}

prop_state_machine! {
    #![proptest_config(ProptestConfig { cases: 48, .. ProptestConfig::default() })]

    /// Property 1, generalised: over any interleaving of writes, settlements,
    /// clock jumps and manual retries, the primary slot never holds a store
    /// that is behind the source.
    #[test]
    fn a_standby_swaps_in_without_losing_a_write(sequential 1..40 => Test);
}

// ------------------------------------- property 2, over any clock and answers

/// A store that refuses every open, so the boot slot can only be left by the
/// try count — never by success.
const NEVER_OPENS: u32 = u32::MAX;

#[derive(Clone, Debug)]
enum BootOp {
    /// The effect runner answers the oldest pending request.
    Settle,
    /// Time passes, by any jump.
    Tick(u64),
}

struct BootRef;

impl ReferenceStateMachine for BootRef {
    type State = ();
    type Transition = BootOp;

    fn init_state() -> BoxedStrategy<()> {
        Just(()).boxed()
    }

    fn transitions(_: &()) -> BoxedStrategy<BootOp> {
        prop_oneof![
            2 => Just(BootOp::Settle),
            3 => (1u64..40).prop_map(BootOp::Tick),
        ]
        .boxed()
    }

    fn apply(state: (), _: &BootOp) {
        state
    }
}

struct BootSut {
    d: Driver,
    pair: Pair,
}

struct BootTest;

impl StateMachineTest for BootTest {
    type SystemUnderTest = BootSut;
    type Reference = BootRef;

    fn init_test(_: &()) -> BootSut {
        let (d, pair) = booted(Faults {
            definite_open_failures: NEVER_OPENS,
            ..Faults::default()
        });
        BootSut { d, pair }
    }

    fn apply(mut sut: BootSut, _: &(), op: BootOp) -> BootSut {
        match op {
            BootOp::Settle => {
                let pair = &sut.pair;
                sut.d.complete_oldest(|req| run_effect(pair, req))
            }
            BootOp::Tick(secs) => sut.d.tick(secs),
        }
        .expect("the net quiesces");
        sut
    }

    fn check_invariants(sut: &BootSut, _: &()) {
        let asked = sut.d.requests_of("MakeLoroReady").count();
        let boot_tries = sut.d.requests_of("MakeTursoReady").count();

        // non_blocking_boot: once the preferred store's FIRST attempt is over
        // (its run timer at the latest), the fallback is asked. Nothing the
        // preferred store does afterwards may push this out.
        if sut.d.now.as_secs() > RUN_TIMER_SECS {
            assert!(
                asked >= 1 || token_content(&sut.d, "Storage").is_some(),
                "non_blocking_boot: {} s in, the fallback was never asked \
                 (the preferred store was asked {boot_tries} times)",
                sut.d.now.as_secs()
            );
        }

        // one_boot_try: the preferred store's later tries run in the background
        // standby slot, so the boot slot is used exactly once.
        assert!(
            boot_tries <= 1,
            "one_boot_try: the boot slot asked the preferred store {boot_tries} times, \
             so its retries sit between the user and a working store"
        );
    }
}

prop_state_machine! {
    #![proptest_config(ProptestConfig { cases: 48, .. ProptestConfig::default() })]

    /// Property 2: for any clock jump and any answer order, the fallback is
    /// asked within the preferred store's first attempt, and the preferred
    /// store's later tries never delay it.
    #[test]
    fn a_fallback_is_asked_within_the_first_attempt(sequential 1..30 => BootTest);
}

// ------------------------------------------------------------- property 3

/// The run timer ends a slow attempt and that counts ONE try, exactly like a
/// definite failure; the attempt then runs again because the try count changed.
#[test]
fn a_timeout_counts_one_try() {
    if blocking_boot() {
        // The blocking variant is a harness for property 2 only: there the
        // preferred store keeps the boot slot, so this scenario has no standby.
        return;
    }
    // A store that never answers: the run timer is the only thing that ends an
    // attempt.
    let (mut d, _pair) = booted(Faults::default());
    let tries = |d: &Driver| {
        int_of(
            token_content(d, "ProviderTries").expect("a tries token"),
            "turso",
        )
    };
    assert_eq!(tries(&d), 0, "no try is spent while the attempt is open");
    assert_eq!(d.requests_of("MakeTursoReady").count(), 1);

    d.tick(RUN_TIMER_SECS + 1).expect("the net quiesces");
    assert_eq!(
        tries(&d),
        1,
        "the run timer ended the attempt and that counts one try"
    );
    assert_eq!(
        d.requests_of("MakeTursoReady").count(),
        1,
        "the boot slot asks the preferred store exactly once"
    );
    assert_eq!(
        d.requests_of("MakeLoroReady").count(),
        1,
        "that one try released the fallback"
    );
    assert_eq!(
        d.requests_of("MakeTursoStandby").count(),
        1,
        "the preferred store's next try runs in the background"
    );
}

/// Property 2 (REV2-boot.a), the witness: a preferred store that never answers
/// must not delay the fallback. The bound is Turso's FIRST attempt plus Loro's
/// open — Turso's later tries run in the standby slot and never extend it.
///
/// `HOLON_NET_BOOT=blocking` makes this red: there the fallback waits for all
/// MAX_TRIES tries, so the bound is MAX_TRIES * (run timer + abandon).
#[test]
fn a_silent_preferred_store_never_delays_the_fallback() {
    let (mut d, pair) = booted(Faults::default());

    d.tick(RUN_TIMER_SECS + 1).expect("the net quiesces");
    assert!(
        d.pending
            .iter()
            .any(|r| r.transition.as_ref() == "MakeLoroReady"),
        "non_blocking_boot: {} s after the first attempt timed out the fallback was not asked \
         (the preferred store was asked {} times)",
        d.now.as_secs(),
        d.requests_of("MakeTursoReady").count()
    );

    settle(&mut d, &pair, "MakeLoroReady");
    assert_eq!(
        str_of(token_content(&d, "Storage").expect("a primary"), "provider"),
        LORO,
        "the fallback serves"
    );
    assert_eq!(
        d.now.as_secs(),
        RUN_TIMER_SECS + 1,
        "the store is up within the first attempt's bound plus the fallback's open"
    );
}

/// A manual `RetryProvider` puts the try counts back to zero, so the preferred
/// store can be attempted again after it gave up.
#[test]
fn a_manual_retry_resets_the_tries() {
    if blocking_boot() {
        // The blocking variant is a harness for property 2 only: there the
        // preferred store keeps the boot slot, so this scenario has no standby.
        return;
    }
    let (mut d, pair) = booted(Faults {
        definite_open_failures: MAX_TRIES as u32,
        ..Faults::default()
    });
    // One try in the boot slot, the rest in the background standby slot.
    settle(&mut d, &pair, "MakeTursoReady");
    for _ in 1..MAX_TRIES {
        settle(&mut d, &pair, "MakeTursoStandby");
    }
    let tries = |d: &Driver| {
        let t = token_content(d, "ProviderTries").expect("a tries token");
        (int_of(t, "turso"), int_of(t, "loro"))
    };
    assert_eq!(tries(&d), (MAX_TRIES, 0), "Turso spent every try");

    d.fire("RetryProvider", VAULT, OccurrenceId::caller("retry#1"))
        .expect("ProviderTries holds a token");
    assert_eq!(tries(&d), (0, 0), "the manual retry reset the counts");
}
