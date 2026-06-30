// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The `testharness.js` results bridge: collect per-subtest results out of a
//! loaded harness into host state.
//!
//! `testharness.js` reports completion through `add_completion_callback(cb)`,
//! where `cb(tests, status)` receives the array of `Test` objects (each with
//! `name` / `status` / `message`). The bridge registers such a callback that
//! forwards each result to the `__reportResult` native sink, which records it in
//! [`HostState::results`]. [`Runtime::run_testharness`] drives the whole flow.
//!
//! Engine-neutral like the rest of the host surface (native sink + JS bootstrap,
//! over `CallCx`); validated on Boa + Nova.

use std::cell::RefCell;

use script_engine_api::{CallCx, NativeFn, ScriptEngine};

use crate::HostState;

/// One subtest's outcome, mirrored out of `testharness.js`. `status` is the
/// harness's numeric code: 0 PASS, 1 FAIL, 2 TIMEOUT, 3 NOTRUN,
/// 4 PRECONDITION_FAILED.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestResult {
    pub name: String,
    pub status: i64,
    pub message: Option<String>,
}

impl TestResult {
    /// Whether this subtest passed (status PASS).
    pub fn passed(&self) -> bool {
        self.status == 0
    }
}

/// Install the `__reportResult` native sink. The completion-callback JS (installed
/// by [`install_bridge`] after `testharness.js` loads) calls it per subtest.
pub(crate) fn install_report_sink<E: ScriptEngine>(engine: &mut E) -> Result<(), E::Error> {
    engine.set_function::<ReportResult>("__reportResult", 3)
}

/// Register the completion callback on a loaded `testharness.js`. Must run *after*
/// the harness is evaluated (it defines `add_completion_callback`).
pub(crate) fn install_bridge<E: ScriptEngine>(engine: &mut E) -> Result<(), E::Error> {
    engine.eval(BRIDGE_JS)?;
    Ok(())
}

/// `__reportResult(name, status, message)` — record one subtest into host state.
/// `status` and `message` arrive as strings (no number-minting primitive yet);
/// `message` is `"null"`/`"undefined"` when the harness has none.
struct ReportResult;
impl<E: ScriptEngine> NativeFn<E> for ReportResult {
    fn call(cx: &mut E::CallCx<'_>) -> Result<E::Value, E::Error> {
        let name_v = cx.arg(0);
        let name = cx.value_to_string(&name_v)?;
        let status_v = cx.arg(1);
        let status = cx.value_to_string(&status_v)?.parse::<i64>().unwrap_or(-1);
        let message_v = cx.arg(2);
        let message = match cx.value_to_string(&message_v)?.as_str() {
            "null" | "undefined" | "" => None,
            other => Some(other.to_string()),
        };
        if let Some(data) = cx.host_data() {
            if let Some(cell) = data.downcast_ref::<RefCell<HostState>>() {
                cell.borrow_mut().results.push(TestResult {
                    name,
                    status,
                    message,
                });
            }
        }
        Ok(cx.undefined())
    }
}

/// Disables the harness's HTML output (a headless runner reads results
/// programmatically; the output path renders a results table via DOM APIs we don't
/// implement, e.g. `createElementNS`), then registers a completion callback that
/// forwards each subtest to `__reportResult`.
const BRIDGE_JS: &str = r#"
setup({ output: false });
add_completion_callback(function(tests) {
  for (var i = 0; i < tests.length; i++) {
    __reportResult(String(tests[i].name), String(tests[i].status), String(tests[i].message));
  }
});
"#;
