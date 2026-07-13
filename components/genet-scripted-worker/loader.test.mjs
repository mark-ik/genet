import assert from "node:assert/strict";
import test from "node:test";

import { MEMORY64_PROBE, startScriptedWorker, supportsMemory64 } from "./loader.mjs";

const artifacts = {
  nova: { engine: "nova", moduleUrl: "nova.js", wasmUrl: "nova_bg.wasm" },
  boa: { engine: "boa", moduleUrl: "boa.js", wasmUrl: "boa_bg.wasm" },
};

class FakeWorker {
  static instances = [];

  constructor(_url, options) {
    this.options = options;
    this.messages = [];
    this.terminated = false;
    FakeWorker.instances.push(this);
  }

  postMessage(message) {
    this.messages.push(structuredClone(message));
    queueMicrotask(() => {
      if (message.moduleUrl === artifacts.nova.moduleUrl) {
        this.onmessage({ data: { id: 0, type: "error", message: "forced nova failure" } });
      } else {
        this.onmessage({
          data: {
            id: 0,
            type: "ready",
            engine: "boa",
            pointerWidth: 32,
            sharedMemory: false,
          },
        });
      }
    });
  }

  terminate() {
    this.terminated = true;
  }
}

test("the checked-in probe is a valid wasm module", () => {
  assert.equal(MEMORY64_PROBE.slice(0, 4).toString(), "0,97,115,109");
  assert.equal(typeof supportsMemory64(), "boolean");
});

test("nova initialization failure retries boa with identical input", async () => {
  FakeWorker.instances = [];
  const initial = { html: "<body>same input</body>", baseUrl: "https://example.test/" };
  const selected = await startScriptedWorker({
    artifacts,
    initial,
    WorkerCtor: FakeWorker,
    webAssembly: { validate: () => true },
  });

  assert.equal(selected.diagnostic.engine, "boa");
  assert.match(selected.novaFailure.message, /forced nova failure/);
  assert.equal(FakeWorker.instances.length, 2);
  assert.equal(FakeWorker.instances[0].terminated, true);
  assert.deepEqual(FakeWorker.instances[0].messages[0].html, initial.html);
  assert.deepEqual(FakeWorker.instances[1].messages[0].html, initial.html);
  assert.deepEqual(FakeWorker.instances[1].messages[0].baseUrl, initial.baseUrl);
});

test("failed memory64 validation selects boa without attempting nova", async () => {
  FakeWorker.instances = [];
  const selected = await startScriptedWorker({
    artifacts,
    initial: { html: "<body></body>" },
    WorkerCtor: FakeWorker,
    webAssembly: { validate: () => false },
  });

  assert.equal(selected.diagnostic.engine, "boa");
  assert.equal(FakeWorker.instances.length, 1);
  assert.match(FakeWorker.instances[0].options.name, /boa/);
});
