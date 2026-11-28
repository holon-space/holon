------------------------- MODULE ReaderIsolationWaitGraph -------------------------
(***************************************************************************)
(* The wait graph of readers and takes whose rows overlap.                 *)
(*                                                                         *)
(* N transitions in a ring: transition t takes the token of row t and      *)
(* reads row t+1, so each one is a take and a reader of another take's     *)
(* row, and a wait cycle is one edge set away. The bind, hold, commit and  *)
(* restore rules are those of ReaderIsolation.tla over Single rows: a      *)
(* reader gets an edge at its bind to each take that is open then and      *)
(* holds the token of a read row the reader saw as a hole (narrow I2).     *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    N,          \* transitions and rows
    MaxWrites   \* outside writes

ASSUME N \in Nat /\ N >= 2

Trans == 1..N
Rows == 1..N
Take(t) == t
Reads(t) == {(t % N) + 1}

VARIABLES
    row,    \* row[r]: the id of the token in row r, 0 for a hole
    ver,    \* ver[r]: the row's version
    nid,    \* next token id
    w,      \* outside writes so far
    st,     \* st[t]: "idle", "run", "rec" or "held"
    hold,   \* hold[t]: the token id t's open take holds, 0 if none
    sver,   \* sver[t]: the row versions at t's bind (its read trace)
    edges,  \* edges[t]: the takes t waits on
    stale   \* stale[t]: a restore put a token back into what t read

vars == <<row, ver, nid, w, st, hold, sver, edges, stale>>

Init ==
    /\ row = [r \in Rows |-> 0]
    /\ ver = [r \in Rows |-> 0]
    /\ nid = 1
    /\ w = 0
    /\ st = [t \in Trans |-> "idle"]
    /\ hold = [t \in Trans |-> 0]
    /\ sver = [t \in Trans |-> [r \in Rows |-> 0]]
    /\ edges = [t \in Trans |-> {}]
    /\ stale = [t \in Trans |-> FALSE]

Open(u) == hold[u] /= 0
StaleNow(t) == stale[t] \/ \E r \in Reads(t) : sver[t][r] /= ver[r]

CanBind(t) == st[t] = "idle" /\ row[Take(t)] /= 0

\* The take and the read are one bind; the edges go to takes open now.
Bind(t) ==
    /\ CanBind(t)
    /\ LET dep == {u \in Trans \ {t} : Open(u) /\ Take(u) \in Reads(t) /\ row[Take(u)] = 0}
       IN edges' = [u \in Trans |-> IF u = t THEN dep ELSE edges[u]]
    /\ hold' = [hold EXCEPT ![t] = row[Take(t)]]
    /\ row' = [row EXCEPT ![Take(t)] = 0]
    /\ sver' = [sver EXCEPT ![t] = ver]
    /\ st' = [st EXCEPT ![t] = "run"]
    /\ UNCHANGED <<ver, nid, w, stale>>

\* The take ends without consuming: the token goes back unless a newer one
\* was written (PutsBack); each reader with an edge on it turns stale.
Restore(t) ==
    LET r == Take(t)
        back == row[r] = 0
    IN /\ row' = IF back THEN [row EXCEPT ![r] = hold[t]] ELSE row
       /\ stale' = [u \in Trans |-> IF u = t THEN FALSE ELSE stale[u] \/ (back /\ t \in edges[u])]
       /\ edges' = [u \in Trans |-> IF u = t THEN {} ELSE edges[u] \ {t}]
       /\ st' = [st EXCEPT ![t] = "idle"]
       /\ hold' = [hold EXCEPT ![t] = 0]
       /\ UNCHANGED <<ver, nid, w, sver>>

\* The take commits (H1): the version moves when the held token counted; a
\* reader with an edge on it moves its trace along (I4).
Commit(t) ==
    LET r == Take(t)
        bump == row[r] = 0
    IN /\ ver' = IF bump THEN [ver EXCEPT ![r] = @ + 1] ELSE ver
       /\ sver' = [u \in Trans |->
                     IF bump /\ t \in edges[u] /\ sver[u][r] = ver[r]
                     THEN [sver[u] EXCEPT ![r] = ver[r] + 1] ELSE sver[u]]
       /\ edges' = [u \in Trans |-> edges[u] \ {t}]
       /\ st' = [st EXCEPT ![t] = "idle"]
       /\ hold' = [hold EXCEPT ![t] = 0]
       /\ UNCHANGED <<row, nid, w, stale>>

CanRecord(t) == st[t] = "rec" \/ (st[t] = "held" /\ (edges[t] = {} \/ StaleNow(t)))

\* The stale check runs before the hold; a commit waits for every edge.
Record(t) ==
    /\ CanRecord(t)
    /\ IF StaleNow(t) THEN Restore(t)
       ELSE IF edges[t] /= {}
            THEN st' = [st EXCEPT ![t] = "held"] /\ UNCHANGED <<row, ver, nid, w, hold, sver, edges, stale>>
            ELSE Commit(t)

Internal(t) == Bind(t) \/ Record(t)

Quiet == \A t \in Trans : ~CanBind(t) /\ ~CanRecord(t)

Write(r) ==
    /\ w < MaxWrites
    /\ row' = [row EXCEPT ![r] = nid]
    /\ ver' = [ver EXCEPT ![r] = @ + 1]
    /\ nid' = nid + 1
    /\ w' = w + 1
    /\ UNCHANGED <<st, hold, sver, edges, stale>>

Complete(t, res) ==
    /\ st[t] = "run"
    /\ IF res = "done"
       THEN st' = [st EXCEPT ![t] = "rec"] /\ UNCHANGED <<row, ver, nid, w, hold, sver, edges, stale>>
       ELSE Restore(t)

Outside ==
    /\ Quiet
    /\ \/ \E r \in Rows : Write(r)
       \/ \E t \in Trans, res \in {"done", "fail"} : Complete(t, res)

Finished ==
    /\ Quiet
    /\ w = MaxWrites
    /\ \A t \in Trans : st[t] = "idle"
    /\ UNCHANGED vars

Next == (\E t \in Trans : Internal(t)) \/ Outside \/ Finished

Spec == Init /\ [][Next]_vars

-----------------------------------------------------------------------------
TypeOK ==
    /\ \A t \in Trans : st[t] \in {"idle", "run", "rec", "held"}
    /\ \A t \in Trans : edges[t] \subseteq Trans \ {t}

RECURSIVE Path(_, _, _)
Path(t, u, n) == u \in edges[t] \/ (n > 0 /\ \E v \in edges[t] : Path(v, u, n - 1))

\* No reader waits, through any chain of edges, on itself.
NoWaitCycle == \A t \in Trans : ~Path(t, t, N)

\* Witness: t waits, through a chain of edges, on u while u reads the row
\* t takes, so one more edge from u to t would close a cycle.
NeverCrossEdge == ~\E t, u \in Trans : Path(t, u, N) /\ Take(t) \in Reads(u)

\* Witness: a reader is held on an open take.
NeverHeld == \A t \in Trans : st[t] /= "held"
================================================================================
