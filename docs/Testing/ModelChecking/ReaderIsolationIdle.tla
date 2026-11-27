------------------------ MODULE ReaderIsolationIdle ------------------------
(***************************************************************************)
(* The idle rule of asks: an offered ask has its Answer enabled. One      *)
(* subject; a Single row Item; a take Grab of Item (an effect that may     *)
(* fail retryably, then runs again); the ask of the work Accept and its   *)
(* Answer firing, which reads Item.                                        *)
(*                                                                         *)
(* Shape selects what the ask's situation reads:                          *)
(*   present - Item present (an ordinary arc)                              *)
(*   absent  - Item absent (an Inhibit arc)                                *)
(* Edge selects I2 (wide or narrow). HeldPresent is the ruling that an   *)
(* ask's situation and the Answer's bind read a held token as present.    *)
(* The ask derive does not count the Answer's own Running place as busy.  *)
(***************************************************************************)
EXTENDS Integers

CONSTANTS Shape, Edge, HeldPresent, MaxWrites, MaxAtt, MaxTakes

ASSUME /\ Shape \in {"present", "absent"} /\ Edge \in {"wide", "narrow"}
       /\ HeldPresent \in BOOLEAN /\ MaxWrites \in Nat /\ MaxAtt \in Nat /\ MaxTakes \in Nat

VARIABLES
    row,        \* the token in Item (0: empty)
    ver,        \* Item's version
    nw,         \* writes so far
    g,          \* Grab: state, held token, recorded version, attempts
    a,          \* the Answer firing: idle, or held with what it saw
    answeredAt, \* the Item version at the last committed Answer (none: -1)
    badRead     \* ghost: an Answer committed on a view other than the committed front

vars == <<row, ver, nw, g, a, answeredAt, badRead>>

Init ==
    /\ row = 0 /\ ver = 0 /\ nw = 0
    /\ g = [st |-> "idle", k |-> 0, at |-> 0, att |-> 0, n |-> 0]
    /\ a = [st |-> "idle", seen |-> 0, sver |-> 0]
    /\ answeredAt = -1 /\ badRead = FALSE

Held == g.st \in {"running", "retry"}

Superseded == ver /= g.at

CommittedFront == IF row /= 0 THEN row ELSE IF Held /\ ~Superseded THEN g.k ELSE 0

\* What an ask's situation and the Answer's bind read.
\* Models: crates/petri-workflow/src/exec.rs:1511 at commit 104f32fe (`Reader::committed`).
AskView == IF HeldPresent THEN CommittedFront ELSE row

\* I2: does a reader that saw `seen` depend on the open take?
DependsOn(seen) ==
    /\ Held
    /\ IF Edge = "wide" THEN TRUE ELSE seen = 0 /\ ~Superseded

Situation == IF Shape = "present" THEN AskView /= 0 ELSE AskView = 0

Offered == Situation /\ answeredAt /= ver

Write ==
    /\ nw < MaxWrites
    /\ row' = nw + 1 /\ nw' = nw + 1 /\ ver' = ver + 1
    /\ UNCHANGED <<g, a, answeredAt, badRead>>

Take ==
    /\ g.st = "idle" /\ row /= 0 /\ g.n < MaxTakes
    /\ g' = [st |-> "running", k |-> row, at |-> ver, att |-> 1, n |-> g.n + 1]
    /\ row' = 0
    /\ UNCHANGED <<ver, nw, a, answeredAt, badRead>>

\* The ask is answered: the Answer binds; it commits, or is held on its edge.
Answer ==
    /\ Offered /\ a.st = "idle"
    /\ IF DependsOn(AskView)
       THEN /\ a' = [st |-> "held", seen |-> AskView, sver |-> ver]
            /\ UNCHANGED <<answeredAt, badRead>>
       ELSE /\ answeredAt' = ver
            /\ badRead' = (badRead \/ AskView /= CommittedFront)
            /\ UNCHANGED a
    /\ UNCHANGED <<row, ver, nw, g>>

\* A held Answer is released by the take's end: it commits, or is stale and goes back.
Release(newver) ==
    IF a.st = "held"
    THEN LET sv == IF a.sver = ver THEN newver ELSE a.sver
         IN /\ a' = [a EXCEPT !.st = "idle"]
            /\ answeredAt' = IF sv = newver THEN newver ELSE answeredAt
    ELSE UNCHANGED <<a, answeredAt>>

Done ==
    /\ g.st = "running"
    /\ ver' = IF Superseded THEN ver ELSE ver + 1
    /\ g' = [g EXCEPT !.st = "idle"]
    /\ Release(ver')
    /\ UNCHANGED <<row, nw, badRead>>

FailRetryable ==
    /\ g.st = "running" /\ g.att < MaxAtt
    /\ g' = [g EXCEPT !.st = "retry"]
    /\ UNCHANGED <<row, ver, nw, a, answeredAt, badRead>>

RetryDue ==
    /\ g.st = "retry"
    /\ g' = [g EXCEPT !.st = "running", !.att = @ + 1]
    /\ UNCHANGED <<row, ver, nw, a, answeredAt, badRead>>

\* A held Answer has an edge only while the take is open, so a restore makes it stale.
Fail ==
    /\ g.st = "running"
    /\ g' = [g EXCEPT !.st = "idle"]
    /\ IF Superseded THEN UNCHANGED <<row, ver>> ELSE row' = g.k /\ ver' = ver + 1
    /\ IF a.st = "held" /\ ~Superseded
       THEN a' = [a EXCEPT !.st = "idle"] /\ UNCHANGED answeredAt
       ELSE Release(ver')
    /\ UNCHANGED <<nw, badRead>>

Settled == ~Held /\ a.st = "idle"

Terminate == Settled /\ UNCHANGED vars

Next == Write \/ Take \/ Answer \/ Done \/ FailRetryable \/ RetryDue \/ Fail \/ Terminate

Spec == Init /\ [][Next]_vars

\* D151.a: an offered ask has its Answer enabled.
IdleRule == Offered => a.st = "idle"
AnswerNeverWaits == a.st = "idle"
I6Sound == ~badRead

\* Witness: an Answer commits.
NeverAnswered == answeredAt = -1
================================================================================
