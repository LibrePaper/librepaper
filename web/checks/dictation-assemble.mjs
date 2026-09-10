// Table-driven check for web/src/lib/dictation/assemble.js.
//
// House style follows checks/latex-local.mjs: a plain script, a `check`
// helper that counts failures, no test framework, exit 1 iff something
// failed.

import { assemble, cleanSegment, needsSpace, startsSentence } from "../src/lib/dictation/assemble.js";

let failures = 0;
function check(what, condition, detail = "") {
  if (condition) return;
  failures += 1;
  console.error(`dictation-assemble: FAIL ${what}${detail ? ` -- ${detail}` : ""}`);
}

const PUNCTUATES_CAPITALIZES = { capitalizes: true, punctuates: true }; // e.g. whisper-base
const NEITHER = { capitalizes: false, punctuates: false }; // e.g. parakeet-ctc

/* --------------------------------------------------------- cleanSegment */

const cleanCases = [
  ["trims surrounding whitespace", "  hello world  ", "hello world"],
  ["drops a lone [BLANK_AUDIO] token", "[BLANK_AUDIO]", ""],
  ["drops a lone [Music] token, case-insensitively", "[Music]", ""],
  ["drops a lone (music) token", "(music)", ""],
  ["drops a lone (applause) token", "(applause)", ""],
  ["drops a lone music note", "♪", ""],
  ["drops repeated music notes", "♪ ♪", ""],
  ["drops an empty string", "", ""],
  ["drops a whitespace-only string", "   ", ""],
  ["strips a noise token mixed with words, keeping the words", "[BLANK_AUDIO] hello", "hello"],
  ["strips a trailing noise token, keeping the words", "hello (applause)", "hello"],
  ["strips an interior noise token without leaving a double space", "hello ♪ world", "hello world"],
];

for (const [what, raw, expected] of cleanCases) {
  check(`cleanSegment: ${what}`, cleanSegment(raw) === expected, `got ${JSON.stringify(cleanSegment(raw))}`);
}

/* --------------------------------------------------------- needsSpace */

const spaceCases = [
  ["false at the start of the document", "", false],
  ["false after a trailing space", "hello ", false],
  ["false after a newline", "hello\n", false],
  ["false after an opening paren", "see (", false],
  ["false after an opening bracket", "list [", false],
  ["false after an opening brace", "code {", false],
  ["false after a straight double quote", 'she said "', false],
  ["false after a straight single quote", "it's '", false],
  ["false after a curly opening quote", "she said “", false],
  ["true after a letter (mid-word caret is still a word boundary case)", "hello", true],
  ["true right after sentence punctuation with no space yet", "Done.", true],
  ["true after a digit", "chapter 1", true],
  ["true after a closing paren", "(see above)", true],
];

for (const [what, before, expected] of spaceCases) {
  check(`needsSpace: ${what}`, needsSpace(before) === expected, `got ${needsSpace(before)}`);
}

/* --------------------------------------------------------- startsSentence */

const sentenceCases = [
  ["true at the very start", "", true],
  ["true after a period", "Done.", true],
  ["true after a period and a space", "Done. ", true],
  ["true after an exclamation mark", "Wow!", true],
  ["true after a question mark", "Really?", true],
  ["true after a period inside closing quotes", 'He said "no."', true],
  ["true after a period then a closing paren", "(Done.)", true],
  ["true after a period, closing quote, and trailing space", 'She said "stop." ', true],
  ["true after a bare newline with no punctuation", "line one\n", true],
  ["false mid-sentence after a letter", "hello", false],
  ["false mid-sentence after a comma", "hello,", false],
  ["false after a letter followed by trailing space", "hello there ", false],
];

for (const [what, before, expected] of sentenceCases) {
  check(`startsSentence: ${what}`, startsSentence(before) === expected, `got ${startsSentence(before)}`);
}

/* --------------------------------------------------------- assemble */

const assembleCases = [
  ["drops a pure noise segment, returning empty", "hello ", "[BLANK_AUDIO]", NEITHER, ""],
  ["inserts a leading space mid-sentence", "hello", "world", NEITHER, " world"],
  ["adds no leading space at the start of the document", "", "hello", NEITHER, "Hello"],
  ["adds no leading space after a trailing space", "hello ", "world", NEITHER, "world"],
  ["adds no leading space after a newline", "hello\n", "world", NEITHER, "World"],
  ["adds no leading space after an opening bracket", "list [", "one", NEITHER, "one"],
  [
    "capitalizes the first letter at a sentence start when the model does not",
    "Done. ",
    "hello world",
    NEITHER,
    "Hello world",
  ],
  [
    "uppercases a French accented letter, Unicode aware",
    "",
    "écoute",
    NEITHER,
    "Écoute",
  ],
  [
    "does not capitalize mid-sentence even when the model does not",
    "hello",
    "world",
    NEITHER,
    " world",
  ],
  [
    "leaves a model that punctuates and capitalizes untouched, still spaced",
    "hello",
    "World.",
    PUNCTUATES_CAPITALIZES,
    " World.",
  ],
  [
    "leaves a model that punctuates and capitalizes untouched at a sentence start",
    "Done. ",
    "world",
    PUNCTUATES_CAPITALIZES,
    "world",
  ],
  [
    "invents no punctuation for a model that does not punctuate",
    "hello",
    "world",
    NEITHER,
    " world",
  ],
  ["strips noise mixed with real words, still spacing and capitalizing", "Done. ", "[Music] hello", NEITHER, "Hello"],
];

for (const [what, before, raw, model, expected] of assembleCases) {
  const got = assemble(before, raw, model);
  check(`assemble: ${what}`, got === expected, `got ${JSON.stringify(got)}, wanted ${JSON.stringify(expected)}`);
}

/* --------------------------------------------------------- run */

if (failures) {
  console.error(`dictation-assemble: ${failures} check(s) failed`);
  process.exit(1);
}
console.log(
  `dictation-assemble: ${cleanCases.length + spaceCases.length + sentenceCases.length + assembleCases.length} check(s) passed`,
);
