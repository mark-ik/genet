// Shared event-propagation state for the native dispatcher.
//
// The native dispatch path sends an event *by value* (a `Clone`) to each
// listener it routes to, so a handler cannot mutate one shared event struct the
// way the JS DOM dispatcher does (`event.stopPropagation()` flipping a field
// every later listener sees). To get the same semantics here, the cancellation
// flags live behind a shared `Rc<Cell<…>>`: cloning the event clones the
// *handle*, not the flags, so a handler that calls `stop_propagation()` /
// `prevent_default()` on the event it received is seen by the dispatch loop and
// by every other clone.
//
// This is the native twin of `dom.rs`'s `__stop` / `__stopImmediate` /
// `__canceled` on the JS `Event`. See
// `docs/history/2026-06-01_event_model_convergence_plan.md` — both dispatchers satisfy
// one propagation/cancellation contract.

use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, Debug, Default)]
struct Flags {
    stop: bool,
    stop_immediate: bool,
    default_prevented: bool,
}

/// Shared, clone-through cancellation state for one dispatched event.
///
/// Embedded in the native event payloads ([`PointerClick`](crate::PointerClick),
/// [`KeyEvent`](crate::KeyEvent)). All clones of an event share one flag cell, so
/// a handler calling [`stop_propagation`](Self::stop_propagation) /
/// [`prevent_default`](Self::prevent_default) is observed by the dispatch loop
/// (which checks [`stopped`](Self::stopped) between routed paths) and by the host
/// (which reads [`default_prevented`](Self::default_prevented) to decide whether
/// to run the event's default action).
#[derive(Clone, Debug)]
pub struct Propagation {
    flags: Rc<Cell<Flags>>,
}

impl Default for Propagation {
    fn default() -> Self {
        Self::new()
    }
}

impl Propagation {
    /// A fresh propagation state (nothing stopped or prevented). One per
    /// dispatch; the event's clones share it.
    pub fn new() -> Self {
        Self {
            flags: Rc::new(Cell::new(Flags::default())),
        }
    }

    /// Stop the event reaching any *later* node in the propagation path. The
    /// current node's other listeners still run (this dispatcher registers at
    /// most one listener per node per type, so that distinction rarely bites —
    /// but the contract matches the JS side). Mirrors `Event.stopPropagation`.
    pub fn stop_propagation(&self) {
        let mut f = self.flags.get();
        f.stop = true;
        self.flags.set(f);
    }

    /// Stop the event reaching any later listener at all, including remaining
    /// listeners on the current node — and imply `stop_propagation`. Mirrors
    /// `Event.stopImmediatePropagation`.
    pub fn stop_immediate_propagation(&self) {
        let mut f = self.flags.get();
        f.stop = true;
        f.stop_immediate = true;
        self.flags.set(f);
    }

    /// Mark the event's default action as canceled. The host reads
    /// [`default_prevented`](Self::default_prevented) after dispatch to decide
    /// whether to run that default (form activation, drag start, caret move).
    /// Mirrors `Event.preventDefault` (the native event is always cancelable —
    /// Genet has no passive-listener distinction yet).
    pub fn prevent_default(&self) {
        let mut f = self.flags.get();
        f.default_prevented = true;
        self.flags.set(f);
    }

    /// Whether propagation has been stopped (`stop_propagation` or
    /// `stop_immediate_propagation`). The dispatch loop checks this between
    /// routed paths to halt the capture/bubble walk.
    pub fn stopped(&self) -> bool {
        self.flags.get().stop
    }

    /// Whether `prevent_default` was called. Read by the host after dispatch.
    pub fn default_prevented(&self) -> bool {
        self.flags.get().default_prevented
    }
}
