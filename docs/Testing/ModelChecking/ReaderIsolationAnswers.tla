------------------------ MODULE ReaderIsolationAnswers ------------------------
(***************************************************************************)
(* The lowering rule of answers, for one subject: `Answer.T` is enabled    *)
(* only while an ask is open and no place of the gate holds a token. The   *)
(* work T takes the Answered token and runs its effect, which completes    *)
(* Done, Failed (retryable or definite) or Unknown (parked until resolve). *)
(*                                                                         *)
(* Gate selects the inhibitor set of `Answer.T`:                           *)
(*   production - the busy set of the lowering: Answered, Running,         *)
(*                RetryPending, OutcomeUnknown, WorkFailed                 *)
(*   testnet    - the same without OutcomeUnknown                          *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS Asks, Gate

ASSUME Asks \in Nat /\ Gate \in {"production", "testnet"}

Busy == IF Gate = "production"
        THEN {"Answered", "Running", "RetryPending", "OutcomeUnknown", "WorkFailed"}
        ELSE {"Answered", "Running", "RetryPending", "WorkFailed"}

VARIABLES
    ask,      \* the open ask (0: none)
    asked,    \* asks posed so far
    tok,      \* tok[p]: the ask whose token place p holds (0: none)
    effects   \* effects[a]: work occurrences started for ask a

vars == <<ask, asked, tok, effects>>

Places == {"Answered", "Running", "RetryPending", "OutcomeUnknown", "WorkFailed"}

Init ==
    /\ ask = 0 /\ asked = 0
    /\ tok = [p \in Places |-> 0]
    /\ effects = [a \in 1..Asks |-> 0]

Offered == ask /= 0 /\ \A p \in Busy : tok[p] = 0

\* Modelling choice: the situation of the work holds; the derived ask row
\* `Asks.T` appears.
Ask ==
    /\ ask = 0 /\ asked < Asks
    /\ ask' = asked + 1 /\ asked' = asked + 1
    /\ UNCHANGED <<tok, effects>>

\* Models: crates/holon-net/src/uft/lower.rs, the manual `Answer.T`
\* (bind: the ask is offered; inhibitors: the gate).
Answer ==
    /\ Offered
    /\ tok' = [tok EXCEPT !["Answered"] = ask]
    /\ UNCHANGED <<ask, asked, effects>>

\* The work takes the Answered token; its effect runs.
Work ==
    /\ tok["Answered"] /= 0 /\ tok["Running"] = 0
    /\ tok' = [tok EXCEPT !["Answered"] = 0, !["Running"] = tok["Answered"]]
    /\ effects' = [effects EXCEPT ![tok["Answered"]] = @ + 1]
    /\ UNCHANGED <<ask, asked>>

Move(from, to) == tok' = [tok EXCEPT ![from] = 0, ![to] = tok[from]]

\* The work commits: the situation is consumed, so the ask closes.
Commit(from) ==
    /\ tok' = [tok EXCEPT ![from] = 0]
    /\ ask' = IF ask = tok[from] THEN 0 ELSE ask
    /\ UNCHANGED <<asked, effects>>

Complete ==
    /\ tok["Running"] /= 0
    /\ \/ Commit("Running")
       \/ Move("Running", "RetryPending") /\ UNCHANGED <<ask, asked, effects>>
       \/ Move("Running", "WorkFailed") /\ UNCHANGED <<ask, asked, effects>>
       \/ Move("Running", "OutcomeUnknown") /\ UNCHANGED <<ask, asked, effects>>

RetryDue ==
    /\ tok["RetryPending"] /= 0 /\ tok["Running"] = 0
    /\ Move("RetryPending", "Running") /\ UNCHANGED <<ask, asked, effects>>

Resolve ==
    /\ tok["OutcomeUnknown"] /= 0
    /\ \/ Commit("OutcomeUnknown")
       \/ tok["Running"] = 0 /\ Move("OutcomeUnknown", "Running") /\ UNCHANGED <<ask, asked, effects>>

Next == Ask \/ Answer \/ Work \/ Complete \/ RetryDue \/ Resolve

Spec == Init /\ [][Next]_vars

\* One effect per ask: no ask starts a second work occurrence.
OneEffectPerAsk == \A a \in 1..Asks : effects[a] <= 1

\* Witness: an ask gets its effect.
NoAskAnswered == \A a \in 1..Asks : effects[a] = 0
================================================================================
