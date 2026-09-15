import * as diagnosticsRule from "../diagnostics.js";
import { diagnosticContext } from "../assistant-review.js";

// Owns the three diagnostic streams shown by the reader. Render diagnostics
// have their own delayed-paint policy; bibliography and local-tool diagnostics
// are merged into the latest visible result without duplicating entries.
export function createRenderDiagnostics({ active, local, update, deliver, painterOptions = {} }) {
  let rendered = [];
  let bibliography = [];

  function refresh() {
    if (!active()) return;
    const seen = new Set();
    const combined = [...rendered, ...bibliography, ...local()].filter((item) => {
      const key = JSON.stringify([item.file, item.line, item.column, item.message]);
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });
    update(combined);
    deliver(combined);
  }

  const painter = diagnosticsRule.painter({
    ...painterOptions,
    paint(list) {
      if (!active()) return;
      rendered = list;
      refresh();
    },
  });

  return {
    painter,
    refresh,
    bibliography(result, request) {
      bibliography = (result?.diagnostics || []).map((item) =>
        diagnosticContext(item, { main: request?.main || "", texts: request?.texts || {} }, ""));
      refresh();
    },
  };
}
