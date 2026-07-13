// Private content-worker protocol. The host owns this Worker and may terminate it
// at any time; the Rust session owns the engine/runtime/DOM while it is alive.
let session;

const reply = (id, type, payload = {}) => self.postMessage({ id, type, ...payload });

self.onmessage = async ({ data }) => {
  const { id = 0, type } = data;
  try {
    switch (type) {
      case "initialize": {
        const bindings = await import(data.moduleUrl);
        await bindings.default(data.wasmUrl);
        session = new bindings.WorkerSession(data.html ?? "");
        reply(id, "ready", {
          engine: session.engine,
          pointerWidth: session.pointerWidth,
          sharedMemory: session.sharedMemory,
          snapshot: session.snapshot(),
        });
        break;
      }
      case "evaluate":
        reply(id, "result", { snapshot: session.evaluate(data.source) });
        break;
      case "evaluate-module":
        reply(id, "result", {
          snapshot: session.evaluateModule(data.source, data.baseUrl ?? "about:blank"),
        });
        break;
      case "dispatch-event":
        reply(id, "result", {
          proceed: session.dispatchEvent(String(data.nodeId), data.eventType),
          snapshot: session.snapshot(),
        });
        break;
      case "snapshot":
        reply(id, "result", { snapshot: session.snapshot() });
        break;
      case "collect-garbage":
        reply(id, "result", { collection: JSON.parse(session.collectGarbage()) });
        break;
      case "shutdown":
        session?.free();
        session = undefined;
        reply(id, "shutdown");
        self.close();
        break;
      default:
        throw new Error(`unknown scripted-worker message: ${type}`);
    }
  } catch (error) {
    reply(id, "error", { message: error instanceof Error ? error.message : String(error) });
  }
};
