----------------------- MODULE ReaderIsolationFifoAsk -----------------------
(***************************************************************************)
(* An ask work on a FIFO row whose front an open take holds. One subject; *)
(* a FIFO P0; T0, an auto Keyed take of P0 that retries; W0, an ask work   *)
(* that consumes P0 and whose situation reads the front of P0. The ask's  *)
(* situation reads the committed view (a held token counts as present);   *)
(* the user answers about the token it shows.                             *)
(*                                                                         *)
(* Bind selects how W0 binds:                                              *)
(*   current - W0's take pops the visible head                             *)
(*   d       - W0 is enabled only while no open take holds the committed  *)
(*             front; it then takes the committed front                    *)
(*   b       - as current, but the ask is hidden while a take holds the   *)
(*             front                                                       *)
(* Mech selects the executor's guards on T0:                              *)
(*   executor - the failure mark and the attempt bound                     *)
(*   nomark   - the attempt bound only (witness: W0 can starve)           *)
(*   none     - neither (witness: W0 can starve)                           *)
(***************************************************************************)
EXTENDS Naturals, Sequences

CONSTANTS Bind, Mech, MaxWrites, MaxAtt

ASSUME /\ Bind \in {"current", "d", "b"} /\ Mech \in {"executor", "nomark", "none"}
       /\ MaxWrites \in Nat /\ MaxAtt \in Nat

VARIABLES
    row,      \* P0's visible tokens, oldest first; tokens are numbered by their write
    nw,       \* writes so far
    t0,       \* T0: state, held token, attempts, whether an attempt ended Unknown, the token its last definite failure marked
    answered, \* the token the answered situation saw (0: not answered)
    w         \* W0: none, done (with the token it consumed) or expired

vars == <<row, nw, t0, answered, w>>

Init ==
    /\ row = <<>> /\ nw = 0
    /\ t0 = [st |-> "idle", k |-> 0, att |-> 0, unk |-> FALSE, mark |-> 0]
    /\ answered = 0
    /\ w = [st |-> "none", k |-> 0]

Held == t0.st /= "idle"

CommittedFront == IF Held THEN t0.k ELSE IF row = <<>> THEN 0 ELSE Head(row)

\* Models: crates/petri-workflow/src/exec.rs:4324 at commit 104f32fe (a
\* retryable failure retries only while attempts < max_attempts).
AttemptLeft == Mech = "none" \/ t0.att < MaxAtt
NextAtt == IF t0.att < MaxAtt THEN t0.att + 1 ELSE t0.att

Write ==
    /\ nw < MaxWrites
    /\ row' = Append(row, nw + 1) /\ nw' = nw + 1
    /\ UNCHANGED <<t0, answered, w>>

\* Models: crates/petri-workflow/src/exec.rs:3509-3515 at commit 104f32fe
\* (`decide`: no Auto re-fire on the read trace of a definite failure,
\* `FailedOnThisInput`).
Take ==
    /\ t0.st = "idle" /\ row /= <<>>
    /\ Mech /= "executor" \/ Head(row) /= t0.mark
    /\ t0' = [st |-> "running", k |-> Head(row), att |-> 1, unk |-> FALSE, mark |-> t0.mark]
    /\ row' = Tail(row)
    /\ UNCHANGED <<nw, answered, w>>

Done ==
    /\ t0.st = "running"
    /\ t0' = [t0 EXCEPT !.st = "idle"]
    /\ UNCHANGED <<row, nw, answered, w>>

FailRetryable ==
    /\ t0.st = "running" /\ AttemptLeft
    /\ t0' = [t0 EXCEPT !.st = "retry"]
    /\ UNCHANGED <<row, nw, answered, w>>

RetryDue ==
    /\ t0.st = "retry"
    /\ t0' = [t0 EXCEPT !.st = "running", !.att = NextAtt]
    /\ UNCHANGED <<row, nw, answered, w>>

\* Occurrence rule: after an Unknown attempt, a failure parks the take.
\* Models: crates/petri-workflow/src/exec.rs:4281, 4298 at commit 104f32fe
\* (`fail` calls `set_mark`: a definite failure marks the run's read trace).
Fail ==
    /\ t0.st = "running"
    /\ IF t0.unk
       THEN t0' = [t0 EXCEPT !.st = "parked"] /\ UNCHANGED row
       ELSE t0' = [t0 EXCEPT !.st = "idle", !.mark = t0.k] /\ row' = <<t0.k>> \o row
    /\ UNCHANGED <<nw, answered, w>>

Unknown ==
    /\ t0.st = "running"
    /\ t0' = [t0 EXCEPT !.st = "parked", !.unk = TRUE]
    /\ UNCHANGED <<row, nw, answered, w>>

ResolveCommit ==
    /\ t0.st = "parked"
    /\ t0' = [t0 EXCEPT !.st = "idle"]
    /\ UNCHANGED <<row, nw, answered, w>>

ResolveRetry ==
    /\ t0.st = "parked" /\ AttemptLeft
    /\ t0' = [t0 EXCEPT !.st = "running", !.att = NextAtt]
    /\ UNCHANGED <<row, nw, answered, w>>

Offered ==
    /\ answered = 0 /\ w.st = "none" /\ CommittedFront /= 0
    /\ Bind = "b" => ~Held

Answer ==
    /\ Offered
    /\ answered' = CommittedFront
    /\ UNCHANGED <<row, nw, t0, w>>

\* W0's situation hash no longer matches: the work expires.
Expire ==
    /\ answered /= 0 /\ w.st = "none" /\ CommittedFront /= answered
    /\ w' = [w EXCEPT !.st = "expired"]
    /\ UNCHANGED <<row, nw, t0, answered>>

\* Models: crates/petri-workflow/src/exec.rs:3539 at commit 104f32fe (rule
\* (d): an asked work is not enabled while `front_taker` holds the front).
Work ==
    /\ answered /= 0 /\ w.st = "none" /\ CommittedFront = answered
    /\ row /= <<>>
    /\ Bind = "d" => ~Held
    /\ w' = [st |-> "done", k |-> Head(row)]
    /\ row' = Tail(row)
    /\ UNCHANGED <<nw, t0, answered>>

Settled == ~Held /\ (w.st /= "none" \/ answered = 0)

Terminate == Settled /\ UNCHANGED vars

Next ==
    \/ Write \/ Take \/ Done \/ FailRetryable \/ RetryDue \/ Fail \/ Unknown
    \/ ResolveCommit \/ ResolveRetry \/ Answer \/ Expire \/ Work \/ Terminate

\* Weak fairness only:
\* - a running effect completes: the runner replies, or the run timer ends it;
\* - a due retry runs: `step` fires the timers (exec.rs:2853 at commit 104f32fe);
\* - an enabled W0 fires: `quiesce` fires every enabled auto transition
\*   until none is enabled (exec.rs:2874 at commit 104f32fe).
Fairness ==
    /\ WF_vars(Done \/ FailRetryable \/ Fail \/ Unknown) /\ WF_vars(RetryDue)
    /\ WF_vars(Expire) /\ WF_vars(Work)

Spec == Init /\ [][Next]_vars /\ Fairness

\* Each committed ask work consumed the token that its answered situation saw.
AskConsumesWhatItSaw == w.st = "done" => w.k = answered

\* An answered work settles once T0 is resolved; without resolve it may wait for ever.
WorkSettles == answered /= 0 ~> w.st /= "none"
WorkSettlesWhenResolved == WF_vars(ResolveCommit) => WorkSettles

\* Witnesses: each must be violated.
NeverDone == w.st /= "done"
NeverWaitsParked == ~(answered /= 0 /\ w.st = "none" /\ t0.st = "parked")
================================================================================
