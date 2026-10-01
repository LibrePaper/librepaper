// The document in Preview shares the docs origin with the reader, so its
// postMessage payloads are untrusted even after the usual origin checks.
// Normalize only the small set of values the reader consumes; never stringify
// or recursively walk the sender's object.

const MAX_READY_TEXT = 16_000_000;
const MAX_SELECTION_TEXT = 1_000_000;
const MAX_CONTEXT_TEXT = 4096;
const MAX_ID = 256;
const MAX_COORDINATE = 10_000_000;

const finite = (value, min = -MAX_COORDINATE, max = MAX_COORDINATE) =>
  typeof value === "number" && Number.isFinite(value) && value >= min && value <= max;
const boundedString = (value, max) => typeof value === "string" && value.length <= max;
const plainObject = (value) => value !== null && typeof value === "object" && !Array.isArray(value);

function pdfPoint(value) {
  if (!plainObject(value) || !Number.isSafeInteger(value.page) || value.page < 0 || value.page > 1_000_000
      || !finite(value.x, 0, MAX_COORDINATE) || !finite(value.y, 0, MAX_COORDINATE)) return null;
  return { page: value.page, x: value.x, y: value.y };
}

function rectangle(value) {
  if (!plainObject(value) || !finite(value.top) || !finite(value.left)
      || !finite(value.right) || !finite(value.bottom)) return null;
  if (value.right < value.left || value.bottom < value.top) return null;
  return { top: value.top, left: value.left, right: value.right, bottom: value.bottom };
}

function selector(value) {
  if (value === null) return null;
  if (!plainObject(value) || !boundedString(value.exact, MAX_SELECTION_TEXT)
      || !value.exact.length || !boundedString(value.prefix, MAX_CONTEXT_TEXT)
      || !boundedString(value.suffix, MAX_CONTEXT_TEXT)) return undefined;
  const result = { exact: value.exact, prefix: value.prefix, suffix: value.suffix };
  if (value.position !== undefined) {
    if (!Number.isSafeInteger(value.position) || value.position < 0 || value.position > MAX_READY_TEXT) return undefined;
    result.position = value.position;
  }
  return result;
}

/** Return a minimal, normalized message, or null when its schema is invalid. */
export function validateFrameMessage(input) {
  if (!plainObject(input) || input.librepaper !== true || !boundedString(input.type, 64)) return null;
  switch (input.type) {
    case "ready":
      return boundedString(input.text, MAX_READY_TEXT) ? { type: "ready", text: input.text } : null;
    case "selection": {
      const selected = selector(input.selector);
      if (selected === undefined) return null;
      let rect = null;
      if (input.rect !== undefined && input.rect !== null) {
        rect = rectangle(input.rect);
        if (!rect) return null;
      }
      return { type: "selection", selector: selected, ...(rect ? { rect } : {}) };
    }
    case "caret": {
      if (!Number.isSafeInteger(input.offset) || input.offset < 0 || input.offset > MAX_READY_TEXT) return null;
      let pdf = null;
      if (input.pdf !== undefined && input.pdf !== null) {
        pdf = pdfPoint(input.pdf);
        if (!pdf) return null;
      }
      return { type: "caret", offset: input.offset, pdf };
    }
    case "pdf-caret": {
      const pdf = pdfPoint(input.pdf);
      return pdf ? { type: "pdf-caret", pdf } : null;
    }
    case "viewer-state": {
      const namedMode = boundedString(input.mode, 32) && ["auto", "page-width", "page-fit", "page-actual"].includes(input.mode);
      const numericMode = boundedString(input.mode, 32) && /^(?:\d+(?:\.\d+)?|\.\d+)$/.test(input.mode)
        && Number(input.mode) >= 0.1 && Number(input.mode) <= 10;
      if (typeof input.drawn !== "boolean" || (!namedMode && !numericMode)
          || (input.scale !== null && !finite(input.scale, 0, 100))) return null;
      return { type: "viewer-state", drawn: input.drawn, mode: input.mode, scale: input.scale };
    }
    case "annotate":
    case "disarm":
      return { type: input.type };
    case "focus":
      return boundedString(input.id, MAX_ID) && input.id.length ? { type: "focus", id: input.id } : null;
    case "reading-position":
      return Number.isSafeInteger(input.start) && input.start >= 0 && input.start <= MAX_READY_TEXT
        ? { type: "reading-position", start: input.start } : null;
    case "suggestion-review":
      return boundedString(input.id, MAX_ID) && input.id.length && input.reason === "overlap"
        ? { type: "suggestion-review", id: input.id, reason: "overlap" } : null;
    default:
      return null;
  }
}

const RATE = {
  // Ready messages trigger a full text projection and re-anchoring. Charge in
  // million-character units so large documents cannot consume the burst
  // allowance as cheaply as short notes.
  ready: { capacity: 32, refillPerSecond: 2 },
  selection: { capacity: 48, refillPerSecond: 12 },
  "viewer-state": { capacity: 24, refillPerSecond: 4 },
  default: { capacity: 40, refillPerSecond: 8 },
};

/**
 * Authenticate and validate messages from the currently mounted document.
 * Budgets are per message class and reset whenever the frame URL changes.
 */
export function createFrameMessageReceiver({
  getFrame, getSrc, getDocsOrigin, onmessage,
  now = () => performance.now(),
  setTimer = (callback, delay) => setTimeout(callback, delay),
  clearTimer = (timer) => clearTimeout(timer),
}) {
  let budgetUrl;
  let buckets = new Map();
  let pending = new Map();
  let timer = null;
  let timerDue = null;
  let generation = 0;
  let disposed = false;

  function cancelPending() {
    generation++;
    if (timer !== null) clearTimer(timer);
    timer = null;
    timerDue = null;
    pending.clear();
  }

  function reset() {
    cancelPending();
    budgetUrl = undefined;
    buckets = new Map();
  }

  // A load may replace the document while preserving the iframe WindowProxy
  // and URL. Drop queued work on every load, but keep the budgets so a hostile
  // same-origin document cannot renew its allowance by navigating itself.
  function frameLoaded() {
    cancelPending();
  }

  function dispose() {
    reset();
    disposed = true;
  }

  function currentTarget() {
    const frame = getFrame?.();
    const src = getSrc?.();
    const docsOrigin = getDocsOrigin?.();
    if (!frame?.contentWindow || typeof src !== "string" || !src || typeof docsOrigin !== "string") return null;
    try {
      const target = new URL(src, globalThis.location?.href);
      const trusted = new URL(docsOrigin, globalThis.location?.href).origin;
      return target.origin === trusted ? { frame, window: frame.contentWindow, url: target.href, origin: target.origin } : null;
    } catch {
      return null;
    }
  }

  function consume(message, time) {
    const rule = RATE[message.type] || RATE.default;
    const previous = buckets.get(message.type) || { tokens: rule.capacity, time };
    const tokens = Math.min(rule.capacity, previous.tokens + Math.max(0, time - previous.time) * rule.refillPerSecond / 1000);
    const cost = message.type === "ready" ? Math.max(1, Math.ceil(message.text.length / 1_000_000)) : 1;
    if (tokens < cost) {
      buckets.set(message.type, { tokens, time });
      return false;
    }
    buckets.set(message.type, { tokens: tokens - cost, time });
    return true;
  }

  function scheduleFlush() {
    if (!pending.size) return;
    const time = now();
    const wait = Math.min(...[...pending].map(([type, entry]) => {
      const rule = RATE[type];
      const previous = buckets.get(type) || { tokens: rule.capacity, time };
      const available = Math.min(rule.capacity, previous.tokens + Math.max(0, time - previous.time) * rule.refillPerSecond / 1000);
      const cost = type === "ready" ? Math.max(1, Math.ceil(entry.message.text.length / 1_000_000)) : 1;
      return Math.max(1, (cost - available) * 1000 / rule.refillPerSecond);
    }));
    const due = time + wait;
    if (timer !== null && timerDue <= due) return;
    if (timer !== null) clearTimer(timer);
    generation++;
    const mine = generation;
    timerDue = due;
    timer = setTimer(() => {
      timer = null;
      timerDue = null;
      if (disposed || mine !== generation) return;
      flushPending();
    }, wait);
  }

  function queue(message, context) {
    // Keep only the newest document projection and selection. Once the rate
    // budget refills, the latest state is more useful than replaying a burst.
    pending.set(message.type, { message, context });
    scheduleFlush();
  }

  function flushPending() {
    if (disposed) return;
    const current = currentTarget();
    if (!current || current.url !== budgetUrl) {
      cancelPending();
      return;
    }
    const time = now();
    for (const [type, entry] of pending) {
      if (entry.context.url !== current.url || entry.context.window !== current.window) {
        pending.delete(type);
        continue;
      }
      if (consume(entry.message, time)) {
        pending.delete(type);
        onmessage?.(entry.message);
      }
    }
    scheduleFlush();
  }

  function receive(event) {
    if (disposed) return false;
    const current = currentTarget();
    if (!current || event.origin !== current.origin || event.source !== current.window) return false;

    // A changed URL is a new document identity. Discard accumulated state so
    // navigation cannot inherit or exhaust another document's allowance.
    if (budgetUrl !== current.url) {
      cancelPending();
      budgetUrl = current.url;
      buckets = new Map();
    }
    const message = validateFrameMessage(event.data);
    if (!message) return false;
    const time = now();
    if (!consume(message, time)) {
      if (message.type === "ready" || message.type === "selection") {
        queue(message, { url: current.url, window: current.window });
        return true;
      }
      return false;
    }
    // A newer in-budget state supersedes anything queued for that class.
    pending.delete(message.type);
    onmessage?.(message);
    return true;
  }

  return { receive, reset, frameLoaded, dispose };
}
