// Notes the requests that failed, for the diagnostics report.
//
// Only failures are kept: the method, the route pattern, the status, how long
// it took and the server's request id. Never a body, a header other than the
// request id, or a query string, which is also where share keys could be.

import { record } from "./log.js";
import { routePattern, scrubMessage } from "./redact.js";

const urlOf = (input) => (typeof input === "string" ? input : input?.url ?? input?.href ?? String(input));
const methodOf = (input, init) => String(init?.method || input?.method || "GET").toUpperCase();

/// Wraps `target.fetch` so failed requests are recorded. Responses and errors
/// pass through unchanged. Returns the way to put the original back.
export function watchNetwork(target = globalThis) {
  const original = target.fetch;
  if (typeof original !== "function") return () => {};
  const wrapped = async function (input, init) {
    const started = Date.now();
    const method = methodOf(input, init);
    const route = routePattern(urlOf(input));
    let response;
    try {
      response = await original.call(target, input, init);
    } catch (error) {
      record({ kind: "request", method, route, status: 0, ms: Date.now() - started, requestId: "", error: scrubMessage(error?.message) });
      throw error;
    }
    if (response.status >= 400) {
      record({
        kind: "request", method, route, status: response.status, ms: Date.now() - started,
        requestId: response.headers?.get?.("x-request-id") || "", error: "",
      });
    }
    return response;
  };
  target.fetch = wrapped;
  return () => {
    if (target.fetch === wrapped) target.fetch = original;
  };
}
