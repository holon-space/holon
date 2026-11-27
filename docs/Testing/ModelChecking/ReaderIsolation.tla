---------------------------- MODULE ReaderIsolation ----------------------------
(***************************************************************************)
(* Reader isolation during a take (ruling C, with the parked-take rule).   *)
(*                                                                         *)
(* Per subject s: the taker T0 consumes the front token of each row in     *)
(* Rows (P0, and P2 for a take of two rows); the reader T1 consumes P1[s]  *)
(* and reads the rows that T0 takes. T0's body is a Keyed effect with      *)
(* retries; T1's body is pure or a Keyed effect, with an optional fact     *)
(* output into one of the rows.                                            *)
(*                                                                         *)
(* The executor is single-threaded: an outside operation (write, complete, *)
(* step, resolve) runs only when the net is quiet, and the internal steps  *)
(* (start, bind, record) run until it is quiet again.                      *)
(*                                                                         *)
(* Written from the rulings and invariants I1-I7, not from the executor    *)
(* or its test model. Mutant names one deliberately broken design rule;    *)
(* "none" is the ruled design.                                             *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Subj,       \* subjects
    Cap,        \* capacity of P0: "Single", "Register" or "Fifo"; P2 is Single
    TwoRows,    \* TRUE: one take over the two rows P0 and P2
    Taker,      \* TRUE: T0 exists
    ReadKind,   \* "field": T1 reads the rows; "running": T1 reads T0.Running
    Body,       \* T1's body: "pure" or "keyed"
    FactRow,    \* "none" or the row T1 puts a fact into when it commits
    MaxAtt,     \* attempts of one T0 occurrence before it parks or fails
    MaxWrites,  \* outside writes per subject in a free run
    Xs,         \* field values an outside write draws in a free run
    Edge,       \* the I2 variant: "narrow" (ruled) or "wide"
    Mutant,     \* "none", "nowait", "nostale", "noadvance", "keepedge",
                \* "nocond", "oldtrace", "oldcond", "holdfirst", "staleatrecord"
    Case        \* "free", or the name of a static example (CaseScript)

Rows == IF TwoRows THEN {"P0", "P2"} ELSE {"P0"}
T1Rows == IF ReadKind = "field" THEN Rows ELSE {}
CapOf(r) == IF r = "P0" THEN Cap ELSE "Single"

ASSUME
    /\ Cap \in {"Single", "Register", "Fifo"}
    /\ ReadKind \in {"field", "running"}
    /\ Body \in {"pure", "keyed"}
    /\ FactRow \in {"none"} \cup T1Rows
    /\ Edge \in {"narrow", "wide"}
    /\ Mutant \in {"none", "nowait", "nostale", "noadvance", "keepedge", "nocond", "oldtrace",
                   "oldcond", "holdfirst", "staleatrecord"}
    /\ MaxAtt \in Nat \ {0}

NoTok == [id |-> 0, x |-> "none"]

VARIABLES
    row,    \* row[s][r]: the tokens of row r, oldest first
    ver,    \* ver[s][r]: the row's version; moves when its committed content moves
    nid,    \* nid[s]: next token id of subject s (ids are never reused)
    p1,     \* p1[s]: P1[s] holds a token
    w,      \* w[s]: outside writes so far (free run bound)
    t0,     \* t0[s]: T0's occurrence for s
    t1,     \* t1[s]: T1's occurrence for s
    mark1,  \* mark1[s]: T1 failed on the current P1 token
    cond,   \* the subjects s with a Condition "T1[s] waits for T0[s]"
    first,  \* first[s]: how T1's first occurrence for s settled
    ev,     \* the last record of T1 (ghost, for the invariants)
    pc      \* index of the next operation of the case script

vars == <<row, ver, nid, p1, w, t0, t1, mark1, cond, first, ev, pc>>

-----------------------------------------------------------------------------
(* Static examples: the operations of each case test, in order. Each      *)
(* operation is a record: k "w" write (r, x), "c0"/"c1" complete T0/T1     *)
(* (res), "step", "r0"/"r1" resolve T0/T1 (how). Checks: after operation   *)
(* `at`, once the net is quiet, `kind` holds for subject s.                *)

W(s, r, x) == [k |-> "w", s |-> s, r |-> r, x |-> x]
C0(s, res) == [k |-> "c0", s |-> s, res |-> res]
C1(s, res) == [k |-> "c1", s |-> s, res |-> res]
STEP(s) == [k |-> "step", s |-> s]
R0(s, how) == [k |-> "r0", s |-> s, how |-> how]

Chk(at, kind, s, v) == [at |-> at, kind |-> kind, s |-> s, v |-> v]

GOps == <<W("s", "P0", "none"), W("s", "P0", "f"), W("s", "P1", "none"),
          C0("s", "done"), C1("s", "done")>>

\* T1 binds while T0 holds the token (read_while), or after T0's retryable
\* failure put it back and before the retry takes it again.
Beside(readWhile, res) ==
    IF readWhile
    THEN <<W("s", "P0", "none"), W("s", "P1", "none"), C0("s", res), C1("s", "done")>>
    ELSE <<W("s", "P0", "none"), C0("s", "retryable"), W("s", "P1", "none"),
           STEP("s"), C0("s", res), C1("s", "done")>>

ParkSetup ==
    <<W("s", "P0", "none"), W("s", "P1", "none"), W("s2", "P0", "none"), W("s2", "P1", "none"),
      C0("s", "unknown"), C0("s2", "unknown"), C0("s", "unknown"), C0("s2", "unknown"),
      C1("s", "done"), C1("s2", "done")>>

ParkChecks == {Chk(10, "cond", "s", 0), Chk(10, "cond", "s2", 0),
               Chk(10, "held", "s", 0), Chk(10, "held", "s2", 0)}

\* After the resolves: s committed, its Condition gone; s2 still waits.
ParkEnd(at) == {Chk(at, "verdict", "s", "done"), Chk(at, "nocond", "s", 0),
                Chk(at, "cond", "s2", 0), Chk(at, "held", "s2", 0)}

CaseScript(c) ==
    CASE c = "held_token_superseded_by_a_write" ->
            <<GOps[1], GOps[2], GOps[3], GOps[4], W("s", "P0", "t"), GOps[5]>>
      [] c = "absent_read_then_written_and_taken" ->
            <<W("s", "P1", "none"), W("s", "P0", "none"), C1("s", "done")>>
      [] c = "dirty_absent_read_records_before_the_taker_fails" ->
            <<W("s", "P0", "none"), W("s", "P1", "none"), C1("s", "done"), C0("s", "definite")>>
      [] c = "read_of_a_token_written_during_a_take" -> GOps
      [] c = "run_state_read_then_the_run_starts" ->
            <<W("s", "P1", "none"), W("s", "P0", "none"), C1("s", "done")>>
      [] c \in {"fact_into_fifo_appended_behind_head", "fifo_append_behind_the_read_head"} ->
            <<W("s", "P0", "none"), W("s", "P1", "none"), W("s", "P0", "t"), C1("s", "done")>>
      [] c = "read_then_taken_taker_runs" -> Beside(FALSE, "unknown")
      [] c = "read_then_taken_taker_done" -> Beside(FALSE, "done")
      [] c = "read_then_taken_taker_fails" -> Beside(FALSE, "definite")
      [] c = "read_while_taken_taker_done" -> Beside(TRUE, "done")
      [] c = "read_while_taken_taker_fails" -> Beside(TRUE, "definite")
      [] c = "read_while_taken_taker_retries" -> Beside(TRUE, "retryable")
      [] c = "present_fifo_read_whose_held_front_goes_back" ->
            <<W("s", "P0", "none"), W("s", "P0", "t"), W("s", "P1", "none"),
              C0("s", "definite"), C1("s", "done")>>
      [] c = "a_stale_reader_puts_no_fact_over_the_restored_token" -> Beside(TRUE, "definite")
      [] c = "reader_waits_on_a_parked_take_verify" -> ParkSetup \o <<R0("s", "verify")>>
      [] c = "reader_waits_on_a_parked_take_drop" -> ParkSetup \o <<R0("s", "drop")>>
      [] c = "reader_waits_on_a_parked_take_retry_then_fails" ->
            ParkSetup \o <<R0("s", "retry"), C0("s", "definite"), R0("s", "drop")>>
      [] OTHER -> <<>>

CaseChecks(c) ==
    LET n == Len(CaseScript(c)) IN
    CASE c = "free" -> {}
      [] c = "dirty_absent_read_records_before_the_taker_fails" ->
            {Chk(3, "held", "s", 0), Chk(4, "notheld", "s", 0), Chk(4, "verdict", "s", "stale")}
      [] c = "read_of_a_token_written_during_a_take" ->
            {Chk(3, "noedge", "s", 0), Chk(5, "verdict", "s", "done")}
      [] c = "read_while_taken_taker_done" ->
            {Chk(2, "edge", "s", 0), Chk(4, "verdict", "s", "done")}
      [] c = "a_stale_reader_puts_no_fact_over_the_restored_token" ->
            {Chk(4, "verdict", "s", "stale"), Chk(4, "p0", "s", "none")}
      [] c = "reader_waits_on_a_parked_take_verify" -> ParkChecks \cup ParkEnd(11)
      [] c = "reader_waits_on_a_parked_take_drop" -> ParkChecks \cup ParkEnd(11)
      [] c = "reader_waits_on_a_parked_take_retry_then_fails" ->
            ParkChecks \cup ParkEnd(13) \cup
            {Chk(11, "nocond", "s", 0), Chk(11, "held", "s", 0),
             Chk(12, "cond", "s", 0), Chk(12, "held", "s", 0)}
      [] c \in {"absent_read_then_written_and_taken", "held_token_superseded_by_a_write",
                "fact_into_fifo_appended_behind_head", "fifo_append_behind_the_read_head",
                "read_then_taken_taker_done", "read_while_taken_taker_fails",
                "read_while_taken_taker_retries", "present_fifo_read_whose_held_front_goes_back"} ->
            {Chk(n, "verdict", "s", "stale")}
      [] OTHER -> {Chk(n, "verdict", "s", "done")}

Script == CaseScript(Case)

-----------------------------------------------------------------------------
Ids(sq) == {sq[i].id : i \in DOMAIN sq}
Front(s, r) == IF row[s][r] = <<>> THEN NoTok ELSE Head(row[s][r])
Min(S) == CHOOSE m \in S : \A n \in S : m <= n
Max(S) == CHOOSE m \in S : \A n \in S : m >= n

\* A restore puts the held token back: always on a FIFO (at its old place),
\* on a Single or Register row only when no newer token was written there.
PutsBack(s, r) == CapOf(r) = "Fifo" \/ row[s][r] = <<>>

RestoreInto(sq, tok, cap) ==
    IF cap = "Fifo"
    THEN SelectSeq(sq, LAMBDA t : t.id < tok.id) \o <<tok>> \o SelectSeq(sq, LAMBDA t : t.id > tok.id)
    ELSE IF sq = <<>> THEN <<tok>> ELSE sq

Holds0(s, r) == t0[s].hold[r] /= NoTok
HoldsAny0(s) == \E r \in Rows : Holds0(s, r)
Live1(s) == t1[s].st \in {"run", "rec", "held", "parked"}
\* T0 holds now the tokens of the same take that was open at T1's bind.
SameTake(s) == HoldsAny0(s) /\ t1[s].tk = t0[s].tk

\* I2, one definition with both variants: does T1, which saw the token
\* `seen` at the front of r, depend on the take that holds a token of r?
\* Narrow: only if the take's failure could put its token back into what
\* T1 saw (an absent row, or a FIFO front behind the held token).
DependsOn(s, r, seen) ==
    \/ Edge = "wide"
    \/ seen = 0
    \/ CapOf(r) = "Fifo" /\ t0[s].hold[r].id < seen

\* The I6 oracle: the row's tokens plus the tokens that open takes hold
\* from it; on a non-FIFO row with an empty head, the newest held token.
CommittedFront(s, r) ==
    LET held == IF Holds0(s, r) THEN {t0[s].hold[r].id} ELSE {}
        inRow == Ids(row[s][r])
    IN IF CapOf(r) = "Fifo"
       THEN IF inRow \cup held = {} THEN 0 ELSE Min(inRow \cup held)
       ELSE IF row[s][r] /= <<>> THEN Head(row[s][r]).id
            ELSE IF held = {} THEN 0 ELSE Max(held)

T0Idle == [st |-> "idle", hold |-> [r \in Rows |-> NoTok], att |-> 0, unk |-> FALSE,
           tk |-> 0, mark |-> [r \in Rows |-> 0]]
T1Idle == [st |-> "idle", n |-> 0, seen |-> [r \in T1Rows |-> 0], ver |-> [r \in T1Rows |-> 0],
           edges |-> {}, stale |-> {}, tk |-> 0, moved |-> {}, deprest |-> FALSE, adv |-> FALSE]
NoEv == [s |-> "none", verdict |-> "none", seen |-> [r \in T1Rows |-> 0],
         front |-> [r \in T1Rows |-> 0], edges |-> {}, moved |-> {}, deprest |-> FALSE,
         adv |-> FALSE, fact |-> "none"]

Init ==
    /\ row = [s \in Subj |-> [r \in Rows |-> <<>>]]
    /\ ver = [s \in Subj |-> [r \in Rows |-> 0]]
    /\ nid = [s \in Subj |-> 1]
    /\ p1 = [s \in Subj |-> FALSE]
    /\ w = [s \in Subj |-> 0]
    /\ t0 = [s \in Subj |-> T0Idle]
    /\ t1 = [s \in Subj |-> T1Idle]
    /\ mark1 = [s \in Subj |-> FALSE]
    /\ cond = {}
    /\ first = [s \in Subj |-> "none"]
    /\ ev = NoEv
    /\ pc = 1

-----------------------------------------------------------------------------
(* Internal steps *)

CanStart0(s) ==
    /\ Taker
    /\ t0[s].st = "idle"
    /\ \A r \in Rows : row[s][r] /= <<>>
    /\ \E r \in Rows : Front(s, r).id /= t0[s].mark[r]

\* Models: crates/petri-workflow/src/exec.rs:start (a take pops the front
\* token of each consumed row into the attempt; the row's version stays).
TakeFronts(s, att, unk) ==
    /\ t0' = [t0 EXCEPT ![s] = [@ EXCEPT !.st = "run", !.hold = [r \in Rows |-> Front(s, r)],
                                         !.att = att, !.unk = unk, !.tk = @ + 1]]
    /\ row' = [row EXCEPT ![s] = [r \in Rows |-> Tail(row[s][r])]]

Start0(s) ==
    /\ CanStart0(s)
    /\ TakeFronts(s, 1, FALSE)
    /\ UNCHANGED <<ver, nid, p1, w, t1, mark1, cond, first, ev, pc>>

CanBind1(s) == t1[s].st = "idle" /\ p1[s] /\ ~mark1[s]

\* Models: crates/petri-workflow/src/exec.rs:decide and start for T1 (the
\* bind reads the rows; I2 builds an edge to each take it depends on).
Bind1(s) ==
    /\ CanBind1(s)
    /\ LET seen == [r \in T1Rows |-> Front(s, r).id]
           edges == {<<r, t0[s].hold[r].id>> : r \in {q \in T1Rows : Holds0(s, q) /\ DependsOn(s, q, seen[q])}}
       IN /\ t1' = [t1 EXCEPT ![s] = [st |-> IF Body = "pure" THEN "rec" ELSE "run",
                                      n |-> IF @.n < 2 THEN @.n + 1 ELSE 2, seen |-> seen,
                                      ver |-> IF Mutant = "oldtrace" /\ @.n > 0 THEN @.ver
                                              ELSE [r \in T1Rows |-> ver[s][r]],
                                      edges |-> edges, stale |-> {},
                                      tk |-> IF HoldsAny0(s) THEN t0[s].tk ELSE 0,
                                      moved |-> {}, deprest |-> FALSE, adv |-> FALSE]]
          /\ cond' = IF edges /= {} /\ t0[s].st = "parked" /\ Mutant /= "nocond"
                     THEN cond \cup {s} ELSE cond
    /\ p1' = [p1 EXCEPT ![s] = FALSE]
    /\ UNCHANGED <<row, ver, nid, w, t0, mark1, first, ev, pc>>

\* The reader's trace no longer matches its rows: a write moved one, or a
\* restore put a token back into one.
StaleNow(s) == t1[s].stale /= {} \/ \E r \in T1Rows : t1[s].ver[r] /= ver[s][r]

\* The stale check runs before the hold: a stale reader waits on no take.
StaleFirst(s) == Mutant /= "holdfirst" /\ StaleNow(s)

\* The reader's wait ends, so its Condition goes.
EndWait(s) == cond' = IF Mutant = "oldcond" THEN cond ELSE cond \ {s}

CanRecord(s) ==
    \/ t1[s].st = "rec"
    \/ t1[s].st = "held" /\ (t1[s].edges = {} \/ (Mutant /= "staleatrecord" /\ StaleFirst(s)))

Settle1(s, v) == first' = [first EXCEPT ![s] = IF t1[s].n = 1 THEN v ELSE @]

\* Models: crates/petri-workflow/src/exec.rs:record (hold while a dependency
\* is open, I3; else the stale check, then commit or the stale path) and the
\* release of held readers at the top of each quiesce round.
Record(s) ==
    /\ CanRecord(s)
    /\ IF Mutant /= "nowait" /\ t1[s].edges /= {} /\ ~StaleFirst(s)
       THEN /\ t1' = [t1 EXCEPT ![s].st = "held"]
            /\ UNCHANGED <<row, ver, nid, p1, first, ev, cond>>
       ELSE LET o == t1[s]
                stale == o.stale /= {} \/ \E r \in T1Rows : o.ver[r] /= ver[s][r]
                lands == ~stale /\ FactRow /= "none"
                fact == [id |-> nid[s], x |-> "t"]
            IN /\ ev' = [s |-> s, verdict |-> IF stale THEN "stale" ELSE "done", seen |-> o.seen,
                         front |-> [r \in T1Rows |-> CommittedFront(s, r)], edges |-> o.edges,
                         moved |-> o.moved, deprest |-> o.deprest, adv |-> o.adv,
                         fact |-> IF FactRow = "none" THEN "none"
                                  ELSE IF stale THEN "refused" ELSE "landed"]
               /\ Settle1(s, IF stale THEN "stale" ELSE "done")
               /\ t1' = [t1 EXCEPT ![s] = [T1Idle EXCEPT !.n = o.n, !.ver = o.ver]]
               \* a stale record rolls back: T1's input goes back and T1 binds again
               /\ p1' = IF stale THEN [p1 EXCEPT ![s] = TRUE] ELSE p1
               /\ IF lands
                  THEN /\ row' = [row EXCEPT ![s][FactRow] =
                                    IF CapOf(FactRow) = "Fifo" THEN Append(@, fact) ELSE <<fact>>]
                       /\ ver' = [ver EXCEPT ![s][FactRow] = @ + 1]
                       /\ nid' = [nid EXCEPT ![s] = @ + 1]
                  ELSE UNCHANGED <<row, ver, nid>>
               /\ EndWait(s)
    /\ UNCHANGED <<w, t0, mark1, pc>>

Internal(s) == Start0(s) \/ Bind1(s) \/ Record(s)

Quiet == \A s \in Subj : ~CanStart0(s) /\ ~CanBind1(s) /\ ~CanRecord(s)

-----------------------------------------------------------------------------
(* Outcomes of T0's take *)

\* Models: crates/petri-workflow/src/exec.rs:commit_consumption with H1. The
\* row's version moves only if the held token counted in the committed row
\* (a restore would have put it back). A reader with an edge to the take
\* moves its trace version along when nothing else moved the row (I4).
CommitTake(s) ==
    LET bump == [r \in Rows |-> Holds0(s, r) /\ PutsBack(s, r)]
        o == t1[s]
        mine == {e \in o.edges : t0[s].hold[e[1]] /= NoTok /\ e[2] = t0[s].hold[e[1]].id}
        other == Live1(s) /\ o.tk /= t0[s].tk /\ \E r \in T1Rows : bump[r]
    IN /\ ver' = [ver EXCEPT ![s] = [r \in Rows |-> @[r] + IF bump[r] THEN 1 ELSE 0]]
       /\ t1' = [t1 EXCEPT ![s] = [@ EXCEPT
                   !.ver = [r \in T1Rows |->
                              IF Mutant /= "noadvance" /\ bump[r] /\ <<r, t0[s].hold[r].id>> \in mine
                                 /\ o.ver[r] = ver[s][r]
                              THEN ver[s][r] + 1 ELSE o.ver[r]],
                   !.edges = IF Mutant = "keepedge" THEN @ ELSE @ \ mine,
                   !.moved = IF other THEN @ \cup {"commit"} ELSE @,
                   !.adv = @ \/ mine /= {}]]
       /\ t0' = [t0 EXCEPT ![s] = [T0Idle EXCEPT !.tk = t0[s].tk, !.mark = t0[s].mark]]
       /\ UNCHANGED <<row, nid>>

\* Models: crates/petri-workflow/src/exec.rs:rollback -> Tokens::restore with
\* H2: a token put back makes each reader with an edge on it stale (I5).
RestoreTake(s, next) ==
    LET o == t1[s]
        back == [r \in Rows |-> Holds0(s, r) /\ PutsBack(s, r)]
        mine == {e \in o.edges : t0[s].hold[e[1]] /= NoTok /\ e[2] = t0[s].hold[e[1]].id}
        touched == {r \in T1Rows : back[r]}
    IN /\ row' = [row EXCEPT ![s] = [r \in Rows |->
                    IF Holds0(s, r) THEN RestoreInto(@[r], t0[s].hold[r], CapOf(r)) ELSE @[r]]]
       /\ t1' = [t1 EXCEPT ![s] = [@ EXCEPT
                   !.stale = IF Mutant = "nostale" THEN @ ELSE @ \cup {e[1] : e \in {m \in mine : back[m[1]]}},
                   !.edges = @ \ mine,
                   !.moved = IF Live1(s) /\ touched /= {} THEN @ \cup {"restore"} ELSE @,
                   !.deprest = @ \/ (Live1(s) /\ SameTake(s) /\ touched /= {})]]
       /\ t0' = [t0 EXCEPT ![s] = next]
       /\ UNCHANGED <<ver, nid>>

\* A parked take keeps its tokens; each reader that depends on it is
\* disclosed by a Condition (I7, ruling (a)).
Park0(s) ==
    /\ t0' = [t0 EXCEPT ![s] = [@ EXCEPT !.st = "parked", !.unk = TRUE]]
    /\ cond' = IF Live1(s) /\ t1[s].edges /= {} /\ Mutant /= "nocond" THEN cond \cup {s} ELSE cond
    /\ UNCHANGED <<row, ver, nid, t1>>

\* Models: crates/petri-workflow/src/exec.rs:observe for T0. An occurrence
\* fails only when every attempt failed; after an unknown attempt a failure
\* parks it. A retryable failure of a clean occurrence puts the tokens back
\* until the retry takes them again.
Complete0(s, res) ==
    /\ t0[s].st = "run"
    /\ LET o == t0[s]
           last == o.att >= MaxAtt
       IN CASE res = "done" -> CommitTake(s) /\ UNCHANGED cond
            [] res = "unknown" /\ ~last ->
                 /\ t0' = [t0 EXCEPT ![s] = [@ EXCEPT !.att = @ + 1, !.unk = TRUE]]
                 /\ UNCHANGED <<row, ver, nid, t1, cond>>
            [] res = "unknown" /\ last -> Park0(s)
            [] res = "retryable" /\ ~last /\ o.unk ->
                 /\ t0' = [t0 EXCEPT ![s].st = "retry"]
                 /\ UNCHANGED <<row, ver, nid, t1, cond>>
            [] res = "retryable" /\ ~last /\ ~o.unk ->
                 RestoreTake(s, [o EXCEPT !.st = "retry", !.hold = [r \in Rows |-> NoTok]])
                 /\ UNCHANGED cond
            [] (res = "definite" \/ last) /\ o.unk -> Park0(s)
            [] OTHER ->
                 RestoreTake(s, [T0Idle EXCEPT !.tk = o.tk, !.mark = [r \in Rows |-> o.hold[r].id]])
                 /\ UNCHANGED cond
    /\ UNCHANGED <<p1, w, mark1, first, ev>>

\* Models: crates/petri-workflow/src/exec.rs:step (the retry is due).
Step(s) ==
    /\ t0[s].st = "retry"
    /\ IF t0[s].unk
       THEN /\ t0' = [t0 EXCEPT ![s] = [@ EXCEPT !.st = "run", !.att = @ + 1]]
            /\ UNCHANGED row
       ELSE /\ \A r \in Rows : row[s][r] /= <<>>
            /\ TakeFronts(s, t0[s].att + 1, FALSE)
    /\ UNCHANGED <<ver, nid, p1, w, t1, mark1, cond, first, ev>>

\* Models: crates/petri-workflow/src/exec.rs:resolve and unpark. Verify and
\* Drop commit the take; Retry runs one more attempt. Each removes the
\* Conditions of this take.
Resolve0(s, how) ==
    /\ t0[s].st = "parked"
    /\ cond' = cond \ {s}
    /\ IF how = "retry"
       THEN /\ t0' = [t0 EXCEPT ![s] = [@ EXCEPT !.st = "run", !.att = MaxAtt]]
            /\ UNCHANGED <<row, ver, nid, t1>>
       ELSE CommitTake(s)
    /\ UNCHANGED <<p1, w, mark1, first, ev>>

-----------------------------------------------------------------------------
(* Outcomes of T1's effect. Modelling choice: T1's own retries are left   *)
(* out; a failure settles T1 at once and marks its input.                 *)

Complete1(s, res) ==
    /\ Body = "keyed"
    /\ t1[s].st = "run"
    /\ CASE res = "done" -> t1' = [t1 EXCEPT ![s].st = "rec"] /\ UNCHANGED <<p1, mark1, first>>
         [] res = "unknown" -> t1' = [t1 EXCEPT ![s].st = "parked"] /\ UNCHANGED <<p1, mark1, first>>
         [] OTHER ->
              /\ t1' = [t1 EXCEPT ![s] = [T1Idle EXCEPT !.n = t1[s].n]]
              /\ p1' = [p1 EXCEPT ![s] = TRUE]
              /\ mark1' = [mark1 EXCEPT ![s] = TRUE]
              /\ Settle1(s, "failed")
    /\ IF res \in {"done", "unknown"} THEN UNCHANGED cond ELSE EndWait(s)
    /\ UNCHANGED <<row, ver, nid, w, t0, ev>>

Resolve1(s, how) ==
    /\ t1[s].st = "parked"
    /\ CASE how = "verify" -> t1' = [t1 EXCEPT ![s].st = "rec"] /\ UNCHANGED first
         [] how = "retry" -> t1' = [t1 EXCEPT ![s].st = "run"] /\ UNCHANGED first
         [] OTHER -> t1' = [t1 EXCEPT ![s] = [T1Idle EXCEPT !.n = t1[s].n]] /\ Settle1(s, "dropped")
    /\ IF how = "drop" THEN EndWait(s) ELSE UNCHANGED cond
    /\ UNCHANGED <<row, ver, nid, p1, w, t0, mark1, ev>>

-----------------------------------------------------------------------------
(* Outside writes *)

\* Models: crates/petri-workflow/src/exec.rs:write. A write of equal content
\* leaves a Single or Register token; a FIFO write appends.
Write(s, r, x) ==
    /\ IF r = "P1"
       THEN /\ p1' = [p1 EXCEPT ![s] = TRUE]
            /\ mark1' = [mark1 EXCEPT ![s] = FALSE]
            /\ UNCHANGED <<row, ver, nid, t1>>
       ELSE /\ UNCHANGED <<p1, mark1>>
            /\ LET tok == [id |-> nid[s], x |-> x]
                   same == CapOf(r) /= "Fifo" /\ row[s][r] /= <<>> /\ Head(row[s][r]).x = x
               IN IF same
                  THEN UNCHANGED <<row, ver, nid, t1>>
                  ELSE /\ row' = [row EXCEPT ![s][r] = IF CapOf(r) = "Fifo" THEN Append(@, tok) ELSE <<tok>>]
                       /\ ver' = [ver EXCEPT ![s][r] = @ + 1]
                       /\ nid' = [nid EXCEPT ![s] = @ + 1]
                       /\ t1' = IF Live1(s) /\ r \in T1Rows
                                THEN [t1 EXCEPT ![s].moved = @ \cup {"write"}] ELSE t1
    /\ UNCHANGED <<t0, cond, first, ev>>

-----------------------------------------------------------------------------
Free(s) ==
    \/ w[s] < MaxWrites /\ \E r \in {"P1"} \cup Rows, x \in Xs :
            Write(s, r, x) /\ w' = [w EXCEPT ![s] = @ + 1]
    \/ \E res \in {"done", "retryable", "definite", "unknown"} :
            (Complete0(s, res) \/ Complete1(s, res)) /\ UNCHANGED w
    \/ Step(s) /\ UNCHANGED w
    \/ \E how \in {"verify", "retry", "drop"} : (Resolve0(s, how) \/ Resolve1(s, how)) /\ UNCHANGED w

DoOp(op) ==
    CASE op.k = "w" -> Write(op.s, op.r, op.x)
      [] op.k = "c0" -> Complete0(op.s, op.res)
      [] op.k = "c1" -> Complete1(op.s, op.res)
      [] op.k = "step" -> Step(op.s)
      [] op.k = "r0" -> Resolve0(op.s, op.how)

\* An operation that does not apply blocks the script; the witness
\* ScriptNeverEnds then holds and its row turns red.
Outside ==
    /\ Quiet
    /\ IF Case = "free"
       THEN \E s \in Subj : Free(s) /\ UNCHANGED pc
       ELSE /\ pc <= Len(Script)
            /\ DoOp(Script[pc]) /\ UNCHANGED w
            /\ pc' = pc + 1

Finished ==
    /\ Quiet
    /\ \/ Case /= "free" /\ pc > Len(Script)
       \/ Case = "free" /\ \A s \in Subj : t0[s].st = "idle" /\ t1[s].st = "idle" /\ w[s] = MaxWrites
    /\ UNCHANGED vars

Next == (\E s \in Subj : Internal(s)) \/ Outside \/ Finished

ResolveCommit0(s) == Quiet /\ UNCHANGED <<w, pc>> /\ \E how \in {"verify", "drop"} : Resolve0(s, how)

\* Every internal step runs (quiesce loops to a fixpoint); every running
\* effect completes (runner reply or its timer); a due retry runs (the
\* clock moves). Resolve is the operator's call and has no fairness.
Fairness ==
    \A s \in Subj :
        /\ WF_vars(Internal(s))
        /\ WF_vars(Quiet /\ UNCHANGED <<w, pc>> /\
                   \E res \in {"done", "retryable", "definite", "unknown"} :
                      Complete0(s, res) \/ Complete1(s, res))
        /\ WF_vars(Quiet /\ Step(s) /\ UNCHANGED <<w, pc>>)

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
(* Invariants *)

TypeOK ==
    /\ \A s \in Subj : t1[s].st \in {"idle", "run", "rec", "held", "parked"}
    /\ \A s \in Subj : t0[s].st \in {"idle", "run", "retry", "parked"}
    /\ cond \subseteq Subj

\* I1: a held token is in no row, and a reader never saw a token that the
\* take open at its bind holds.
I1Exclusive ==
    \A s \in Subj, r \in Rows :
        /\ Holds0(s, r) => t0[s].hold[r].id \notin Ids(row[s][r])
        /\ (Live1(s) /\ r \in T1Rows /\ SameTake(s) /\ Holds0(s, r))
              => t1[s].seen[r] /= t0[s].hold[r].id

\* I2: a live reader keeps an edge to each take it depends on while that
\* take stays open; every edge names the take open at the bind (acyclic).
I2Dependency ==
    \A s \in Subj :
        /\ \A r \in T1Rows :
              (Live1(s) /\ SameTake(s) /\ Holds0(s, r) /\ DependsOn(s, r, t1[s].seen[r]))
                 => <<r, t0[s].hold[r].id>> \in t1[s].edges
        /\ \A e \in t1[s].edges : SameTake(s) /\ t0[s].hold[e[1]].id = e[2]

\* I3: no commit with an open dependency; a stale reader rolls back at once.
I3CommitWait == ev.verdict = "done" => ev.edges = {}

\* I4: a reader whose only change was the commit of the take it depended
\* on is not stale.
I4TakeCommits == ev.verdict = "stale" => ev.moved /= {}

\* I5: a reader into whose read row the take open at its bind put its token
\* back does not commit.
I5TakeFails == ev.verdict = "done" => ~ev.deprest

\* I6: a record that is not stale saw the committed front of each read row.
I6Sound == ev.verdict = "done" => \A r \in T1Rows : ev.seen[r] = ev.front[r]

\* Fact CAS: a fact lands only from a reader that saw the committed front.
FactSound == ev.fact = "landed" => ev.seen[FactRow] = ev.front[FactRow]

WaitsParked(s) == Live1(s) /\ t1[s].edges /= {} /\ t0[s].st = "parked"

\* I7: while a reader depends on a parked take, a Condition names them.
I7Disclosure == \A s \in Subj : WaitsParked(s) => s \in cond

\* I7, other half: a Condition names a reader that still waits.
I7NoStaleCondition == \A s \in Subj : s \in cond => WaitsParked(s)

\* The static example's checks, after its operation `at` once quiet.
Holds(c) ==
    LET s == c.s IN
    CASE c.kind = "held" -> t1[s].st = "held"
      [] c.kind = "notheld" -> t1[s].st /= "held"
      [] c.kind = "edge" -> t1[s].edges /= {}
      [] c.kind = "noedge" -> t1[s].edges = {}
      [] c.kind = "cond" -> s \in cond
      [] c.kind = "nocond" -> s \notin cond
      [] c.kind = "verdict" -> first[s] = c.v
      [] c.kind = "p0" -> Len(row[s]["P0"]) = 1 /\ Head(row[s]["P0"]).x = c.v

CaseHolds == \A c \in CaseChecks(Case) : (pc = c.at + 1 /\ Quiet) => Holds(c)

-----------------------------------------------------------------------------
(* Witnesses: each must be violated (the state it excludes is reachable). *)

NeverHeldToken == \A s \in Subj : ~HoldsAny0(s)
NeverEdge == \A s \in Subj : t1[s].edges = {}
NeverHeldReader == \A s \in Subj : t1[s].st /= "held"
NeverDoneAfterItsTakeCommitted == ~(ev.verdict = "done" /\ ev.adv)
NeverStaleByRestore == ev.verdict /= "stale" \/ ~ev.deprest
NeverDoneRecord == ev.verdict /= "done"
NeverWaitsParked == \A s \in Subj : ~WaitsParked(s)
NeverCondition == cond = {}
NeverHeldStale == \A s \in Subj : ~(t1[s].st = "held" /\ StaleNow(s))
NeverFactRefused == ev.fact /= "refused"
NeverFactLanded == ev.fact /= "landed"
ScriptNeverEnds == ~(pc > Len(Script) /\ Quiet)

-----------------------------------------------------------------------------
(* Liveness *)

Held(s) == t1[s].st = "held"

\* A held reader is released unless a take it waits on is parked.
ReleasedUnlessParked == \A s \in Subj : Held(s) ~> (~Held(s) \/ WaitsParked(s))

\* Readers do not starve under retry, park and resolve: if a parked take
\* is eventually verified or dropped, every held reader is released.
ReleasedWhenResolved ==
    (\A s \in Subj : SF_vars(ResolveCommit0(s))) => \A s \in Subj : Held(s) ~> ~Held(s)

\* A held reader that a write or a restore made stale is released for its
\* rebind, without a resolve of the take it waits on.
StaleReleased == \A s \in Subj : (Held(s) /\ StaleNow(s)) ~> ~Held(s)

\* Witness: without a resolve a reader can wait for ever.
ReleasedWithoutResolve == \A s \in Subj : Held(s) ~> ~Held(s)

\* Every quiesce reaches a fixpoint (Level-dead: no enabled transition is
\* left unfired, with held readers).
QuiesceEnds == []<>Quiet
================================================================================
