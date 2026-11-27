---------------------- MODULE ReaderIsolationSupersede ----------------------
(***************************************************************************)
(* The supersede test of a restore, on one Single row P0 with two takes   *)
(* (T0, T2) and one reader T1. A restore puts the held token back only if  *)
(* the take is not superseded; the edge test (narrow I2) and the committed *)
(* front use the same definition.                                          *)
(*                                                                         *)
(* Rule selects the supersede test:                                        *)
(*   version - the row version is not the version the take recorded       *)
(*   slot    - the row holds a token, or another open take holds one       *)
(* Case "free" explores every order; any other Case runs its script.      *)
(***************************************************************************)
EXTENDS Naturals, Sequences

CONSTANTS Rule, MaxWrites, Case

ASSUME Rule \in {"version", "slot"} /\ MaxWrites \in Nat

Takes == {"T0", "T2"}

VARIABLES
    row,      \* the token in P0 (0: empty); tokens are numbered by their write
    ver,      \* P0's version
    nw,       \* writes so far (the newest written token)
    tk,       \* tk[t]: the take's state, held token and recorded version
    r,        \* the reader T1
    restored, \* ghost: a restore put a token back
    revived,  \* ghost: a restore put back a token older than a written one
    badRead,  \* ghost: T1 committed not stale on a view other than the committed front
    pc

vars == <<row, ver, nw, tk, r, restored, revived, badRead, pc>>

Script ==
    CASE Case = "superseded_token_stays_out_when_a_later_take_holds_the_newer_one" ->
            <<"write", "take_T0", "write", "bind", "take_T2", "fail_T0", "done_T2", "record">>
      [] Case = "superseded_token_stays_out_after_the_newer_one_is_consumed" ->
            <<"write", "take_T0", "write", "take_T2", "bind", "done_T2", "fail_T0", "record">>
      [] Case = "free" -> <<>>

Expected ==
    CASE Case = "superseded_token_stays_out_when_a_later_take_holds_the_newer_one" ->
            row = 0 /\ r.verdict = "stale"
      [] Case = "superseded_token_stays_out_after_the_newer_one_is_consumed" ->
            row = 0 /\ r.verdict = "notstale"
      [] Case = "free" -> TRUE

Op(name) ==
    IF Case = "free" THEN UNCHANGED pc
    ELSE pc <= Len(Script) /\ Script[pc] = name /\ pc' = pc + 1

Init ==
    /\ row = 0 /\ ver = 0 /\ nw = 0
    /\ tk = [t \in Takes |-> [st |-> "idle", k |-> 0, at |-> 0]]
    /\ r = [st |-> "idle", seen |-> 0, sver |-> 0, edges |-> {}, verdict |-> "none"]
    /\ restored = FALSE /\ revived = FALSE /\ badRead = FALSE
    /\ pc = 1

Open(t) == tk[t].st = "running"

\* Models: crates/petri-workflow/src/exec.rs:3750 at commit 104f32fe (`superseded`).
Superseded(t) ==
    IF Rule = "version" THEN ver /= tk[t].at
    ELSE row /= 0 \/ \E u \in Takes \ {t} : Open(u)

\* I2, narrow: the take's failure could put its token back into what T1 saw.
DependsOn(t, seen) == Open(t) /\ seen = 0 /\ ~Superseded(t)

CommittedFront ==
    LET live == {tk[t].k : t \in {u \in Takes : Open(u) /\ ~Superseded(u)}}
    IN IF row /= 0 THEN row
       ELSE IF live = {} THEN 0
       ELSE CHOOSE k \in live : \A j \in live : j <= k

Write ==
    /\ nw < MaxWrites
    /\ row' = nw + 1 /\ nw' = nw + 1 /\ ver' = ver + 1
    /\ Op("write")
    /\ UNCHANGED <<tk, r, restored, revived, badRead>>

Take(t) ==
    /\ tk[t].st = "idle" /\ row /= 0
    /\ tk' = [tk EXCEPT ![t] = [st |-> "running", k |-> row, at |-> ver]]
    /\ row' = 0
    /\ Op("take_" \o t)
    /\ UNCHANGED <<ver, nw, r, restored, revived, badRead>>

Bind ==
    /\ r.st = "idle"
    /\ r' = [st |-> "bound", seen |-> row, sver |-> ver,
             edges |-> {t \in Takes : DependsOn(t, row)}, verdict |-> "none"]
    /\ Op("bind")
    /\ UNCHANGED <<row, ver, nw, tk, restored, revived, badRead>>

\* H1: the version moves only if the held token counted in the committed row.
Done(t) ==
    /\ Open(t)
    /\ ver' = IF Superseded(t) THEN ver ELSE ver + 1
    /\ tk' = [tk EXCEPT ![t].st = "done"]
    /\ r' = IF t \in r.edges
            THEN [r EXCEPT !.edges = @ \ {t}, !.sver = IF @ = ver THEN ver' ELSE @]
            ELSE r
    /\ Op("done_" \o t)
    /\ UNCHANGED <<row, nw, restored, revived, badRead>>

\* H2: a token put back moves the version, so a reader of the hole is stale.
Fail(t) ==
    /\ Open(t)
    /\ tk' = [tk EXCEPT ![t].st = "done"]
    /\ IF Superseded(t)
       THEN UNCHANGED <<row, ver, restored, revived>>
       ELSE /\ row' = tk[t].k /\ ver' = ver + 1 /\ restored' = TRUE
            /\ revived' = (revived \/ tk[t].k < nw)
    /\ r' = [r EXCEPT !.edges = @ \ {t}]
    /\ Op("fail_" \o t)
    /\ UNCHANGED <<nw, badRead>>

Record ==
    /\ r.st = "bound" /\ r.edges = {}
    /\ r' = [r EXCEPT !.st = "done", !.verdict = IF ver = r.sver THEN "notstale" ELSE "stale"]
    /\ badRead' = (badRead \/ (ver = r.sver /\ r.seen /= CommittedFront))
    /\ Op("record")
    /\ UNCHANGED <<row, ver, nw, tk, restored, revived>>

Settled ==
    IF Case = "free" THEN (\A t \in Takes : ~Open(t)) /\ r.st /= "bound"
    ELSE pc > Len(Script)

Terminate == Settled /\ UNCHANGED vars

Next ==
    \/ Write \/ Bind \/ Record \/ Terminate
    \/ \E t \in Takes : Take(t) \/ Done(t) \/ Fail(t)

Spec == Init /\ [][Next]_vars

\* A restore never brings back a superseded token.
NoRevive == ~revived
I6Sound == ~badRead
CaseHolds == pc > Len(Script) => Expected

\* Witnesses: each must be violated.
NeverRestored == ~restored
NeverStale == r.verdict /= "stale"
NeverNotStale == r.verdict /= "notstale"
ScriptNeverEnds == pc <= Len(Script)
================================================================================
