------------------------------ MODULE Writeback ------------------------------
(***************************************************************************)
(* Org write-back of one block in one document, and a test that settles    *)
(* and then reads the file.                                                *)
(*                                                                         *)
(* Code anchors are at commit ffa1611f. The pipeline is the same at the    *)
(* pre-fix commit b452395b, the parent of 3b10d7bb1b; only the settle      *)
(* predicate differs, selected by Design.                                  *)
(*                                                                         *)
(* A version number stands for the text of the block: the render of        *)
(* version v is v. Every action is one atomic step; a step of another      *)
(* actor can fall between any two steps, as a loaded host allows.          *)
(***************************************************************************)
EXTENDS Naturals, Sequences

CONSTANTS
    Writes,     \* editor writes in one run
    Bulk,       \* TRUE: a block diff may route to the debounced bulk re-render
    Design      \* "prefix": settle of b452395b; "main": settle of ffa1611f;
                \* "through": main with the write-back feed cursor of the open
                \* convergence-predicate diff

ASSUME Writes \in Nat /\ Bulk \in BOOLEAN /\ Design \in {"prefix", "main", "through"}

VARIABLES
    newest,     \* the newest version the editor wrote
    store,      \* the version the store holds
    feed,       \* the version the block feed applied (its consumed_seq)
    srcPending, \* the feed holds a diff the home_by fold has not taken
    folding,    \* HomeByProgress::is_folding
    foldVer,    \* the version the fold is working on
    supChan,    \* supervisor -> write-back consumer channel (versions)
    handed,     \* the version whose renders the consumer has queued
    queued,     \* QueuedRenders: re-renders sent and not yet processed
    rrChan,     \* consumer -> controller loop re-render channel
    bulk,       \* bulk_pending: a debounced bulk re-render is armed
    authority,  \* write-back net Authority: the render the store owes the file
    base,       \* write-back net Base: the text Holon last wrote
    disk,       \* the file's content
    changed,    \* file change seq the filesystem broadcast
    processed,  \* processed file change seq (OrgSyncIdleSignal change_seq)
    polled,     \* polled_change_seq
    tick,       \* OrgSyncIdleSignal tick
    phase,      \* the test: "idle", "poll" (settling), "returned"
    snap        \* the settle's previous read

vars == <<newest, store, feed, srcPending, folding, foldVer, supChan, handed, queued,
          rrChan, bulk, authority, base, disk, changed, processed, polled,
          tick, phase, snap>>

-----------------------------------------------------------------------------
(* Settle predicates *)

\* Models: crates/holon-integration-tests/src/pbt/convergence.rs:converge_signals
\* at b452395b. Activity = a moved CDC watermark, block-feed consumed_seq or
\* org tick, or the fold / pass_in_flight flag. A pass is one atomic step here,
\* so in_pass is never seen raised; bulk_pending is the rest of pass_in_flight.
PrefixRead  == <<store, feed, tick>>
PrefixQuiet == ~folding /\ ~bulk

\* Models: crates/holon-integration-tests/src/pbt/convergence.rs:owed and
\* writeback_owed at ffa1611f.
\* Modelling choice: the CDC delivery ledger and the block-feed stage are one
\* comparison, feed < store, because routing is part of the commit here.
MainRead  == <<store, feed, changed, processed, polled>>
MainQuiet ==
    /\ feed = store                  \* cdc + block-feed stages
    /\ processed = changed           \* org-ingest stage
    /\ polled = changed              \* org-poll stage
    /\ ~bulk                         \* pass_in_flight
    /\ ~folding                      \* writeback_fold_in_flight
    /\ queued = 0                    \* queued_renders
    /\ authority = base              \* a file behind its store (CatchUp enabled)

\* Models the open convergence-predicate diff: the block feed's delivery
\* cursor applies a batch only when the write-back consumer takes the fold's
\* Through marker, after every render the batch implies is queued
\* (crates/holon-orgmode/src/di.rs:register_org_file_sync_core, Homed::Through
\* arm). It replaces the feed = store comparison.
ThroughRead  == <<store, handed, changed, processed, polled>>
ThroughQuiet == MainQuiet /\ handed = store

Read  == CASE Design = "prefix" -> PrefixRead
           [] Design = "main"   -> MainRead
           [] OTHER             -> ThroughRead
Quiet == CASE Design = "prefix" -> PrefixQuiet
           [] Design = "main"   -> MainQuiet
           [] OTHER             -> ThroughQuiet

-----------------------------------------------------------------------------
Init ==
    /\ newest = 0 /\ store = 0 /\ feed = 0
    /\ srcPending = FALSE /\ folding = FALSE /\ foldVer = 0
    /\ supChan = <<>> /\ handed = 0 /\ queued = 0 /\ rrChan = <<>> /\ bulk = FALSE
    /\ authority = 0 /\ base = 0 /\ disk = 0
    /\ changed = 0 /\ processed = 0 /\ polled = 0 /\ tick = 0
    /\ phase = "idle" /\ snap = Read

\* Modelling choice: any operation that commits a block edit to the store.
\* The test edits only between settles, as a PBT transition does.
EditorWrite ==
    /\ phase = "idle"
    /\ newest < Writes
    /\ newest' = newest + 1
    /\ store' = newest + 1
    /\ UNCHANGED <<feed, srcPending, folding, foldVer, supChan, handed, queued, rrChan,
                   bulk, authority, base, disk, changed, processed, polled,
                   tick, phase, snap>>

\* Models: crates/holon-api/src/live_data.rs:subscribe (the apply actor:
\* apply_changes_with_origins queues the diff on the signal map, then
\* last_consumed_seq is stored). One batch carries every committed write.
FeedApply ==
    /\ feed < store
    /\ feed' = store
    /\ srcPending' = TRUE
    /\ UNCHANGED <<newest, store, folding, foldVer, supChan, handed, queued, rrChan,
                   bulk, authority, base, disk, changed, processed, polled,
                   tick, phase, snap>>

\* Models: crates/holon-api/src/live_data/home_by.rs:home_diffs_with_progress
\* (a burst is taken, then progress.set(true) before the authority reads).
FoldTake ==
    /\ srcPending /\ ~folding
    /\ srcPending' = FALSE
    /\ folding' = TRUE
    /\ foldVer' = feed
    /\ UNCHANGED <<newest, store, feed, supChan, handed, queued, rrChan, bulk,
                   authority, base, disk, changed, processed, polled, tick,
                   phase, snap>>

\* Models: crates/holon-api/src/live_data/home_by.rs:home_diffs_with_progress
\* (the diff is yielded; progress.set(false) once pending is drained) and
\* crates/holon-api/src/live_data/supervision.rs:run_supervised (tx.send).
FoldHand ==
    /\ folding
    /\ supChan' = Append(supChan, foldVer)
    /\ folding' = FALSE
    /\ UNCHANGED <<handed, newest, store, feed, srcPending, foldVer, queued, rrChan,
                   bulk, authority, base, disk, changed, processed, polled,
                   tick, phase, snap>>

\* Models: crates/holon-orgmode/src/di.rs:register_org_file_sync_core (the
\* org-writeback-consumer task: route_upsert, queued.queued, tx.send).
\* Modelling choice: the route is free; a block whose home the walk cannot
\* name routes to OrgRerender::All (BlockRoute::Recover).
ConsumerRoute ==
    /\ supChan /= <<>>
    /\ \E kind \in IF Bulk THEN {"block", "all"} ELSE {"block"} :
          rrChan' = Append(rrChan, [k |-> kind, v |-> Head(supChan)])
    /\ supChan' = Tail(supChan)
    /\ handed' = Head(supChan)
    /\ queued' = queued + 1
    /\ UNCHANGED <<newest, store, feed, srcPending, folding, foldVer, bulk,
                   authority, base, disk, changed, processed, polled, tick,
                   phase, snap>>

LastBlock(s) ==
    LET blocks == SelectSeq(s, LAMBDA m : m.k = "block")
    IN  IF blocks = <<>> THEN 0 ELSE blocks[Len(blocks)].v

\* Models: crates/holon-orgmode/src/di.rs:run_file_sync_controller (the
\* rerender_rx arm: drain, on_block_changed_coalesced, set_bulk_pending,
\* queued_renders.done, mark_progress) and
\* crates/holon-filesystem/src/file_sync_controller/writeback.rs:owe_render.
\* Modelling choice: the pass is one step; its in_pass guard covers it whole.
LoopRerender ==
    /\ rrChan /= <<>>
    /\ authority' = IF LastBlock(rrChan) > 0 THEN LastBlock(rrChan) ELSE authority
    /\ bulk' = (bulk \/ \E i \in 1..Len(rrChan) : rrChan[i].k = "all")
    /\ queued' = queued - Len(rrChan)
    /\ rrChan' = <<>>
    /\ tick' = tick + 1
    /\ UNCHANGED <<newest, store, feed, srcPending, folding, foldVer, supChan, handed,
                   base, disk, changed, processed, polled, phase, snap>>

\* Models: crates/holon-orgmode/src/di.rs:run_file_sync_controller (the
\* rerender_flush_tick arm: re_render_all_tracked).
\* Modelling choice: the bulk pass renders the version the block feed holds.
FlushTick ==
    /\ bulk
    /\ bulk' = FALSE
    /\ authority' = feed
    /\ tick' = tick + 1
    /\ UNCHANGED <<newest, store, feed, srcPending, folding, foldVer, supChan, handed,
                   queued, rrChan, base, disk, changed, processed, polled,
                   phase, snap>>

\* Models: crates/holon-orgmode/src/di.rs:run_file_sync_controller (the
\* writeback_net.fired arm) and
\* crates/holon-filesystem/src/file_sync_controller/writeback.rs:drive_writeback_net
\* (CatchUpWrite: write the owed render, then Base moves to it). The write
\* broadcasts a file change that the watcher delivers later.
NetWrite ==
    /\ authority /= base
    /\ disk' = authority
    /\ base' = authority
    /\ changed' = changed + 1
    /\ tick' = tick + 1
    /\ UNCHANGED <<newest, store, feed, srcPending, folding, foldVer, supChan, handed,
                   queued, rrChan, bulk, authority, processed, polled, phase,
                   snap>>

\* Echo suppression. Models: crates/holon-orgmode/src/di.rs:run_file_sync_controller
\* (the file_rx arm: on_file_changed, mark_progress, advance_change_seq) and
\* crates/holon-filesystem/src/file_sync_controller/writeback.rs:read_disk.
\* The read-back reads the file as it is now; a file at its Base is Holon's
\* own write, any other content is ingested into the store.
FileEvent ==
    /\ processed < changed
    /\ processed' = processed + 1
    /\ store' = IF disk /= base THEN disk ELSE store
    /\ tick' = tick + 1
    /\ UNCHANGED <<newest, feed, srcPending, folding, foldVer, supChan, handed, queued,
                   rrChan, bulk, authority, base, disk, changed, polled, phase,
                   snap>>

\* Models: crates/holon-orgmode/src/di.rs:poll_pass (polled := the processed
\* seq the pass started at; a re-stat of a file at its Base ingests nothing).
PollPass ==
    /\ polled < processed
    /\ polled' = processed
    /\ store' = IF disk /= base THEN disk ELSE store
    /\ UNCHANGED <<newest, feed, srcPending, folding, foldVer, supChan, handed, queued,
                   rrChan, bulk, authority, base, disk, changed, processed,
                   tick, phase, snap>>

\* Models: crates/holon-integration-tests/src/pbt/convergence.rs:converge_signals
\* (the first read).
SettleStart ==
    /\ phase = "idle"
    /\ phase' = "poll"
    /\ snap' = Read
    /\ UNCHANGED <<newest, store, feed, srcPending, folding, foldVer, supChan, handed,
                   queued, rrChan, bulk, authority, base, disk, changed,
                   processed, polled, tick>>

\* Models: crates/holon-integration-tests/src/pbt/convergence.rs:converge_signals
\* (settled = two consecutive reads, nothing owed, nothing moved).
\* Modelling choice: one read is one atomic snapshot, and the quiet floor of
\* the pre-fix design is one more read: a delayed task stays delayed for any
\* number of polls on a loaded host.
SettlePoll ==
    /\ phase = "poll"
    /\ IF Quiet /\ Read = snap
         THEN phase' = "returned" /\ UNCHANGED snap
         ELSE snap' = Read /\ UNCHANGED phase
    /\ UNCHANGED <<newest, store, feed, srcPending, folding, foldVer, supChan, handed,
                   queued, rrChan, bulk, authority, base, disk, changed,
                   processed, polled, tick>>

\* Modelling choice: the test reads the file in the "returned" state, then
\* goes on with its next transition.
TestRead ==
    /\ phase = "returned"
    /\ phase' = "idle"
    /\ UNCHANGED <<newest, store, feed, srcPending, folding, foldVer, supChan, handed,
                   queued, rrChan, bulk, authority, base, disk, changed,
                   processed, polled, tick, snap>>

System == FeedApply \/ FoldTake \/ FoldHand \/ ConsumerRoute \/ LoopRerender
          \/ FlushTick \/ NetWrite \/ FileEvent \/ PollPass

Next == EditorWrite \/ System \/ SettleStart \/ SettlePoll \/ TestRead

Fairness ==
    /\ WF_vars(FeedApply) /\ WF_vars(FoldTake) /\ WF_vars(FoldHand)
    /\ WF_vars(ConsumerRoute) /\ WF_vars(LoopRerender) /\ WF_vars(FlushTick)
    /\ WF_vars(NetWrite) /\ WF_vars(FileEvent) /\ WF_vars(PollPass)
    /\ WF_vars(SettlePoll) /\ WF_vars(TestRead)

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
P == INSTANCE SettleProperties WITH
        output <- disk,
        settling <- (phase = "poll"),
        returned <- (phase = "returned")

SettledReadsNewest     == P!SettledReadsNewest
EveryOwedRenderWritten == P!EveryOwedRenderWritten
SettleReturns          == P!SettleReturns
================================================================================
