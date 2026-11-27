---- MODULE WorkRuntime ----
\* Properties of the work-runtime net (crates/holon-net/nets/work-runtime.yaml).
\* The behaviour is WorkRuntimeNet, generated from the net file; the outside
\* world is WorkRuntimeEnv. Each property has a witness that TLC must find
\* violated, so that the property is not true of an unreachable state.
EXTENDS WorkRuntimeNet, TLC

Perms == Permutations(Subj)

\* Model bound, not semantics: a fifo row holds at most 2 tokens and at most
\* one attempt per subject is parked.
Bound == \A s \in Subj : Waiting[s] <= 2 /\ StartJob_parked[s] <= 1 /\ Handoff_parked[s] <= 1

JobsInFlight == Count(StartJob_running) + Total(StartJob_parked)

\* At most `pool` jobs run at once.
AtMostPoolJobs == JobsInFlight <= Pool_Cpu
\* Witness: every slot busy.
NotAllSlotsBusy == JobsInFlight < Pool_Cpu

\* A subject never gets a Job while a StartJob run for it is in flight.
NoJobWhileStarting == \A s \in Subj : ~(Job[s] /\ StartJob_running[s])
\* Witness: a subject holds a Job and another waiting token, which only the
\* inhibitor arc keeps from starting.
NeverJobAndWaiting == \A s \in Subj : ~(Job[s] /\ Waiting[s] > 0)

FailedStart(s) == \E r \in BOOLEAN : Failed_StartJob(s, r)

\* A failed run returns its input to the place it came from, and while the
\* run is open and no retry waits, the failure port gets a token. An
\* occurrence that saw an unknown outcome parks instead.
FailureReturnsInput ==
  [][\A s \in Subj :
       (FailedStart(s) /\ ~StartJob_unk[s])
         => /\ Waiting'[s] = Waiting[s] + 1
            /\ Cpu' = Cpu + 1
            /\ (StartJob_attempts[s] > 0 /\ ~StartJob_retry'[s]) => Stalled'[s]]_vars
\* Witness: a run fails.
NoStartFails == [][\A s \in Subj : ~FailedStart(s)]_vars

\* Every Cpu token is free, held by an attempt, or held by a Job.
SlotsConserved == Cpu + Lent_Cpu + Count(Job) = Pool_Cpu

\* A Job ends by returning its slot.
JobEndReturnsSlot == [][\A s \in Subj : (Job[s] /\ ~Job'[s]) => Cpu' = Cpu + 1]_vars
\* Witness: a Job ends.
NoJobEnds == [][\A s \in Subj : ~(Job[s] /\ ~Job'[s])]_vars

\* Liveness: a waiting job with its agent, its budget and a free slot, which
\* no stall token and no parked attempt holds, starts or stops being ready.
\* Fairness assumption: weak fairness of every auto firing (the executor fires
\* every enabled auto pair before a call returns) and of every timer (the clock
\* advances). No effect result, probe or user action is fair.
Ready(s) == Waiting[s] > 0 /\ Cpu > 0 /\ Budget[s] /\ Agent[s] /\ ~Stalled[s] /\ StartJob_parked[s] = 0
WaitingStarts == \A s \in Subj : Ready(s) ~> (StartJob_running[s] \/ ~Ready(s))
\* Witness: a subject is ready.
NeverReady == \A s \in Subj : ~Ready(s)

TimerFairness ==
  \A s \in Subj :
    /\ WF_vars(RunTimeout_StartJob(s)) /\ WF_vars(RetryDue_StartJob(s)) /\ WF_vars(Abandon_StartJob(s))
    /\ WF_vars(RunTimeout_Handoff(s)) /\ WF_vars(RetryDue_Handoff(s))
LiveSpec == Spec /\ AutoFairness /\ TimerFairness
====
