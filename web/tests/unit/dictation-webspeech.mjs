// `createWebSpeechSession` in isolation, against a fake `SpeechRecognition`
// constructor (see the header comment in web/src/lib/dictation/webspeech.js).
// This check
// exercises the mapping rules -- construction, the restart-on-end behavior,
// and the error/event mapping -- directly, the way
// `web/tests/dictation-service.mjs` exercises the same module wired into
// the state machine.

import { createWebSpeechSession } from "../../src/lib/dictation/webspeech.js";

let failures = 0;
function check(what, condition, detail = "") {
  if (condition) return;
  failures += 1;
  console.error(`dictation-webspeech: FAIL ${what}${detail ? ` -- ${detail}` : ""}`);
}

/* --------------------------------------------------------- fake: SpeechRecognition */

function makeFakeSpeechRecognition() {
  const instances = [];
  function FakeSpeechRecognition() {
    const listeners = new Map();
    const instance = {
      continuous: false,
      interimResults: true,
      lang: "unset",
      maxAlternatives: 1,
      startCalls: 0,
      stopCalls: 0,
      addEventListener(name, fn) {
        if (!listeners.has(name)) listeners.set(name, new Set());
        listeners.get(name).add(fn);
      },
      start() {
        instance.startCalls += 1;
      },
      stop() {
        instance.stopCalls += 1;
        Promise.resolve().then(() => instance.fire("end"));
      },
      fire(name, payload) {
        for (const fn of listeners.get(name) || []) fn(payload);
      },
    };
    instances.push(instance);
    return instance;
  }
  FakeSpeechRecognition.instances = instances;
  return FakeSpeechRecognition;
}

function makeResultEvent(transcripts) {
  const results = transcripts.map(({ text, isFinal = true }) => {
    const alt = [{ transcript: text }];
    return Object.assign(alt, { isFinal });
  });
  results.length = transcripts.length;
  return { results };
}

async function flush() {
  for (let i = 0; i < 4; i += 1) await Promise.resolve();
}

/* --------------------------------------------------------- scenarios */

function testConfiguration() {
  const SpeechRecognition = makeFakeSpeechRecognition();
  createWebSpeechSession({ SpeechRecognition, language: "fr-CA", onText() {}, onSpeech() {}, onError() {} });
  const instance = SpeechRecognition.instances[0];
  check("continuous is true", instance.continuous === true);
  check("interimResults is false", instance.interimResults === false);
  check("maxAlternatives is 1", instance.maxAlternatives === 1);
  check("lang is set from the language argument", instance.lang === "fr-CA", instance.lang);
  check("recognition is started immediately", instance.startCalls === 1, instance.startCalls);
}

function testEmptyLanguageLetsTheBrowserPick() {
  const SpeechRecognition = makeFakeSpeechRecognition();
  createWebSpeechSession({ SpeechRecognition, language: null, onText() {}, onSpeech() {}, onError() {} });
  const instance = SpeechRecognition.instances[0];
  check("no language -> lang is the empty string", instance.lang === "", JSON.stringify(instance.lang));
}

async function testFinalResultsCallOnText() {
  const SpeechRecognition = makeFakeSpeechRecognition();
  const texts = [];
  createWebSpeechSession({ SpeechRecognition, language: "en", onText: (t) => texts.push(t), onSpeech() {}, onError() {} });
  const instance = SpeechRecognition.instances[0];

  instance.fire("result", makeResultEvent([{ text: "hello" }]));
  check("first final result reaches onText", texts.length === 1 && texts[0] === "hello", JSON.stringify(texts));

  // Chrome accumulates results across events for a continuous session;
  // only the newly-final entries should be reported again.
  instance.fire("result", makeResultEvent([{ text: "hello" }, { text: "world" }]));
  check("only the new final result is reported", texts.length === 2 && texts[1] === "world", JSON.stringify(texts));

  instance.fire("result", makeResultEvent([{ text: "hello" }, { text: "world" }, { text: "interim", isFinal: false }]));
  check("a non-final result is not reported", texts.length === 2, JSON.stringify(texts));
}

function testSpeechEvents() {
  const SpeechRecognition = makeFakeSpeechRecognition();
  const speechEvents = [];
  createWebSpeechSession({ SpeechRecognition, language: "en", onText() {}, onSpeech: (s) => speechEvents.push(s), onError() {} });
  const instance = SpeechRecognition.instances[0];

  instance.fire("speechstart");
  instance.fire("speechend");
  check("speechstart -> onSpeech(true)", speechEvents[0] === true, JSON.stringify(speechEvents));
  check("speechend -> onSpeech(false)", speechEvents[1] === false, JSON.stringify(speechEvents));
}

function testIgnoredErrors() {
  const SpeechRecognition = makeFakeSpeechRecognition();
  const errors = [];
  createWebSpeechSession({ SpeechRecognition, language: "en", onText() {}, onSpeech() {}, onError: (e) => errors.push(e) });
  const instance = SpeechRecognition.instances[0];

  instance.fire("error", { error: "no-speech" });
  instance.fire("error", { error: "aborted" });
  check("no-speech and aborted are ignored", errors.length === 0, JSON.stringify(errors));
}

function testNotAllowedErrors() {
  for (const code of ["not-allowed", "service-not-allowed"]) {
    const SpeechRecognition = makeFakeSpeechRecognition();
    const errors = [];
    createWebSpeechSession({ SpeechRecognition, language: "en", onText() {}, onSpeech() {}, onError: (e) => errors.push(e) });
    const instance = SpeechRecognition.instances[0];

    instance.fire("error", { error: code });
    check(`${code} -> onError with NotAllowedError`, errors.length === 1 && errors[0].name === "NotAllowedError", JSON.stringify(errors));
  }
}

function testOtherErrors() {
  const SpeechRecognition = makeFakeSpeechRecognition();
  const errors = [];
  createWebSpeechSession({ SpeechRecognition, language: "en", onText() {}, onSpeech() {}, onError: (e) => errors.push(e) });
  const instance = SpeechRecognition.instances[0];

  instance.fire("error", { error: "network", message: "network trouble" });
  check("other errors reach onError", errors.length === 1 && errors[0] instanceof Error, JSON.stringify(errors));
  check("other errors are not named NotAllowedError", errors[0]?.name !== "NotAllowedError", errors[0]?.name);
  check("other errors carry the message", errors[0]?.message === "network trouble", errors[0]?.message);
}

async function testRestartsOnUnrequestedEnd() {
  const SpeechRecognition = makeFakeSpeechRecognition();
  createWebSpeechSession({ SpeechRecognition, language: "en", onText() {}, onSpeech() {}, onError() {} });
  const instance = SpeechRecognition.instances[0];
  check("started once", instance.startCalls === 1);

  instance.fire("end");
  check("restarts after an end that stop() did not request", instance.startCalls === 2, instance.startCalls);
}

async function testRestartSwallowsRefusal() {
  const SpeechRecognition = makeFakeSpeechRecognition();
  createWebSpeechSession({ SpeechRecognition, language: "en", onText() {}, onSpeech() {}, onError() {} });
  const instance = SpeechRecognition.instances[0];
  instance.start = () => {
    throw new Error("the browser refused the restart");
  };

  // Must not throw out of the "end" listener.
  instance.fire("end");
  check("a refused restart is swallowed, not thrown", true);
}

async function testStopEndsWithoutRestartingAndResolvesOnEnd() {
  const SpeechRecognition = makeFakeSpeechRecognition();
  const session = createWebSpeechSession({ SpeechRecognition, language: "en", onText() {}, onSpeech() {}, onError() {} });
  const instance = SpeechRecognition.instances[0];

  await session.stop();

  check("stop() calls recognition.stop()", instance.stopCalls === 1, instance.stopCalls);
  check("stop() does not restart recognition", instance.startCalls === 1, instance.startCalls);

  // An `end` arriving after stop() (the fake fires it on its own stop(),
  // which already happened) must still not restart.
  instance.fire("end");
  check("an end after stop() still does not restart", instance.startCalls === 1, instance.startCalls);
}

async function testStopResolvesAfterOneSecondEvenWithoutAnEnd() {
  const SpeechRecognition = makeFakeSpeechRecognition();
  const session = createWebSpeechSession({ SpeechRecognition, language: "en", onText() {}, onSpeech() {}, onError() {} });
  const instance = SpeechRecognition.instances[0];
  // Simulate a browser that never fires `end` after stop() is asked for.
  instance.stop = () => {
    instance.stopCalls += 1;
  };

  const start = Date.now();
  await session.stop();
  const elapsed = Date.now() - start;
  check("stop() resolves via its fallback, not forever", elapsed < 5000, elapsed);
}

/* --------------------------------------------------------- run */

const tests = [
  testConfiguration,
  testEmptyLanguageLetsTheBrowserPick,
  testFinalResultsCallOnText,
  testSpeechEvents,
  testIgnoredErrors,
  testNotAllowedErrors,
  testOtherErrors,
  testRestartsOnUnrequestedEnd,
  testRestartSwallowsRefusal,
  testStopEndsWithoutRestartingAndResolvesOnEnd,
  testStopResolvesAfterOneSecondEvenWithoutAnEnd,
];

for (const test of tests) {
  await test();
  await flush();
}

if (failures) {
  console.error(`dictation-webspeech: ${failures} check(s) failed`);
  process.exit(1);
}
console.log(`dictation-webspeech: ${tests.length} scenario(s) passed`);
