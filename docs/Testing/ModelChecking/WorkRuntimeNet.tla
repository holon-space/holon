---- MODULE WorkRuntimeNet ----
\* Generated from crates/holon-net/nets/work-runtime.yaml. Do not edit: regenerate.
EXTENDS Naturals, FiniteSets, WorkRuntimeEnv
CONSTANTS Subj, FiringRule, Pool_Cpu
\* The file declares pool Cpu with 4 tokens.
VARIABLES Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk
vars == <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

RECURSIVE SumOver(_, _)
SumOver(f, S) == IF S = {} THEN 0 ELSE LET x == CHOOSE y \in S : TRUE IN f[x] + SumOver(f, S \ {x})
Count(b) == Cardinality({x \in Subj : b[x]})
Total(n) == SumOver(n, Subj)
\* Tokens of Cpu that running or parked attempts hold.
Lent_Cpu == Count(StartJob_running) + Total(StartJob_parked)

Init ==
  /\ Agent = [x \in Subj |-> FALSE]
  /\ Budget = [x \in Subj |-> FALSE]
  /\ Card = [x \in Subj |-> FALSE]
  /\ Cpu = Pool_Cpu
  /\ Disk = [x \in Subj |-> FALSE]
  /\ Host = [x \in Subj |-> FALSE]
  /\ Job = [x \in Subj |-> FALSE]
  /\ JobRecord = [x \in Subj |-> FALSE]
  /\ Process = [x \in Subj |-> FALSE]
  /\ Stalled = [x \in Subj |-> FALSE]
  /\ Waiting = [x \in Subj |-> 0]
  /\ StartJob_running = [x \in Subj |-> FALSE]
  /\ StartJob_parked = [x \in Subj |-> 0]
  /\ StartJob_attempts = [x \in Subj |-> 0]
  /\ StartJob_moved = [x \in Subj |-> {}]
  /\ StartJob_mark = [x \in Subj |-> FALSE]
  /\ StartJob_retry = [x \in Subj |-> FALSE]
  /\ StartJob_unk = [x \in Subj |-> FALSE]
  /\ Handoff_running = [x \in Subj |-> FALSE]
  /\ Handoff_parked = [x \in Subj |-> 0]
  /\ Handoff_attempts = [x \in Subj |-> 0]
  /\ Handoff_moved = [x \in Subj |-> {}]
  /\ Handoff_mark = [x \in Subj |-> FALSE]
  /\ Handoff_retry = [x \in Subj |-> FALSE]
  /\ Handoff_unk = [x \in Subj |-> FALSE]

\* Enabled_T(s): the arcs and the executor's own refusals (in flight, retry pending, failed on this input).
Enabled_StartJob(s) == Waiting[s] > 0 /\ Cpu > 0 /\ Budget[s] /\ ~(Job[s]) /\ ~StartJob_running[s] /\ ~StartJob_retry[s] /\ ~StartJob_mark[s]
Enabled_JobEnded(s) == Job[s] /\ Agent[s]
Enabled_AgentTurnEnded(s) == Job[s] /\ Agent[s]
Enabled_Stalled(s) == Agent[s] /\ ~(Stalled[s]) /\ ~(Job[s])
Enabled_Continue(s) == Stalled[s]
Enabled_Handoff(s) == Stalled[s] /\ Card[s] /\ ~Handoff_running[s] /\ ~Handoff_retry[s]
Enabled(t, s) ==
  CASE t = "StartJob" -> Enabled_StartJob(s)
    [] t = "JobEnded" -> Enabled_JobEnded(s)
    [] t = "AgentTurnEnded" -> Enabled_AgentTurnEnded(s)
    [] t = "Stalled" -> Enabled_Stalled(s)
    [] t = "Continue" -> Enabled_Continue(s)
    [] t = "Handoff" -> Enabled_Handoff(s)
AutoTransitions == {"StartJob", "JobEnded", "AgentTurnEnded", "Stalled"}

\* The firing rule. "executor": among the enabled auto producers of one place for one
\* subject only the first in (priority, declaration) order fires. "L0": an auto pair fires
\* only when no auto pair before it in that order is enabled, for any subject.
Preferred(t) ==
  CASE t = "StartJob" -> {}
    [] t = "JobEnded" -> {}
    [] t = "AgentTurnEnded" -> {}
    [] t = "Stalled" -> {}
Earlier(t) ==
  CASE t = "StartJob" -> {}
    [] t = "JobEnded" -> {"StartJob"}
    [] t = "AgentTurnEnded" -> {"StartJob", "JobEnded"}
    [] t = "Stalled" -> {"StartJob", "JobEnded", "AgentTurnEnded"}
MayFire(t, s) ==
  IF FiringRule = "executor"
  THEN \A u \in Preferred(t) : ~Enabled(u, s)
  ELSE \A u \in Earlier(t) : \A k \in Subj : ~Enabled(u, k)
\* Every call into the executor ends quiescent: no auto pair is enabled.
Quiescent == \A t \in AutoTransitions : \A s \in Subj : ~Enabled(t, s)


\* Firings of the net.
Start_StartJob(s) ==
  /\ Enabled_StartJob(s)
  /\ MayFire("StartJob", s)
  /\ Cpu' = (Cpu - 1)
  /\ StartJob_attempts' = [StartJob_attempts EXCEPT ![s] = @ + 1]
  /\ StartJob_moved' = [StartJob_moved EXCEPT ![s] = {}]
  /\ StartJob_running' = [StartJob_running EXCEPT ![s] = TRUE]
  /\ StartJob_unk' = [StartJob_unk EXCEPT ![s] = FALSE]
  /\ Waiting' = [Waiting EXCEPT ![s] = @ - 1]
  /\ UNCHANGED <<Agent, Budget, Card, Disk, Host, Job, JobRecord, Process, Stalled, StartJob_parked, StartJob_mark, StartJob_retry, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Fire_JobEnded(s) ==
  /\ Enabled_JobEnded(s)
  /\ MayFire("JobEnded", s)
  /\ Card' = [Card EXCEPT ![s] = TRUE]
  /\ Cpu' = (IF Cpu + Lent_Cpu - 0 < Pool_Cpu THEN Cpu + 1 ELSE Cpu)
  /\ Handoff_moved' = [Handoff_moved EXCEPT ![s] = IF Handoff_running[s] THEN @ \cup {"Card"} ELSE @]
  /\ Job' = [Job EXCEPT ![s] = FALSE]
  /\ StartJob_moved' = [StartJob_moved EXCEPT ![s] = IF StartJob_running[s] THEN @ \cup {"Job"} ELSE @]
  /\ UNCHANGED <<Agent, Budget, Disk, Host, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_mark, Handoff_retry, Handoff_unk>>

Fire_AgentTurnEnded(s) ==
  /\ Enabled_AgentTurnEnded(s)
  /\ MayFire("AgentTurnEnded", s)
  /\ Job' = [Job EXCEPT ![s] = FALSE]
  /\ StartJob_moved' = [StartJob_moved EXCEPT ![s] = IF StartJob_running[s] THEN @ \cup {"Job"} ELSE @]
  /\ Waiting' = [Waiting EXCEPT ![s] = @ + 1]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, JobRecord, Process, Stalled, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Fire_Stalled(s) ==
  /\ Enabled_Stalled(s)
  /\ MayFire("Stalled", s)
  /\ Stalled' = [Stalled EXCEPT ![s] = TRUE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Fire_Continue(s) ==
  /\ Enabled_Continue(s)
  /\ Quiescent
  /\ "Continue" \in Manual
  /\ Stalled' = [Stalled EXCEPT ![s] = FALSE]
  /\ Waiting' = [Waiting EXCEPT ![s] = @ + 1]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Start_Handoff(s) ==
  /\ Enabled_Handoff(s)
  /\ Quiescent
  /\ "Handoff" \in Manual
  /\ Handoff_attempts' = [Handoff_attempts EXCEPT ![s] = @ + 1]
  /\ Handoff_moved' = [Handoff_moved EXCEPT ![s] = {}]
  /\ Handoff_running' = [Handoff_running EXCEPT ![s] = TRUE]
  /\ Handoff_unk' = [Handoff_unk EXCEPT ![s] = FALSE]
  /\ Stalled' = [Stalled EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_parked, Handoff_mark, Handoff_retry>>

\* What the outside world does; WorkRuntimeEnv says which of it may happen.
Done_StartJob(s) ==
  /\ StartJob_running[s]
  /\ Quiescent
  /\ "ok" \in Outcomes["StartJob"]
  /\ Job' = IF StartJob_moved[s] = {} THEN [Job EXCEPT ![s] = TRUE] ELSE Job
  /\ StartJob_attempts' = [StartJob_attempts EXCEPT ![s] = 0]
  /\ StartJob_mark' = IF StartJob_moved[s] = {} THEN [StartJob_mark EXCEPT ![s] = FALSE] ELSE StartJob_mark
  /\ StartJob_moved' = [StartJob_moved EXCEPT ![s] = {}]
  /\ StartJob_retry' = [StartJob_retry EXCEPT ![s] = FALSE]
  /\ StartJob_running' = [StartJob_running EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, JobRecord, Process, Stalled, Waiting, StartJob_parked, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Failed_StartJob(s, retryable) ==
  /\ StartJob_running[s]
  /\ Quiescent
  /\ retryable \in (IF "failed" \in Outcomes["StartJob"] THEN {FALSE} ELSE {}) \cup (IF "failed_retryable" \in Outcomes["StartJob"] THEN {TRUE} ELSE {})
  /\ Cpu' = IF StartJob_unk[s] THEN Cpu ELSE IF StartJob_attempts[s] > 0 /\ ~(retryable /\ StartJob_attempts[s] < 2) THEN (Cpu + 1) ELSE (Cpu + 1)
  /\ Stalled' = IF StartJob_unk[s] THEN Stalled ELSE IF StartJob_attempts[s] > 0 /\ ~(retryable /\ StartJob_attempts[s] < 2) THEN [Stalled EXCEPT ![s] = TRUE] ELSE Stalled
  /\ StartJob_attempts' = IF StartJob_unk[s] THEN StartJob_attempts ELSE [StartJob_attempts EXCEPT ![s] = IF StartJob_attempts[s] > 0 /\ (retryable /\ StartJob_attempts[s] < 2) THEN @ ELSE 0]
  /\ StartJob_mark' = IF StartJob_unk[s] THEN [StartJob_mark EXCEPT ![s] = TRUE] ELSE [StartJob_mark EXCEPT ![s] = TRUE]
  /\ StartJob_moved' = IF StartJob_unk[s] THEN [StartJob_moved EXCEPT ![s] = {}] ELSE [StartJob_moved EXCEPT ![s] = {}]
  /\ StartJob_parked' = IF StartJob_unk[s] THEN [StartJob_parked EXCEPT ![s] = @ + 1] ELSE StartJob_parked
  /\ StartJob_retry' = IF StartJob_unk[s] THEN StartJob_retry ELSE [StartJob_retry EXCEPT ![s] = StartJob_attempts[s] > 0 /\ (retryable /\ StartJob_attempts[s] < 2)]
  /\ StartJob_running' = IF StartJob_unk[s] THEN [StartJob_running EXCEPT ![s] = FALSE] ELSE [StartJob_running EXCEPT ![s] = FALSE]
  /\ Waiting' = IF StartJob_unk[s] THEN Waiting ELSE IF StartJob_attempts[s] > 0 /\ ~(retryable /\ StartJob_attempts[s] < 2) THEN [Waiting EXCEPT ![s] = @ + 1] ELSE [Waiting EXCEPT ![s] = @ + 1]
  /\ UNCHANGED <<Agent, Budget, Card, Disk, Host, Job, JobRecord, Process, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Unknown_StartJob(s) ==
  /\ StartJob_running[s]
  /\ Quiescent
  /\ "unknown" \in Outcomes["StartJob"]
  /\ StartJob_mark' = [StartJob_mark EXCEPT ![s] = IF FALSE THEN @ ELSE TRUE]
  /\ StartJob_moved' = [StartJob_moved EXCEPT ![s] = IF FALSE THEN @ ELSE {}]
  /\ StartJob_parked' = [StartJob_parked EXCEPT ![s] = IF FALSE THEN @ ELSE @ + 1]
  /\ StartJob_running' = [StartJob_running EXCEPT ![s] = FALSE]
  /\ StartJob_unk' = [StartJob_unk EXCEPT ![s] = TRUE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_attempts, StartJob_retry, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Abandon_StartJob(s) ==
  /\ StartJob_running[s]
  /\ "abandon" \in Timers
  /\ StartJob_attempts[s] = 0
  /\ StartJob_mark' = [StartJob_mark EXCEPT ![s] = IF FALSE THEN @ ELSE TRUE]
  /\ StartJob_moved' = [StartJob_moved EXCEPT ![s] = IF FALSE THEN @ ELSE {}]
  /\ StartJob_parked' = [StartJob_parked EXCEPT ![s] = IF FALSE THEN @ ELSE @ + 1]
  /\ StartJob_running' = [StartJob_running EXCEPT ![s] = FALSE]
  /\ StartJob_unk' = [StartJob_unk EXCEPT ![s] = TRUE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_attempts, StartJob_retry, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

RunTimeout_StartJob(s) ==
  /\ StartJob_attempts[s] > 0
  /\ "run" \in Timers
  /\ Stalled' = [Stalled EXCEPT ![s] = TRUE]
  /\ StartJob_attempts' = [StartJob_attempts EXCEPT ![s] = 0]
  /\ StartJob_retry' = [StartJob_retry EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Waiting, StartJob_running, StartJob_parked, StartJob_moved, StartJob_mark, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

RetryDue_StartJob(s) ==
  /\ StartJob_retry[s]
  /\ "retry" \in Timers
  /\ StartJob_mark' = [StartJob_mark EXCEPT ![s] = FALSE]
  /\ StartJob_retry' = [StartJob_retry EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

ResolveRetry_StartJob(s) ==
  \E m \in SUBSET {"Budget", "Job"} :
    /\ StartJob_parked[s] > 0
    /\ ~StartJob_running[s]
    /\ Quiescent
    /\ "retry" \in Resolutions
    /\ StartJob_moved' = [StartJob_moved EXCEPT ![s] = m]
    /\ StartJob_parked' = [StartJob_parked EXCEPT ![s] = @ - 1]
    /\ StartJob_running' = [StartJob_running EXCEPT ![s] = TRUE]
    /\ StartJob_unk' = [StartJob_unk EXCEPT ![s] = TRUE]
    /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_attempts, StartJob_mark, StartJob_retry, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

ResolveVerify_StartJob(s) ==
  \E m \in SUBSET {"Budget", "Job"} :
    /\ StartJob_parked[s] > 0
    /\ ~StartJob_running[s]
    /\ Quiescent
    /\ "verify" \in Resolutions
    /\ Job' = IF m = {} THEN [Job EXCEPT ![s] = TRUE] ELSE Job
    /\ StartJob_attempts' = [StartJob_attempts EXCEPT ![s] = 0]
    /\ StartJob_mark' = IF m = {} THEN [StartJob_mark EXCEPT ![s] = FALSE] ELSE StartJob_mark
    /\ StartJob_parked' = [StartJob_parked EXCEPT ![s] = @ - 1]
    /\ StartJob_retry' = [StartJob_retry EXCEPT ![s] = FALSE]
    /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_moved, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

ResolveDrop_StartJob(s) ==
  /\ StartJob_parked[s] > 0
  /\ ~StartJob_running[s]
  /\ Quiescent
  /\ "drop" \in Resolutions
  /\ Stalled' = IF StartJob_attempts[s] > 0 THEN [Stalled EXCEPT ![s] = TRUE] ELSE Stalled
  /\ StartJob_attempts' = [StartJob_attempts EXCEPT ![s] = 0]
  /\ StartJob_parked' = [StartJob_parked EXCEPT ![s] = @ - 1]
  /\ StartJob_retry' = [StartJob_retry EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Waiting, StartJob_running, StartJob_moved, StartJob_mark, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Done_Handoff(s) ==
  /\ Handoff_running[s]
  /\ Quiescent
  /\ "ok" \in Outcomes["Handoff"]
  /\ Handoff_attempts' = [Handoff_attempts EXCEPT ![s] = 0]
  /\ Handoff_mark' = IF Handoff_moved[s] = {} THEN [Handoff_mark EXCEPT ![s] = FALSE] ELSE Handoff_mark
  /\ Handoff_moved' = [Handoff_moved EXCEPT ![s] = {}]
  /\ Handoff_retry' = [Handoff_retry EXCEPT ![s] = FALSE]
  /\ Handoff_running' = [Handoff_running EXCEPT ![s] = FALSE]
  /\ Waiting' = IF Handoff_moved[s] = {} THEN [Waiting EXCEPT ![s] = @ + 1] ELSE Waiting
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_parked, Handoff_unk>>

Failed_Handoff(s, retryable) ==
  /\ Handoff_running[s]
  /\ Quiescent
  /\ retryable \in (IF "failed" \in Outcomes["Handoff"] THEN {FALSE} ELSE {}) \cup (IF "failed_retryable" \in Outcomes["Handoff"] THEN {TRUE} ELSE {})
  /\ Handoff_attempts' = IF Handoff_unk[s] THEN Handoff_attempts ELSE [Handoff_attempts EXCEPT ![s] = IF Handoff_attempts[s] > 0 /\ (retryable /\ Handoff_attempts[s] < 3) THEN @ ELSE 0]
  /\ Handoff_mark' = IF Handoff_unk[s] THEN [Handoff_mark EXCEPT ![s] = TRUE] ELSE [Handoff_mark EXCEPT ![s] = TRUE]
  /\ Handoff_moved' = IF Handoff_unk[s] THEN [Handoff_moved EXCEPT ![s] = {}] ELSE [Handoff_moved EXCEPT ![s] = {}]
  /\ Handoff_parked' = IF Handoff_unk[s] THEN [Handoff_parked EXCEPT ![s] = @ + 1] ELSE Handoff_parked
  /\ Handoff_retry' = IF Handoff_unk[s] THEN Handoff_retry ELSE [Handoff_retry EXCEPT ![s] = Handoff_attempts[s] > 0 /\ (retryable /\ Handoff_attempts[s] < 3)]
  /\ Handoff_running' = IF Handoff_unk[s] THEN [Handoff_running EXCEPT ![s] = FALSE] ELSE [Handoff_running EXCEPT ![s] = FALSE]
  /\ Stalled' = IF Handoff_unk[s] THEN Stalled ELSE IF Handoff_attempts[s] > 0 /\ ~(retryable /\ Handoff_attempts[s] < 3) THEN [Stalled EXCEPT ![s] = TRUE] ELSE [Stalled EXCEPT ![s] = TRUE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_unk>>

Unknown_Handoff(s) ==
  /\ Handoff_running[s]
  /\ Quiescent
  /\ "unknown" \in Outcomes["Handoff"]
  /\ Handoff_attempts' = [Handoff_attempts EXCEPT ![s] = IF (Handoff_attempts[s] > 0 /\ Handoff_attempts[s] < 3) THEN @ + 1 ELSE @]
  /\ Handoff_mark' = [Handoff_mark EXCEPT ![s] = IF (Handoff_attempts[s] > 0 /\ Handoff_attempts[s] < 3) THEN @ ELSE TRUE]
  /\ Handoff_moved' = [Handoff_moved EXCEPT ![s] = IF (Handoff_attempts[s] > 0 /\ Handoff_attempts[s] < 3) THEN @ ELSE {}]
  /\ Handoff_parked' = [Handoff_parked EXCEPT ![s] = IF (Handoff_attempts[s] > 0 /\ Handoff_attempts[s] < 3) THEN @ ELSE @ + 1]
  /\ Handoff_running' = [Handoff_running EXCEPT ![s] = (Handoff_attempts[s] > 0 /\ Handoff_attempts[s] < 3)]
  /\ Handoff_unk' = [Handoff_unk EXCEPT ![s] = TRUE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_retry>>

RunTimeout_Handoff(s) ==
  /\ Handoff_attempts[s] > 0
  /\ "run" \in Timers
  /\ Handoff_attempts' = [Handoff_attempts EXCEPT ![s] = 0]
  /\ Handoff_retry' = [Handoff_retry EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_moved, Handoff_mark, Handoff_unk>>

RetryDue_Handoff(s) ==
  /\ Handoff_retry[s]
  /\ "retry" \in Timers
  /\ Handoff_mark' = [Handoff_mark EXCEPT ![s] = FALSE]
  /\ Handoff_retry' = [Handoff_retry EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_unk>>

ResolveRetry_Handoff(s) ==
  \E m \in SUBSET {"Card"} :
    /\ Handoff_parked[s] > 0
    /\ ~Handoff_running[s]
    /\ Quiescent
    /\ "retry" \in Resolutions
    /\ Handoff_moved' = [Handoff_moved EXCEPT ![s] = m]
    /\ Handoff_parked' = [Handoff_parked EXCEPT ![s] = @ - 1]
    /\ Handoff_running' = [Handoff_running EXCEPT ![s] = TRUE]
    /\ Handoff_unk' = [Handoff_unk EXCEPT ![s] = TRUE]
    /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_attempts, Handoff_mark, Handoff_retry>>

ResolveVerify_Handoff(s) ==
  \E m \in SUBSET {"Card"} :
    /\ Handoff_parked[s] > 0
    /\ ~Handoff_running[s]
    /\ Quiescent
    /\ "verify" \in Resolutions
    /\ Handoff_attempts' = [Handoff_attempts EXCEPT ![s] = 0]
    /\ Handoff_mark' = IF m = {} THEN [Handoff_mark EXCEPT ![s] = FALSE] ELSE Handoff_mark
    /\ Handoff_parked' = [Handoff_parked EXCEPT ![s] = @ - 1]
    /\ Handoff_retry' = [Handoff_retry EXCEPT ![s] = FALSE]
    /\ Waiting' = IF m = {} THEN [Waiting EXCEPT ![s] = @ + 1] ELSE Waiting
    /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_moved, Handoff_unk>>

ResolveDrop_Handoff(s) ==
  /\ Handoff_parked[s] > 0
  /\ ~Handoff_running[s]
  /\ Quiescent
  /\ "drop" \in Resolutions
  /\ Handoff_attempts' = [Handoff_attempts EXCEPT ![s] = 0]
  /\ Handoff_parked' = [Handoff_parked EXCEPT ![s] = @ - 1]
  /\ Handoff_retry' = [Handoff_retry EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_moved, Handoff_mark, Handoff_unk>>

Write_Agent(s) ==
  /\ Quiescent
  /\ "Agent" \in Written
  /\ Agent' = [Agent EXCEPT ![s] = TRUE]
  /\ UNCHANGED <<Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Retract_Agent(s) ==
  /\ Quiescent
  /\ "Agent" \in Retracted
  /\ Agent[s]
  /\ Agent' = [Agent EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Write_Budget(s) ==
  /\ Quiescent
  /\ "Budget" \in Written
  /\ Budget' = [Budget EXCEPT ![s] = TRUE]
  /\ StartJob_moved' = [StartJob_moved EXCEPT ![s] = IF StartJob_running[s] THEN @ \cup {"Budget"} ELSE @]
  /\ UNCHANGED <<Agent, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Retract_Budget(s) ==
  /\ Quiescent
  /\ "Budget" \in Retracted
  /\ Budget[s]
  /\ Budget' = [Budget EXCEPT ![s] = FALSE]
  /\ StartJob_moved' = [StartJob_moved EXCEPT ![s] = IF StartJob_running[s] THEN @ \cup {"Budget"} ELSE @]
  /\ UNCHANGED <<Agent, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Write_Card(s) ==
  /\ Quiescent
  /\ "Card" \in Written
  /\ Card' = [Card EXCEPT ![s] = TRUE]
  /\ Handoff_moved' = [Handoff_moved EXCEPT ![s] = IF Handoff_running[s] THEN @ \cup {"Card"} ELSE @]
  /\ UNCHANGED <<Agent, Budget, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_mark, Handoff_retry, Handoff_unk>>

Retract_Card(s) ==
  /\ Quiescent
  /\ "Card" \in Retracted
  /\ Card[s]
  /\ Card' = [Card EXCEPT ![s] = FALSE]
  /\ Handoff_moved' = [Handoff_moved EXCEPT ![s] = IF Handoff_running[s] THEN @ \cup {"Card"} ELSE @]
  /\ UNCHANGED <<Agent, Budget, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_mark, Handoff_retry, Handoff_unk>>

Write_Disk(s) ==
  /\ Quiescent
  /\ "Disk" \in Written
  /\ Disk' = [Disk EXCEPT ![s] = TRUE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Retract_Disk(s) ==
  /\ Quiescent
  /\ "Disk" \in Retracted
  /\ Disk[s]
  /\ Disk' = [Disk EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Host, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Write_Host(s) ==
  /\ Quiescent
  /\ "Host" \in Written
  /\ Host' = [Host EXCEPT ![s] = TRUE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Retract_Host(s) ==
  /\ Quiescent
  /\ "Host" \in Retracted
  /\ Host[s]
  /\ Host' = [Host EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Job, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Write_Job(s) ==
  /\ Quiescent
  /\ "Job" \in Written
  /\ Job' = [Job EXCEPT ![s] = TRUE]
  /\ StartJob_moved' = [StartJob_moved EXCEPT ![s] = IF StartJob_running[s] THEN @ \cup {"Job"} ELSE @]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Retract_Job(s) ==
  /\ Quiescent
  /\ "Job" \in Retracted
  /\ Job[s]
  /\ Job' = [Job EXCEPT ![s] = FALSE]
  /\ StartJob_moved' = [StartJob_moved EXCEPT ![s] = IF StartJob_running[s] THEN @ \cup {"Job"} ELSE @]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, JobRecord, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Write_JobRecord(s) ==
  /\ Quiescent
  /\ "JobRecord" \in Written
  /\ JobRecord' = [JobRecord EXCEPT ![s] = TRUE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Retract_JobRecord(s) ==
  /\ Quiescent
  /\ "JobRecord" \in Retracted
  /\ JobRecord[s]
  /\ JobRecord' = [JobRecord EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, Process, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Write_Process(s) ==
  /\ Quiescent
  /\ "Process" \in Written
  /\ Process' = [Process EXCEPT ![s] = TRUE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Retract_Process(s) ==
  /\ Quiescent
  /\ "Process" \in Retracted
  /\ Process[s]
  /\ Process' = [Process EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Stalled, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Write_Stalled(s) ==
  /\ Quiescent
  /\ "Stalled" \in Written
  /\ Stalled' = [Stalled EXCEPT ![s] = TRUE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Retract_Stalled(s) ==
  /\ Quiescent
  /\ "Stalled" \in Retracted
  /\ Stalled[s]
  /\ Stalled' = [Stalled EXCEPT ![s] = FALSE]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Waiting, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Write_Waiting(s) ==
  /\ Quiescent
  /\ "Waiting" \in Written
  /\ Waiting' = [Waiting EXCEPT ![s] = @ + 1]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

Retract_Waiting(s) ==
  /\ Quiescent
  /\ "Waiting" \in Retracted
  /\ Waiting[s] > 0
  /\ Waiting' = [Waiting EXCEPT ![s] = @ - 1]
  /\ UNCHANGED <<Agent, Budget, Card, Cpu, Disk, Host, Job, JobRecord, Process, Stalled, StartJob_running, StartJob_parked, StartJob_attempts, StartJob_moved, StartJob_mark, StartJob_retry, StartJob_unk, Handoff_running, Handoff_parked, Handoff_attempts, Handoff_moved, Handoff_mark, Handoff_retry, Handoff_unk>>

AutoNext == \E s \in Subj :
  \/ Start_StartJob(s)
  \/ Fire_JobEnded(s)
  \/ Fire_AgentTurnEnded(s)
  \/ Fire_Stalled(s)
ManualNext == \E s \in Subj :
  \/ Fire_Continue(s)
  \/ Start_Handoff(s)
TimerNext == \E s \in Subj :
  \/ Abandon_StartJob(s)
  \/ RunTimeout_StartJob(s)
  \/ RetryDue_StartJob(s)
  \/ RunTimeout_Handoff(s)
  \/ RetryDue_Handoff(s)
CallNext == \E s \in Subj :
  \/ Done_StartJob(s)
  \/ \E r \in {FALSE, TRUE} : Failed_StartJob(s, r)
  \/ Unknown_StartJob(s)
  \/ ResolveRetry_StartJob(s)
  \/ ResolveVerify_StartJob(s)
  \/ ResolveDrop_StartJob(s)
  \/ Done_Handoff(s)
  \/ \E r \in {FALSE, TRUE} : Failed_Handoff(s, r)
  \/ Unknown_Handoff(s)
  \/ ResolveRetry_Handoff(s)
  \/ ResolveVerify_Handoff(s)
  \/ ResolveDrop_Handoff(s)
  \/ Write_Agent(s)
  \/ Retract_Agent(s)
  \/ Write_Budget(s)
  \/ Retract_Budget(s)
  \/ Write_Card(s)
  \/ Retract_Card(s)
  \/ Write_Disk(s)
  \/ Retract_Disk(s)
  \/ Write_Host(s)
  \/ Retract_Host(s)
  \/ Write_Job(s)
  \/ Retract_Job(s)
  \/ Write_JobRecord(s)
  \/ Retract_JobRecord(s)
  \/ Write_Process(s)
  \/ Retract_Process(s)
  \/ Write_Stalled(s)
  \/ Retract_Stalled(s)
  \/ Write_Waiting(s)
  \/ Retract_Waiting(s)
\* The executor fires auto transitions; the user fires manual ones.
NetNext == AutoNext \/ ManualNext
EnvNext == TimerNext \/ CallNext
Next == NetNext \/ EnvNext
Spec == Init /\ [][Next]_vars
\* The executor fires every enabled auto pair before it returns.
AutoFairness == \A s \in Subj : WF_vars(Start_StartJob(s)) /\ WF_vars(Fire_JobEnded(s)) /\ WF_vars(Fire_AgentTurnEnded(s)) /\ WF_vars(Fire_Stalled(s))
====
