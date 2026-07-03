---- MODULE PostMessage ----
EXTENDS FiniteSets, Naturals, Sequences

CONSTANT Trace

VARIABLE pc

BoundarySet ==
    {"eval", "post_message", "run_event_loop", "pump_microtasks", "timer_task"}

PhaseSet == {"start", "end", "error", "performed", "enqueue", "deliver"}

EventOK(e) ==
    /\ e.boundary \in BoundarySet
    /\ e.phase \in PhaseSet

IsEvent(boundary, phase, i) ==
    /\ i \in 1..Len(Trace)
    /\ Trace[i].boundary = boundary
    /\ Trace[i].phase = phase

FirstIndex(S) == CHOOSE n \in S: \A m \in S: n <= m

FirstOrZero(S) == IF S = {} THEN 0 ELSE FirstIndex(S)

EnqueueEvents ==
    {i \in 1..Len(Trace) : IsEvent("post_message", "enqueue", i)}

MessageIds ==
    {Trace[i].detail : i \in EnqueueEvents}

EnqueueIndices(id) ==
    {i \in 1..Len(Trace) :
        /\ IsEvent("post_message", "enqueue", i)
        /\ Trace[i].detail = id}

DeliverIndices(id) ==
    {i \in 1..Len(Trace) :
        /\ IsEvent("post_message", "deliver", i)
        /\ Trace[i].detail = id}

EvalEndIndicesAfter(i) ==
    {k \in 1..Len(Trace) :
        /\ k > i
        /\ IsEvent("eval", "end", k)}

RunEventLoopStartsBefore(i) ==
    {k \in 1..Len(Trace) :
        /\ k < i
        /\ IsEvent("run_event_loop", "start", k)}

RunEventLoopEndsAfter(i) ==
    {k \in 1..Len(Trace) :
        /\ k > i
        /\ IsEvent("run_event_loop", "end", k)}

TimerTasksAfter(i) ==
    {k \in 1..Len(Trace) :
        /\ k > i
        /\ IsEvent("timer_task", "performed", k)}

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

UniqueEnqueueIds ==
    \A id \in MessageIds:
        Cardinality(EnqueueIndices(id)) = 1

ExactlyOnceDelivery ==
    \A id \in MessageIds:
        /\ Cardinality(DeliverIndices(id)) = 1
        /\ FirstOrZero(EnqueueIndices(id)) < FirstOrZero(DeliverIndices(id))

AsyncDelivery ==
    \A id \in MessageIds:
        LET enqueue == FirstOrZero(EnqueueIndices(id))
            deliver == FirstOrZero(DeliverIndices(id))
            callerEnd == FirstOrZero(EvalEndIndicesAfter(enqueue))
        IN /\ enqueue > 0
           /\ callerEnd > enqueue
           /\ deliver > callerEnd

DeliveriesRunInsideEventLoop ==
    \A id \in MessageIds:
        LET deliver == FirstOrZero(DeliverIndices(id))
        IN /\ deliver > 0
           /\ FirstOrZero(RunEventLoopStartsBefore(deliver)) > 0
           /\ FirstOrZero(RunEventLoopEndsAfter(deliver)) > deliver

TimerBackedDelivery ==
    \A id \in MessageIds:
        LET deliver == FirstOrZero(DeliverIndices(id))
        IN /\ deliver > 0
           /\ FirstOrZero(TimerTasksAfter(deliver)) > deliver

====
