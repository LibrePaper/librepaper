// Turns one recognizer segment into the exact string to insert at the caret.
//
// This is pure and dependency-free
// so `web/tests/unit/dictation-assemble.mjs` can run it under Node, and so the
// service can call it synchronously on every worker `text` message without
// worrying about ambient state. It never looks far into `before`: only the
// last few characters can change the outcome (a caret position, not a
// paragraph, decides spacing and capitalization), so every function here
// works off a short tail rather than scanning the whole string.

// Recognizer noise: Whisper emits these for silence, music, and applause
// beds instead of refusing to produce text. None of them is something a
// person dictated, so they are stripped rather than inserted.
const NOISE = /\[(?:blank_audio|music|noise|silence|applause|laughter)\]|\((?:music|applause|laughter|noise)\)|♪/gi;

// Characters after which a new segment should butt up against the existing
// text with no inserted space: whitespace (any, including newline) and an
// opening bracket or quote, because a space there would land on the wrong
// side of the punctuation the user just typed.
const OPENERS = "([{\"'“‘«";

// Trailing punctuation, optionally still inside closing quotes/brackets and
// trailing whitespace, that marks the end of a sentence.
const SENTENCE_END = /[.!?][)\]"'”’»]*\s*$/;

export function cleanSegment(raw) {
  const trimmed = String(raw ?? "").trim();
  const stripped = trimmed.replace(NOISE, " ").replace(/\s+/g, " ").trim();
  return stripped;
}

export function needsSpace(before) {
  if (before === "") return false;
  const last = before.slice(-1);
  if (/\s/.test(last)) return false;
  if (OPENERS.includes(last)) return false;
  return true;
}

export function startsSentence(before) {
  if (before === "") return true;
  // Only the tail can matter; SENTENCE_END never needs more than a handful
  // of trailing characters to match.
  const tail = before.slice(-16);
  if (tail.endsWith("\n")) return true;
  return SENTENCE_END.test(tail);
}

function upperFirst(text) {
  // Unicode-aware: split off one code point (not one UTF-16 code unit) so
  // accented letters like "é" upper-case to "É" instead of being mangled.
  const [first, ...rest] = text;
  return first.toUpperCase() + rest.join("");
}

export function assemble(before, raw, model) {
  const cleaned = cleanSegment(raw);
  if (!cleaned) return "";

  let text = cleaned;
  if (!model.capitalizes && startsSentence(before)) {
    text = upperFirst(text);
  }
  // model.punctuates governs the recognizer's own output; this module never
  // invents punctuation of its own regardless of the flag.

  return (needsSpace(before) ? " " : "") + text;
}
