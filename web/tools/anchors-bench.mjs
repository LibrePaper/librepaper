// Timings for the anchor layer on one document of about 60,000 characters.
//
// Each run builds N anchors on distinct words, then times 200 keystrokes through
// applyOperation and 20 full resolutions, each call timed alone. With no
// argument it runs N = 1, 100 and 1000, one line each; with an argument it runs
// that N only.
//
// Usage: node web/tools/anchors-bench.mjs [N]
import { performance } from "node:perf_hooks";
import { LoroDoc, LoroText } from "loro-crdt";
import { CONTEXT_UNITS, MoveMemory, applyOperation, createAnchor, resolveAnchors } from "../src/lib/anchors.js";

const TARGET_CHARS = 60000;
const KEYSTROKES = 200;
const RESOLVES = 20;
const LETTERS = "abcdefghijklmnopqrstuvwxyz";

// A seeded generator, so every run builds the same document.
function random(seed) {
  let state = seed >>> 0;
  return () => {
    state = (Math.imul(state, 1664525) + 1013904223) >>> 0;
    return state / 0x100000000;
  };
}

// Random words of three to eight letters, a newline after every twelfth word.
function buildText(rnd) {
  let text = "";
  const words = [];
  while (text.length < TARGET_CHARS) {
    const length = 3 + Math.floor(rnd() * 6);
    let word = "";
    for (let i = 0; i < length; i++) word += LETTERS[Math.floor(rnd() * LETTERS.length)];
    words.push({ word, from: text.length, to: text.length + length });
    text += word;
    text += words.length % 12 === 0 ? "\n" : " ";
  }
  return { text, words };
}

function median(values) {
  const sorted = [...values].sort((a, b) => a - b);
  const mid = Math.floor(sorted.length / 2);
  return sorted.length % 2 === 1 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
}

function p95(values) {
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.ceil(0.95 * sorted.length) - 1)];
}

function run(anchorCount) {
  const rnd = random(7);
  const started = performance.now();
  const { text, words } = buildText(rnd);
  if (anchorCount > words.length) {
    throw new Error(`N=${anchorCount} is more than the ${words.length} words in the document`);
  }

  const doc = new LoroDoc();
  doc.setPeerId(1);
  doc.getMap("files").setContainer("f1", new LoroText()).insert(0, text);
  doc.commit();

  // Shuffle the word indices, then anchor the first N of them.
  const order = words.map((_, i) => i);
  for (let i = order.length - 1; i > 0; i--) {
    const j = Math.floor(rnd() * (i + 1));
    [order[i], order[j]] = [order[j], order[i]];
  }
  for (const index of order.slice(0, anchorCount)) {
    const { word, from, to } = words[index];
    createAnchor(doc, "f1", `anchor-${index}`, from, to, {
      exact: word,
      prefix: text.slice(Math.max(0, from - CONTEXT_UNITS), from),
      suffix: text.slice(to, to + CONTEXT_UNITS),
    });
  }
  doc.commit();

  const memory = new MoveMemory();
  const applyMs = [];
  let length = text.length;
  for (let k = 0; k < KEYSTROKES; k++) {
    const at = Math.floor(rnd() * (length + 1));
    const letter = LETTERS[Math.floor(rnd() * LETTERS.length)];
    const edits = [{ from: at, to: at, insert: letter }];
    const t0 = performance.now();
    applyOperation(doc, [{ fileId: "f1", edits }], memory, {});
    applyMs.push(performance.now() - t0);
    length += 1;
  }

  const resolveMs = [];
  for (let k = 0; k < RESOLVES; k++) {
    const t0 = performance.now();
    resolveAnchors(doc, "f1");
    resolveMs.push(performance.now() - t0);
  }

  return { applyMs, resolveMs, totalMs: performance.now() - started };
}

function report(anchorCount) {
  const { applyMs, resolveMs, totalMs } = run(anchorCount);
  console.log(
    `anchors=${anchorCount}` +
      ` applyOperation_ms_median=${median(applyMs).toFixed(3)}` +
      ` applyOperation_ms_p95=${p95(applyMs).toFixed(3)}` +
      ` resolveAnchors_ms_median=${median(resolveMs).toFixed(3)}` +
      ` total_ms=${totalMs.toFixed(1)}`,
  );
}

const argument = process.argv[2];
const sizes = argument === undefined ? [1, 100, 1000] : [Number(argument)];
if (sizes.some((n) => !Number.isInteger(n) || n < 0)) {
  console.error("usage: node web/tools/anchors-bench.mjs [N]");
  process.exit(1);
}
for (const n of sizes) report(n);
