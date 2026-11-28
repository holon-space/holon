---- MODULE WorkRuntimeEnv ----
\* The outside world of the work-runtime net: what an effect may report, which
\* places the probes write, which manual transitions the user fires, which
\* timers run and how the user resolves a parked attempt. WorkRuntimeNet, the
\* spec generated from the net file, extends this module.
\*
\* World "full": everything the executor allows. World "pool": a smaller world
\* in which three jobs share the pool within TLC's reach; the user never hands
\* off, a probe never retracts and a parked attempt is only dropped.
CONSTANT World

\* "ok" is Done, "failed" a definite and "failed_retryable" a retryable
\* failure, "unknown" an outcome the receiver decides. An effect may also never
\* report: no result action is fair, so a behaviour without one is allowed.
Outcomes ==
  [StartJob |-> {"ok", "failed", "failed_retryable", "unknown"},
   Handoff  |-> IF World = "full" THEN {"ok", "failed", "failed_retryable", "unknown"} ELSE {}]

\* The probe places a transition reads. JobRecord, Process, Disk and Host are
\* probed too, but no transition reads them, so they stay empty.
Written == IF World = "full" THEN {"Agent", "Budget", "Card"} ELSE {"Agent", "Budget"}
Retracted == IF World = "full" THEN {"Agent", "Budget", "Card"} ELSE {}

Manual == IF World = "full" THEN {"Continue", "Handoff"} ELSE {"Continue"}

\* A timer may expire at any time: "run", "abandon" and "retry".
Timers == IF World = "full" THEN {"run", "abandon", "retry"} ELSE {"run", "retry"}

Resolutions == IF World = "full" THEN {"verify", "retry", "drop"} ELSE {"drop"}
====
