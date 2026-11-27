---------------------------- MODULE HomeBySettle ----------------------------
(***************************************************************************)
(* The home_by fold of the org write-back, its supervisor channel, the     *)
(* write-back consumer and the block feed's delivery cursor, read by one   *)
(* settle. Each action is one step of the PBT in                           *)
(* crates/holon-api/src/live_data/home_by/settle_spec.rs; the variables    *)
(* are the abstract state that test records after each step.               *)
(*                                                                         *)
(* A version is a CDC batch seq: FeedChange number k delivers batch k.     *)
(* A supervised restart (Supervised::Reset) is not modelled.               *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS
    MaxFeed,    \* batches in one run
    Design      \* "flag": the settle reads only the fold's progress flag;
                \* "through": the settle reads the block feed's delivery cursor

ASSUME Design \in {"flag", "through"}

VARIABLES
    feed,       \* LiveData::consumed_seq of the block feed
    taken,      \* the seq through which the fold took the feed
    mid,        \* the fold is parked inside a burst, in an authority read
    busy,       \* HomeByProgress::is_folding
    chan,       \* the seq whose Homed::Through is in the supervisor channel, 0 if empty
    cons,       \* the seq whose Homed::Through the consumer received
    applied,    \* DeliveryCursor::applied of the write-back feed
    returned    \* this step was a settle read that found nothing owed

vars == <<feed, taken, mid, busy, chan, cons, applied, returned>>

Init ==
    /\ feed = 0 /\ taken = 0 /\ mid = FALSE /\ busy = FALSE
    /\ chan = 0 /\ cons = 0 /\ applied = 0 /\ returned = FALSE

\* Models: crates/holon-api/src/live_data.rs:subscribe (the apply actor queues
\* the batch's changes on the map, then stores last_consumed_seq) and
\* DeliveryCursor::routed, which the CDC router calls before it hands the
\* batch over.
\* Modelling choice: routing and applying one batch is one step.
FeedChange ==
    /\ feed < MaxFeed
    /\ feed' = feed + 1
    /\ returned' = FALSE
    /\ UNCHANGED <<taken, mid, busy, chan, cons, applied>>

\* Models: crates/holon-api/src/live_data/home_by.rs:home_diffs_with_progress
\* (a burst reads the consumed seq, raises the progress flag, then parks in
\* its first authority read; a burst whose batches changed nothing reads no
\* authority, so it ends as in Pump).
PumpHeld ==
    /\ IF ~mid /\ feed > taken
         THEN \/ taken' = feed /\ mid' = TRUE /\ busy' = TRUE /\ UNCHANGED chan
              \/ taken' = feed /\ mid' = FALSE /\ busy' = FALSE /\ chan' = feed
         ELSE UNCHANGED <<taken, mid, busy, chan>>
    /\ returned' = FALSE
    /\ UNCHANGED <<feed, cons, applied>>

\* Models: crates/holon-api/src/live_data/home_by.rs:home_diffs_with_progress
\* (each burst yields its diffs, then Homed::Through; the flag drops when the
\* fold parks on the source) and
\* crates/holon-api/src/live_data/supervision.rs:run_supervised (tx.send).
\* Modelling choice: one poll runs the fold until it parks on the source, so
\* it takes every batch the feed holds.
Pump ==
    /\ IF mid \/ feed > taken
         THEN taken' = feed /\ mid' = FALSE /\ busy' = FALSE /\ chan' = feed
         ELSE UNCHANGED <<taken, mid, busy, chan>>
    /\ returned' = FALSE
    /\ UNCHANGED <<feed, cons, applied>>

\* Models: crates/holon-orgmode/src/di.rs:register_org_file_sync_core (the
\* org-writeback-consumer loop receives every diff and marker in the channel).
Take ==
    /\ cons' = IF chan > 0 THEN chan ELSE cons
    /\ chan' = 0
    /\ returned' = FALSE
    /\ UNCHANGED <<feed, taken, mid, busy, applied>>

\* Models: crates/holon-orgmode/src/di.rs:register_org_file_sync_core (the
\* Homed::Through arm: DeliveryCursor::applied_through).
\* Modelling choice: receiving the marker (Take) and applying it are two
\* steps; the code does both in one task step.
DeliverCursor ==
    /\ applied' = cons
    /\ returned' = FALSE
    /\ UNCHANGED <<feed, taken, mid, busy, chan, cons>>

\* Models, Design "through": crates/holon-app/src/session.rs:writeback_settles
\* (the feed stage waits until complete_through_for(writeback_feed) reaches
\* the emitted seq) and crates/holon-api/src/delivery_ledger.rs:owed.
\* Models, Design "flag": the write-back stage of
\* crates/holon-integration-tests/src/pbt/convergence.rs:writeback_owed
\* (writeback_fold_in_flight) without a delivery cursor.
SettleRead ==
    /\ returned' = IF Design = "through" THEN applied = feed ELSE ~busy
    /\ UNCHANGED <<feed, taken, mid, busy, chan, cons, applied>>

Next == FeedChange \/ PumpHeld \/ Pump \/ Take \/ DeliverCursor \/ SettleRead

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
P == INSTANCE SettleProperties WITH
        Writes <- MaxFeed,
        newest <- feed,
        output <- cons,
        settling <- FALSE,
        returned <- returned

SettledReadsNewest == P!SettledReadsNewest

\* Witnesses: run.py expects TLC to violate each, so each state is reachable.
QuietReadAfterChangeUnreached == ~(returned /\ feed > 0)
FoldNeverParked               == ~mid
ChannelNeverHolds             == chan = 0
ConsumerNeverTakes            == cons = 0
CursorNeverApplied            == applied = 0
================================================================================
