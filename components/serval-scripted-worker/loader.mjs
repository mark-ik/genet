// Memory64 validation module: `(module (memory i64 1))`. Capability detection is
// deliberately binary validation, never browser-name or user-agent detection.
export const MEMORY64_PROBE = new Uint8Array([
  0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00,
  0x05, 0x03, 0x01, 0x04, 0x01,
]);

export function supportsMemory64(webAssembly = WebAssembly) {
  return webAssembly.validate(MEMORY64_PROBE);
}

function initializeCandidate({ WorkerCtor, bootstrapUrl, artifact, initial, timeoutMs }) {
  return new Promise((resolve, reject) => {
    const worker = new WorkerCtor(bootstrapUrl, { type: "module", name: `serval-${artifact.engine}` });
    let settled = false;
    const fail = (error) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      worker.terminate();
      reject(error instanceof Error ? error : new Error(String(error)));
    };
    const timer = setTimeout(() => fail(new Error(`${artifact.engine} initialization timed out`)), timeoutMs);
    worker.onerror = (event) => fail(event.error ?? new Error(event.message ?? "worker error"));
    worker.onmessage = ({ data }) => {
      if (data.id !== 0) return;
      if (data.type === "error") return fail(new Error(data.message));
      if (data.type !== "ready") return;
      settled = true;
      clearTimeout(timer);
      resolve({ worker, diagnostic: data });
    };
    worker.postMessage({
      id: 0,
      type: "initialize",
      moduleUrl: artifact.moduleUrl,
      wasmUrl: artifact.wasmUrl,
      html: initial.html ?? "",
      baseUrl: initial.baseUrl ?? "about:blank",
    });
  });
}

/// Start the preferred backend. A wasm64 validation success only admits Nova to
/// the attempt list; any Nova worker/glue/instantiation failure retries the same
/// initialization input on Boa.
export async function startScriptedWorker({
  artifacts,
  initial,
  bootstrapUrl = new URL("./worker-bootstrap.mjs", import.meta.url),
  WorkerCtor = Worker,
  webAssembly = WebAssembly,
  timeoutMs = 15_000,
}) {
  const candidates = supportsMemory64(webAssembly)
    ? [artifacts.nova, artifacts.boa]
    : [artifacts.boa];
  let novaFailure;
  for (const artifact of candidates) {
    try {
      const selected = await initializeCandidate({
        WorkerCtor,
        bootstrapUrl,
        artifact,
        initial,
        timeoutMs,
      });
      return { ...selected, novaFailure };
    } catch (error) {
      if (artifact.engine !== "nova") throw error;
      novaFailure = error;
    }
  }
  throw novaFailure ?? new Error("no scripted worker artifact available");
}
