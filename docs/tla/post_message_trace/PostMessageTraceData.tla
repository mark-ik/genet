---- MODULE PostMessageTraceData ----
EXTENDS Sequences

Trace ==
    <<
      [seq |-> 1, boundary |-> "eval", phase |-> "start", detail |-> ""],
      [seq |-> 2, boundary |-> "post_message", phase |-> "enqueue", detail |-> "1"],
      [seq |-> 3, boundary |-> "eval", phase |-> "end", detail |-> ""],
      [seq |-> 4, boundary |-> "run_event_loop", phase |-> "start", detail |-> "budget=10"],
      [seq |-> 5, boundary |-> "pump_microtasks", phase |-> "start", detail |-> ""],
      [seq |-> 6, boundary |-> "pump_microtasks", phase |-> "end", detail |-> ""],
      [seq |-> 7, boundary |-> "post_message", phase |-> "deliver", detail |-> "1"],
      [seq |-> 8, boundary |-> "timer_task", phase |-> "performed", detail |-> "fired=1"],
      [seq |-> 9, boundary |-> "pump_microtasks", phase |-> "start", detail |-> ""],
      [seq |-> 10, boundary |-> "pump_microtasks", phase |-> "end", detail |-> ""],
      [seq |-> 11, boundary |-> "run_event_loop", phase |-> "end", detail |-> "fired=1"]
    >>

====
