// Use the production room, collaboration controller and IndexedDB adapter.
// The WebSocket subclass only records lifecycle events; it does not alter them.
import { createReaderCollaboration } from "../../src/lib/reader/collaboration.js";
import * as collab from "../../src/lib/collab.js";

const closes = [];
const errors = [];
let opens = 0;
const NativeWebSocket = globalThis.WebSocket;
globalThis.WebSocket = class extends NativeWebSocket {
  constructor(...args) {
    super(...args);
    this.addEventListener("open", () => opens++);
    this.addEventListener("close", ({ code, reason }) => closes.push({ code, reason }));
  }
};
window.addEventListener("error", (event) => errors.push(event.message));
window.addEventListener("unhandledrejection", (event) => errors.push(String(event.reason)));

const slug = new URLSearchParams(location.search).get("slug");
let session;
let state = {};
let presenceFrames = 0;
const collaboration = createReaderCollaboration({
  slug,
  collab,
  getIdentity: () => "Recovery editor",
  getCanEdit: () => true,
  onState: (next) => { state = next; },
  onSession: (next) => { session = next; },
  onDocumentChanged: (reason) => errors.push(`document changed: ${reason}`),
  onMessage: (message) => {
    // Reader's document-frame dispatch, without rendering the rest of the UI.
    if (message.type === "doc-state") session?.start(message).catch((error) => errors.push(String(error)));
    else if (message.type === "doc-rows") session?.rows(message).catch((error) => errors.push(String(error)));
    else if (message.type === "doc-update") session?.apply(message.update);
    else if (message.type === "doc-gap") session?.gap(message.vector);
    else if (message.type === "doc-ack") session?.acknowledge(message.upTo || 0);
    else if (message.type === "doc-durable") session?.durable(message.vector);
    else if (message.type === "doc-presence") { presenceFrames++; session?.applyPresence(message.update); }
    else if (message.type === "error") errors.push(message.message);
  },
});
const text = () => session?.textOf(session.mainId())?.toString() || "";
let flood;
window.recovery = {
  snapshot: () => ({ ...state, text: text(), joined: Boolean(session?.joined), presenceFrames, closes, opens, errors }),
  async type(words) {
    const body = session.textOf(session.mainId());
    body.insert(body.length, words);
    session.doc.commit();
    await session.persist();
  },
  prepareFlood() {
    session.ephemeral.set("user:flood", { name: "f".repeat(64 * 1024), color: "#123456", tab: "flood" });
    const bytes = session.ephemeral.encode("user:flood");
    flood = { type: "doc-presence", update: btoa(Array.from(bytes, (byte) => String.fromCharCode(byte)).join("")) };
    return new TextEncoder().encode(JSON.stringify(flood)).length;
  },
  flood: () => collaboration.sendLive(flood).ok,
  close: () => collaboration.close(),
};
fetch(`/api/documents/${encodeURIComponent(slug)}`)
  .then(async (response) => {
    if (!response.ok) throw new Error(`document metadata: ${response.status}`);
    collaboration.start(await response.json());
  })
  .catch((error) => errors.push(String(error)));
