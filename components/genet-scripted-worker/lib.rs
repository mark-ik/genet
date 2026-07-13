/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! Backend-exclusive Web Worker entry for the scripted-document runtime.

use genet_scripted::ScriptedDocument;
use wasm_bindgen::prelude::*;

#[cfg(all(feature = "engine-boa", feature = "engine-nova"))]
compile_error!("engine-boa and engine-nova are mutually exclusive");

#[cfg(not(any(feature = "engine-boa", feature = "engine-nova")))]
compile_error!("select exactly one of engine-boa or engine-nova");

#[cfg(all(feature = "engine-nova", not(target_pointer_width = "64")))]
compile_error!("engine-nova requires a 64-bit target (native64 or wasm64)");

#[cfg(feature = "engine-boa")]
type SelectedEngine = script_engine_boa::BoaEngine;

#[cfg(feature = "engine-nova")]
type SelectedEngine = script_engine_nova::NovaEngine;

#[cfg(feature = "engine-boa")]
const ENGINE_NAME: &str = "boa";

#[cfg(feature = "engine-nova")]
const ENGINE_NAME: &str = "nova";

/// One content worker's engine, runtime, DOM, reflector table, and GC cadence.
#[wasm_bindgen]
pub struct WorkerSession {
    document: ScriptedDocument<SelectedEngine>,
}

#[wasm_bindgen]
impl WorkerSession {
    #[wasm_bindgen(constructor)]
    pub fn new(html: &str) -> Result<WorkerSession, JsValue> {
        let mut document = ScriptedDocument::parse(html).map_err(js_error)?;
        // Baseline capability profile: no shared memory and no JS Atomics. Boa may
        // provide these builtins by default, so mask them as well as compiling Nova
        // without the corresponding features.
        document
            .evaluate("globalThis.SharedArrayBuffer = undefined; globalThis.Atomics = undefined;")
            .map_err(js_error)?;
        Ok(Self { document })
    }

    #[wasm_bindgen(getter)]
    pub fn engine(&self) -> String {
        ENGINE_NAME.to_string()
    }

    #[wasm_bindgen(getter, js_name = pointerWidth)]
    pub fn pointer_width(&self) -> u32 {
        usize::BITS
    }

    #[wasm_bindgen(getter, js_name = sharedMemory)]
    pub fn shared_memory(&self) -> bool {
        false
    }

    /// Evaluate a classic script and return the neutral post-mutation DOM snapshot.
    pub fn evaluate(&mut self, source: &str) -> Result<String, JsValue> {
        self.document.evaluate(source).map_err(js_error)?;
        Ok(self.document.dom_snapshot())
    }

    /// Evaluate a module and return the neutral post-mutation DOM snapshot.
    #[wasm_bindgen(js_name = evaluateModule)]
    pub fn evaluate_module(&mut self, source: &str, base_url: &str) -> Result<String, JsValue> {
        self.document
            .evaluate_module(source, base_url)
            .map_err(js_error)?;
        Ok(self.document.dom_snapshot())
    }

    /// Dispatch an event. Node ids cross the JS boundary as decimal strings so the
    /// protocol never truncates a wasm64 `usize` through JavaScript's Number type.
    #[wasm_bindgen(js_name = dispatchEvent)]
    pub fn dispatch_event(&mut self, node_id: &str, event_type: &str) -> Result<bool, JsValue> {
        let node_id = node_id
            .parse::<usize>()
            .map_err(|_| js_error("nodeId must be a decimal pointer-width integer"))?;
        self.document
            .dispatch_event(node_id, event_type)
            .map_err(js_error)
    }

    pub fn snapshot(&self) -> String {
        self.document.dom_snapshot()
    }

    /// Force an engine/reflector/DOM collection tick. The two counts are returned as
    /// JSON to keep the generated wasm-bindgen ABI identical on wasm32 and wasm64.
    #[wasm_bindgen(js_name = collectGarbage)]
    pub fn collect_garbage(&mut self) -> String {
        let (reflectors, nodes) = self.document.collect_garbage();
        format!(r#"{{"reflectorsRetired":{reflectors},"nodesCollected":{nodes}}}"#)
    }
}

fn js_error(message: impl AsRef<str>) -> JsValue {
    JsValue::from_str(message.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handshake_reports_selected_backend_and_pointer_width() {
        let session = WorkerSession::new("<body><p id='x'>before</p></body>").unwrap();
        assert_eq!(session.engine(), ENGINE_NAME);
        assert_eq!(session.pointer_width(), usize::BITS);
        assert!(!session.shared_memory());
    }

    #[test]
    fn evaluation_mutates_the_neutral_snapshot() {
        let mut session = WorkerSession::new("<body><p id='x'>before</p></body>").unwrap();
        let snapshot = session
            .evaluate("document.getElementById('x').setAttribute('data-state', 'after')")
            .unwrap();
        assert!(snapshot.contains("data-state=\"after\""), "{snapshot}");
    }

    #[test]
    fn baseline_profile_keeps_typed_arrays_and_masks_shared_memory() {
        let mut session = WorkerSession::new("<body><p id='x'></p></body>").unwrap();
        let snapshot = session
            .evaluate(
                "const bytes = new Uint8Array(new ArrayBuffer(2));\
                 bytes[1] = 7;\
                 document.getElementById('x').setAttribute(\
                   'data-profile', `${bytes[1]}:${typeof SharedArrayBuffer}:${typeof Atomics}`\
                 );",
            )
            .unwrap();
        assert!(snapshot.contains("7:undefined:undefined"), "{snapshot}");
    }

    #[test]
    fn microtasks_settle_before_the_snapshot_is_returned() {
        let mut session = WorkerSession::new("<body><p id='x'>before</p></body>").unwrap();
        let snapshot = session
            .evaluate(
                "Promise.resolve().then(() => {\
                   document.getElementById('x').setAttribute('data-state', 'settled');\
                 });",
            )
            .unwrap();
        assert!(snapshot.contains("settled"), "{snapshot}");
    }
}
