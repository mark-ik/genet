---- MODULE SchedulerTraceData ----
EXTENDS Sequences

Trace ==
    <<
      [seq |-> 1, boundary |-> "eval", phase |-> "start", detail |-> ""],
      [seq |-> 2, boundary |-> "eval", phase |-> "end", detail |-> ""],
      [seq |-> 3, boundary |-> "run_event_loop", phase |-> "start", detail |-> "budget=10"],
      [seq |-> 4, boundary |-> "pump_microtasks", phase |-> "start", detail |-> ""],
      [seq |-> 5, boundary |-> "pump_microtasks", phase |-> "end", detail |-> ""],
      [seq |-> 6, boundary |-> "timer_task", phase |-> "performed", detail |-> "fired=1"],
      [seq |-> 7, boundary |-> "pump_microtasks", phase |-> "start", detail |-> ""],
      [seq |-> 8, boundary |-> "pump_microtasks", phase |-> "end", detail |-> ""],
      [seq |-> 9, boundary |-> "run_event_loop", phase |-> "end", detail |-> "fired=1"],
      [seq |-> 10, boundary |-> "dispatch_event", phase |-> "start", detail |-> "type=click;node=1"],
      [seq |-> 11, boundary |-> "pump_microtasks", phase |-> "start", detail |-> ""],
      [seq |-> 12, boundary |-> "pump_microtasks", phase |-> "end", detail |-> ""],
      [seq |-> 13, boundary |-> "dispatch_event", phase |-> "end", detail |-> "proceed=false"],
      [seq |-> 14, boundary |-> "run_timers", phase |-> "start", detail |-> "budget=10;now_ms=0"],
      [seq |-> 15, boundary |-> "pump_microtasks", phase |-> "start", detail |-> ""],
      [seq |-> 16, boundary |-> "pump_microtasks", phase |-> "end", detail |-> ""],
      [seq |-> 17, boundary |-> "timer_task", phase |-> "performed", detail |-> "fired=1"],
      [seq |-> 18, boundary |-> "pump_microtasks", phase |-> "start", detail |-> ""],
      [seq |-> 19, boundary |-> "pump_microtasks", phase |-> "end", detail |-> ""],
      [seq |-> 20, boundary |-> "run_timers", phase |-> "end", detail |-> "fired=1"]
    >>

====
