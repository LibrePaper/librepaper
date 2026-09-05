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
    found = {
      doc,
      text: doc.getText("source"),
      // The directory: one Y.Text per file under an id, the paths beside it,
      // and which id is the main file. A map of texts is a shape the two
      // implementations have to agree about on its own -- an update that
      // creates a nested type is not an update that edits one -- so the tests
      // drive it from here rather than trusting that a text inside a map
      // behaves like a text beside one.
      files: doc.getMap("files"),
      paths: doc.getMap("paths"),
      assets: doc.getMap("assets"),
      meta: doc.getMap("meta"),
      awareness: new Awareness(doc),
      sent: [],
    };
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

// The text a browser is bound to, which is the main file's -- exactly what
// `collab.js` resolves. A session the server has not migrated yet has its
// words in the retired `source` text and no directory at all, so that is the
// fallback, and it is the same fallback the editor makes during a deploy.
//
// Every op below that says "the text" means this one. The tests that were
// written when a document was one text therefore go on saying what they said,
// and go on being true: they were never about which Yjs type held the words.
function main(found) {
  const id = found.meta.get("main");
  const text = id ? found.files.get(id) : null;
  return text instanceof Y.Text ? text : found.text;
}

const ops = {
  insert: ({ id, index, text }) => {
    main(peer(id)).insert(index, text);
    return {};
  },
  delete: ({ id, index, length }) => {
    main(peer(id)).delete(index, length);
    return {};
  },
  format: ({ id, index, length, attributes }) => {
    main(peer(id)).format(index, length, attributes);
    return {};
  },
  text: ({ id }) => ({ text: main(peer(id)).toString() }),
  length: ({ id }) => ({ length: main(peer(id)).length }),

  // The directory. `file` is a file's id throughout, never its path: the path
  // is a value in `paths`, which is what makes a rename leave the text alone.
  make_file: ({ id, file, path, body }) => {
    const found = peer(id);
    found.files.set(file, new Y.Text(body || ""));
    found.paths.set(file, path);
    return {};
  },
  file_insert: ({ id, file, index, text }) => {
    peer(id).files.get(file).insert(index, text);
    return {};
  },
  file_delete: ({ id, file, index, length }) => {
    peer(id).files.get(file).delete(index, length);
    return {};
  },
  file_text: ({ id, file }) => {
    const text = peer(id).files.get(file);
    return { text: text ? text.toString() : null };
  },
  file_length: ({ id, file }) => ({ length: peer(id).files.get(file).length }),
  rename: ({ id, file, path }) => {
    peer(id).paths.set(file, path);
    return {};
  },
  remove_file: ({ id, file }) => {
    const found = peer(id);
    found.files.delete(file);
    found.paths.delete(file);
    return {};
  },
  set_asset: ({ id, path, sha }) => {
    peer(id).assets.set(path, sha);
    return {};
  },
  set_main: ({ id, file }) => {
    peer(id).meta.set("main", file);
    return {};
  },
  // The whole directory as this peer sees it, which is what a test compares
  // against what the server sees.
  tree: ({ id }) => {
    const found = peer(id);
    const paths = {};
    for (const [file, path] of found.paths.entries()) paths[file] = path;
    const texts = {};
    for (const [file, text] of found.files.entries()) texts[file] = text.toString();
    const assets = {};
    for (const [path, sha] of found.assets.entries()) assets[path] = sha;
    return { paths, texts, assets, main: found.meta.get("main") ?? "" };
  },
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
