---- MODULE SchedulerTrace ----
EXTENDS Naturals, Sequences, SchedulerTraceData

\* Trace is supplied by SchedulerTraceData.tla, generated from runtime NDJSON.

VARIABLE pc

BoundarySet ==
    {"eval", "dispatch_event", "run_event_loop", "run_timers",
     "pump_microtasks", "timer_task"}

PhaseSet == {"start", "end", "error", "performed"}

EventOK(e) ==
    /\ e.boundary \in BoundarySet
    /\ e.phase \in PhaseSet

Init == pc = 1

Next ==
    \/ /\ pc <= Len(Trace)
       /\ EventOK(Trace[pc])
       /\ pc' = pc + 1
    \/ /\ pc = Len(Trace) + 1
       /\ pc' = pc

Spec == Init /\ [][Next]_pc

TypeOK ==
    /\ pc \in 1..(Len(Trace) + 1)
    /\ \A i \in 1..Len(Trace):
        /\ Trace[i].seq = i
        /\ EventOK(Trace[i])

MicrotaskCheckpointsClose ==
    \A i \in 1..Len(Trace):
        (/\ Trace[i].boundary = "pump_microtasks"
         /\ Trace[i].phase = "start")
        => /\ i < Len(Trace)
           /\ Trace[i + 1].boundary = "pump_microtasks"
           /\ Trace[i + 1].phase \in {"end", "error"}

TimerTasksCheckpointBeforeNextTimer ==
    \A i \in 1..Len(Trace):
        (/\ Trace[i].boundary = "timer_task"
         /\ Trace[i].phase = "performed")
        => \E j \in (i + 1)..Len(Trace):
              /\ Trace[j].boundary = "pump_microtasks"
              /\ Trace[j].phase = "start"
              /\ \A k \in (i + 1)..(j - 1):
                    Trace[k].boundary /= "timer_task"

====
