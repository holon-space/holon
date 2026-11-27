------------------------------ MODULE SettleWake ------------------------------
(***************************************************************************)
(* A windowed test settles: the waiter checks that the frame reflects      *)
(* every emitted CDC batch and that nothing is owed, and between failed    *)
(* checks it waits on UiWatermark::changed (a counter that every wake      *)
(* source bumps). One subscription, one owed child render.                 *)
(*                                                                         *)
(* Waiter selects the design:                                              *)
(*   "spin"   - ffa1611f: settle_window re-checks at once, never parks     *)
(*   "fixed"  - 813caed2: subscribe, then check, then park; every source   *)
(*              wakes, the OwedChild drop included                         *)
(*   "nowake" - the waiter of "fixed" with the wake sources of 7af013ce:   *)
(*              dropping an OwedChild bumps no counter                     *)
(*   "late"   - the waiter checks, then subscribes, then parks             *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS Writes, Waiter

ASSUME Writes \in Nat /\ Waiter \in {"spin", "fixed", "nowake", "late"}

VARIABLES
    newest,     \* the last emitted CDC batch (the newest render owed)
    applied,    \* the last batch the subscription's consumer applied
    owedChild,  \* a ui_watcher render holds an OwedChild of the subscription
    shown,      \* the batch the window's frame reflects
    changes,    \* UiWatermark::changed counter
    sub,        \* the counter value the waiter subscribed at
    wphase      \* "idle", "check", "arm", "parked", "returned"

vars == <<newest, applied, owedChild, shown, changes, sub, wphase>>

Settling == wphase \in {"check", "arm", "parked"}

Init ==
    /\ newest = 0 /\ applied = 0 /\ owedChild = FALSE /\ shown = 0
    /\ changes = 0 /\ sub = 0 /\ wphase = "idle"

\* Modelling choice: any engine write that emits a CDC batch, and its wake.
\* The test emits only between settles, as a PBT transition does.
Emit ==
    /\ wphase = "idle"
    /\ newest < Writes
    /\ newest' = newest + 1
    /\ changes' = changes + 1
    /\ UNCHANGED <<applied, owedChild, shown, sub, wphase>>

\* Models: crates/holon-api/src/live_data.rs:DeliveryCursor::applied_through and
\* crates/holon/src/api/ui_watcher.rs:begin_render (the render takes an
\* OwedChild via DeliveryCursor::owed_child). The apply wakes.
Apply ==
    /\ applied < newest
    /\ applied' = newest
    /\ owedChild' = TRUE
    /\ changes' = changes + 1
    /\ UNCHANGED <<newest, shown, sub, wphase>>

\* Models: crates/holon-api/src/live_data.rs:OwedChild::drop (the render ended;
\* the frame now reflects it). At ffa1611f and 7af013ce the drop bumps
\* nothing; at db6074bc it bumps the root cursor's release_wake.
ChildEnd ==
    /\ owedChild
    /\ owedChild' = FALSE
    /\ shown' = applied
    /\ changes' = IF Waiter = "nowake" THEN changes ELSE changes + 1
    /\ UNCHANGED <<newest, applied, sub, wphase>>

SettleStart ==
    /\ wphase = "idle"
    /\ wphase' = "check"
    /\ UNCHANGED <<newest, applied, owedChild, shown, changes, sub>>

Done == shown >= newest /\ ~owedChild

\* Models: frontends/gpui/tests/pbt_harness/settle.rs:settle_window (the
\* check: epoch.reflects >= target and watermark.owed() is empty). "fixed"
\* and "nowake" subscribe at the top of the iteration, before the check.
Check ==
    /\ wphase = "check"
    /\ IF Done
         THEN wphase' = "returned" /\ UNCHANGED sub
         ELSE CASE Waiter = "spin" -> UNCHANGED <<wphase, sub>>
                [] Waiter = "late" -> wphase' = "arm" /\ UNCHANGED sub
                [] OTHER           -> wphase' = "parked" /\ sub' = changes
    /\ UNCHANGED <<newest, applied, owedChild, shown, changes>>

\* Modelling choice: ffa1611f has no change wait. The subscription taken
\* after the check is the shape of the subscribe-late mutant of
\* settle_window_until in the open settle-wake diff.
Arm ==
    /\ wphase = "arm"
    /\ wphase' = "parked"
    /\ sub' = changes
    /\ UNCHANGED <<newest, applied, owedChild, shown, changes>>

\* Modelling choice: ffa1611f has no change wait; this is await_change in
\* frontends/gpui/tests/pbt_harness/settle.rs of the open settle-wake diff.
Wake ==
    /\ wphase = "parked"
    /\ changes > sub
    /\ wphase' = "check"
    /\ UNCHANGED <<newest, applied, owedChild, shown, changes, sub>>

\* Modelling choice: the test reads the frame in the "returned" state.
TestRead ==
    /\ wphase = "returned"
    /\ wphase' = "idle"
    /\ UNCHANGED <<newest, applied, owedChild, shown, changes, sub>>

Next == Emit \/ Apply \/ ChildEnd \/ SettleStart \/ Check \/ Arm \/ Wake \/ TestRead

Spec ==
    /\ Init /\ [][Next]_vars
    /\ WF_vars(Apply) /\ WF_vars(ChildEnd) /\ WF_vars(Check) /\ WF_vars(Arm)
    /\ WF_vars(Wake) /\ WF_vars(TestRead)

P == INSTANCE SettleProperties WITH
        output <- shown,
        settling <- Settling,
        returned <- (wphase = "returned")

SettledReadsNewest     == P!SettledReadsNewest
EveryOwedRenderWritten == P!EveryOwedRenderWritten
SettleReturns          == P!SettleReturns
================================================================================
