// A real browser Yjs peer, driven from outside.
//
// The Rust server holds the shared document with yrs, and a second
// implementation of a CRDT is only as good as its agreement with the first.
// So the interoperability tests do not model a browser: they run one. This
// script is the browser's half -- the same `yjs` and `y-protocols` packages
// the editor bundles -- speaking a line of JSON per operation over stdin and
// answering with a line of JSON on stdout.
//
// Indices are Yjs's own, which are UTF-16 code units, because that is what a
// browser counts in and what the interoperability of positions is about.

import * as Y from "yjs";
import { Awareness, encodeAwarenessUpdate, applyAwarenessUpdate } from "y-protocols/awareness.js";

const docs = new Map();

const b64 = (bytes) => Buffer.from(bytes).toString("base64");
const bin = (text) => new Uint8Array(Buffer.from(text, "base64"));

function peer(id) {
  let found = docs.get(id);
  if (!found) {
    const doc = new Y.Doc();
    found = { doc, text: doc.getText("source"), awareness: new Awareness(doc), sent: [] };
    // Everything this peer produces locally is kept, so a test can play the
    // part of a socket that dropped: the updates made while it was down are
    // exactly the ones the server never saw.
    doc.on("update", (update, origin) => {
      if (origin !== "remote") found.sent.push(b64(update));
    });
    docs.set(id, found);
  }
  return found;
}

const ops = {
  insert: ({ id, index, text }) => {
    peer(id).text.insert(index, text);
    return {};
  },
  delete: ({ id, index, length }) => {
    peer(id).text.delete(index, length);
    return {};
  },
  format: ({ id, index, length, attributes }) => {
    peer(id).text.format(index, length, attributes);
    return {};
  },
  text: ({ id }) => ({ text: peer(id).text.toString() }),
  length: ({ id }) => ({ length: peer(id).text.length }),
  // The state vector: what this peer already has, which is what it sends a
  // server to ask for the rest.
  vector: ({ id }) => ({ vector: b64(Y.encodeStateVector(peer(id).doc)) }),
  // Everything, or everything the given vector is missing.
  update: ({ id, vector }) => ({
    update: b64(Y.encodeStateAsUpdate(peer(id).doc, vector ? bin(vector) : undefined)),
  }),
  apply: ({ id, update }) => {
    Y.applyUpdate(peer(id).doc, bin(update), "remote");
    return {};
  },
  // What this peer has produced and, by pretending, not managed to send.
  outbox: ({ id }) => {
    const found = peer(id);
    const sent = found.sent;
    found.sent = [];
    return { updates: sent };
  },
  awareness: ({ id, name, color }) => {
    const found = peer(id);
    found.awareness.setLocalStateField("user", { name, color: color || "#2f5bd0" });
    return {
      update: b64(
        encodeAwarenessUpdate(found.awareness, [found.doc.clientID]),
      ),
    };
  },
  awareness_apply: ({ id, update }) => {
    const found = peer(id);
    applyAwarenessUpdate(found.awareness, bin(update), "remote");
    return {
      states: [...found.awareness.getStates().entries()].map(([client, state]) => ({
        client,
        name: state?.user?.name ?? "",
      })),
    };
  },
  drop: ({ id }) => {
    docs.get(id)?.doc.destroy();
    docs.delete(id);
    return {};
  },
};

let buffer = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (chunk) => {
  buffer += chunk;
  let cut;
  while ((cut = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, cut);
    buffer = buffer.slice(cut + 1);
    if (!line.trim()) continue;
    let answer;
    try {
      const request = JSON.parse(line);
      const run = ops[request.op];
      if (!run) throw new Error(`unknown op ${request.op}`);
      answer = { ok: true, ...run(request) };
    } catch (error) {
      answer = { ok: false, error: String(error && error.message ? error.message : error) };
    }
    process.stdout.write(JSON.stringify(answer) + "\n");
  }
});
