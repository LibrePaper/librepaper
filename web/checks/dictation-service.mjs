// The dictation state machine, against a fake worker, microphone, and
// target (SPEC-dictation.md 8; docs/dictation-interfaces.md "service.js").
//
// `assemble.js` and `models.js` are written on other branches in parallel
// and do not exist in this tree (see the header comment in
// `src/lib/dictation/service.js`), so every scenario here injects
// `deps.assemble` and `deps.models` fakes rather than exercising the real
// modules -- `createDictationService` never imports them itself, only
// `getDictation()` does, lazily, and this check never calls `getDictation()`.

import { createDictationService } from "../src/lib/dictation/service.js";
import { createWebSpeechSession } from "../src/lib/dictation/webspeech.js";

let failures = 0;
function check(what, condition, detail = "") {
  if (condition) return;
  failures += 1;
  console.error(`dictation-service: FAIL ${what}${detail ? ` -- ${detail}` : ""}`);
}

/* --------------------------------------------------------- fakes: worker */

function makeFakeWorker() {
  const calls = [];
  let listener = null;
  const worker = {
    onerror: null,
    calls,
    terminated: false,
    set onmessage(fn) {
      listener = fn;
    },
    get onmessage() {
      return listener;
    },
    postMessage(msg) {
      calls.push(msg);
      if (msg.kind === "frame") return;
      const handler = worker.handlers[msg.kind];
      if (handler) handler(msg);
    },
    terminate() {
      worker.terminated = true;
    },
    emit(msg) {
      listener?.({ data: msg });
    },
    // Default protocol behavior; a scenario overrides one entry to script
    // progress, a delay, or a failure.
    handlers: {
      load(msg) {
        worker.emit({ id: msg.id, kind: "loaded", device: "wasm" });
      },
      start(msg) {
        worker.emit({ id: msg.id, kind: "started" });
      },
      stop(msg) {
        worker.emit({ id: msg.id, kind: "stopped" });
      },
    },
  };
  return worker;
}

/* --------------------------------------------------------- fakes: microphone */

function makeMicFactory() {
  const instances = [];
  let nextError = null;
  const factory = async ({ onFrame }) => {
    if (nextError) {
      const error = nextError;
      nextError = null;
      throw error;
    }
    const instance = { onFrame, closed: false, opened: true };
    instance.close = async () => {
      instance.closed = true;
    };
    instances.push(instance);
    return instance;
  };
  factory.instances = instances;
  factory.failNext = (error) => {
    nextError = error;
  };
  return factory;
}

/* --------------------------------------------------------- fakes: SpeechRecognition */

// A fake Web Speech API constructor, in the shape `webspeech.js` expects:
// `addEventListener`, `start()`, `stop()`, and the settable properties it
// configures. A scenario drives it by calling `instance.fire(name, payload)`,
// which runs every listener registered for that event name -- there is only
// ever one live instance per session since `createWebSpeechSession` makes
// exactly one `new SpeechRecognition()`.
function makeFakeSpeechRecognition() {
  const instances = [];
  function FakeSpeechRecognition() {
    const listeners = new Map();
    const instance = {
      continuous: false,
      interimResults: true,
      lang: "",
      maxAlternatives: 1,
      startCalls: 0,
      stopCalls: 0,
      addEventListener(name, fn) {
        if (!listeners.has(name)) listeners.set(name, new Set());
        listeners.get(name).add(fn);
      },
      removeEventListener(name, fn) {
        listeners.get(name)?.delete(fn);
      },
      start() {
        instance.startCalls += 1;
      },
      // A real `recognition.stop()` is asynchronous and eventually fires
      // `end`; the fake mirrors that on a microtask so `session.stop()`
      // resolves through the `end` event rather than its 1-second fallback.
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

// A `SpeechRecognition` result list, shaped like the Web Speech API's
// `SpeechRecognitionResultList`: array-like, each entry array-like of
// alternatives, only the final ones carrying `isFinal: true`.
function makeResultEvent(transcripts) {
  const results = transcripts.map(({ text, isFinal = true }) => {
    const alt = [{ transcript: text }];
    alt.isFinal = isFinal;
    return Object.assign(alt, { isFinal });
  });
  results.length = transcripts.length;
  return { results };
}

/* --------------------------------------------------------- fakes: storage */

function makeStorage() {
  const data = new Map();
  return {
    getItem: (k) => (data.has(k) ? data.get(k) : null),
    setItem: (k, v) => data.set(k, String(v)),
    removeItem: (k) => data.delete(k),
  };
}

/* --------------------------------------------------------- fakes: targets */

function makeTarget(initialText = "") {
  let text = initialText;
  let alive = true;
  const inserted = [];
  return {
    kind: "fake",
    insert(value) {
      inserted.push(value);
      text += value;
    },
    before(limit = 200) {
      return text.slice(Math.max(0, text.length - limit));
    },
    alive() {
      return alive;
    },
    focus() {},
    inserted,
    kill() {
      alive = false;
    },
  };
}

function makeVimTarget() {
  const target = makeTarget();
  const realInsert = target.insert.bind(target);
  target.normal = true;
  target.insert = (text) => {
    if (target.normal) {
      const error = new Error("normal mode");
      error.name = "VimNormalMode";
      throw error;
    }
    realInsert(text);
  };
  return target;
}

/* --------------------------------------------------------- fakes: models/assemble */

const MODEL_A = { id: "model-a", kind: "local", label: "Model A", capitalizes: true, punctuates: true };
const MODEL_BROWSER = { id: "browser", kind: "browser", label: "Browser built-in" };

function makeModels({ defaultId = MODEL_A.id, extra = [] } = {}) {
  const catalog = new Map([[MODEL_A.id, MODEL_A], [MODEL_BROWSER.id, MODEL_BROWSER], ...extra.map((m) => [m.id, m])]);
  const calls = { pickLanguage: [] };
  return {
    calls,
    modelById: (id) => catalog.get(id),
    defaultModel: () => defaultId,
    vad: () => ({ repo: "onnx-community/silero-vad", revision: "pinned" }),
    pickLanguage: (model, setting, navigatorLanguages) => {
      calls.pickLanguage.push({ model, setting, navigatorLanguages });
      if (setting && setting !== "auto") return setting;
      return navigatorLanguages[0] || null;
    },
  };
}

function fakeAssemble(before, raw, model) {
  const trimmed = raw.trim();
  if (!trimmed) return "";
  const space = before && !/[\s([{"'“‘]$/.test(before) ? " " : "";
  return space + trimmed;
}

/* --------------------------------------------------------- deps builder */

function makeDeps(overrides = {}) {
  const notifications = [];
  const persistCalls = { count: 0 };
  const worker = overrides.worker || makeFakeWorker();
  const mic = overrides.mic || makeMicFactory();
  const storage = overrides.storage !== undefined ? overrides.storage : makeStorage();
  const models = overrides.models || makeModels();
  // `SpeechRecognition` defaults to undefined -- the same as a browser
  // without the API -- so only scenarios that opt in exercise the browser
  // backend; every other scenario is unaffected by its existence.
  const SpeechRecognition = "SpeechRecognition" in overrides ? overrides.SpeechRecognition : undefined;
  const deps = {
    createWorker: overrides.createWorker || (() => worker),
    openMicrophone: overrides.openMicrophone || mic,
    storage,
    isSecureContext: overrides.isSecureContext || (() => true),
    hasWebAssembly: overrides.hasWebAssembly || (() => true),
    hasMediaDevices: overrides.hasMediaDevices || (() => true),
    confirmDownload: overrides.confirmDownload || (() => Promise.resolve(true)),
    SpeechRecognition,
    createWebSpeechSession:
      overrides.createWebSpeechSession || ((args) => createWebSpeechSession({ ...args, SpeechRecognition })),
    notify: (message, level) => notifications.push({ message, level }),
    persist: async () => {
      persistCalls.count += 1;
    },
    navigatorLanguages: overrides.navigatorLanguages || (() => ["en-US"]),
    now: () => 0,
    assemble: overrides.assemble || fakeAssemble,
    models,
  };
  return { deps, notifications, persistCalls, worker, mic, storage, models };
}

async function flush() {
  // Let queued microtasks (the async handlers chained off worker.emit(),
  // themselves sometimes awaiting another postMessage/emit round trip)
  // settle before the next assertion.
  for (let i = 0; i < 8; i += 1) await Promise.resolve();
}

/* --------------------------------------------------------- scenarios */

async function testPreconditions() {
  {
    const { deps, notifications } = makeDeps({ isSecureContext: () => false });
    const service = createDictationService(deps);
    await service.start(makeTarget());
    check("insecure context -> unavailable", service.state === "unavailable", service.state);
    check("insecure context -> reason set", typeof service.reason === "string" && service.reason.length > 0);
    check("insecure context -> toast", notifications.length === 1 && notifications[0].level === "error");
  }
  {
    const { deps, notifications } = makeDeps({ hasWebAssembly: () => false });
    const service = createDictationService(deps);
    await service.start(makeTarget());
    check("no WebAssembly -> unavailable", service.state === "unavailable", service.state);
    check("no WebAssembly -> toast", notifications.length === 1);
  }
  {
    const { deps, notifications } = makeDeps({ hasMediaDevices: () => false });
    const service = createDictationService(deps);
    await service.start(makeTarget());
    check("no media devices -> unavailable", service.state === "unavailable", service.state);
    check("no media devices -> reason mentions microphone", /microphone/i.test(service.reason || ""), service.reason);
    check("no media devices -> toast", notifications.length === 1);
  }
}

async function testBrowserBackendMissingSpeechRecognition() {
  // No `SpeechRecognition` constructor at all (the default in `makeDeps`) is
  // the "this browser has no built-in dictation" precondition failure, not a
  // crash -- SPEC 6's table of failure modes.
  const models = makeModels({ defaultId: MODEL_BROWSER.id });
  const { deps, notifications } = makeDeps({ models });
  const service = createDictationService(deps);
  await service.start(makeTarget());
  check("browser-kind model without SpeechRecognition -> unavailable", service.state === "unavailable", service.state);
  check("browser-kind model without SpeechRecognition -> reason", /no built-in dictation/i.test(service.reason || ""), service.reason);
  check("browser-kind model without SpeechRecognition -> toast", notifications.length === 1);
}

async function testBrowserBackendSetting() {
  // The stored backend setting, not an explicit model request, picks the
  // browser catalog entry (SPEC 4.10, service.js contract). No download
  // confirmation and no worker are involved.
  const storage = makeStorage();
  storage.setItem("librepaper-dictation-backend", "browser");
  let confirmCalls = 0;
  let workerCalls = 0;
  const { deps } = makeDeps({
    storage,
    confirmDownload: () => {
      confirmCalls += 1;
      return Promise.resolve(true);
    },
    createWorker: () => {
      workerCalls += 1;
      return makeFakeWorker();
    },
    SpeechRecognition: makeFakeSpeechRecognition(),
  });
  const service = createDictationService(deps);
  await service.start(makeTarget());

  check("backend=browser setting reaches listening", service.state === "listening", service.state);
  check("backend=browser setting picks the browser model", service.model?.id === "browser", JSON.stringify(service.model));
  check("backend=browser setting reports the browser device", service.device === "browser", service.device);
  check("backend=browser setting skips the download confirmation", confirmCalls === 0, `confirmCalls=${confirmCalls}`);
  check("backend=browser setting never creates a worker", workerCalls === 0, `workerCalls=${workerCalls}`);

  await service.stop();
}

async function testBrowserBackendFinalResultInserted() {
  const FakeSpeechRecognition = makeFakeSpeechRecognition();
  const { deps } = makeDeps({ models: makeModels({ defaultId: MODEL_BROWSER.id }), SpeechRecognition: FakeSpeechRecognition });
  const service = createDictationService(deps);
  const target = makeTarget("Hello");

  await service.start(target);
  check("browser session listening", service.state === "listening", service.state);
  const instance = FakeSpeechRecognition.instances[0];
  check("recognition started", instance.startCalls === 1, instance.startCalls);
  check("recognition configured continuous", instance.continuous === true);
  check("recognition configured non-interim", instance.interimResults === false);

  instance.fire("speechstart");
  check("speaking flag set from speechstart", service.speaking === true);
  instance.fire("speechend");
  check("speaking flag cleared from speechend", service.speaking === false);

  instance.fire("result", makeResultEvent([{ text: "world" }]));
  await flush();
  check("final result assembled and inserted", target.inserted[0] === " world", JSON.stringify(target.inserted));

  await service.stop();
  check("stop() stops the recognition and returns to idle", instance.stopCalls === 1 && service.state === "idle", `${instance.stopCalls} ${service.state}`);
}

async function testBrowserBackendNotAllowedRemembered() {
  const FakeSpeechRecognition = makeFakeSpeechRecognition();
  const { deps, notifications } = makeDeps({ models: makeModels({ defaultId: MODEL_BROWSER.id }), SpeechRecognition: FakeSpeechRecognition });
  const service = createDictationService(deps);

  await service.start(makeTarget());
  const instance = FakeSpeechRecognition.instances[0];
  instance.fire("error", { error: "not-allowed" });
  await flush();

  check("not-allowed error returns to idle", service.state === "idle", service.state);
  check("not-allowed error toasts the permission message", notifications.some((n) => n.level === "error" && /denied/i.test(n.message)), JSON.stringify(notifications));

  await service.start(makeTarget());
  check("denial is remembered for the session", service.state === "idle" && FakeSpeechRecognition.instances.length === 1, `instances=${FakeSpeechRecognition.instances.length} state=${service.state}`);
}

async function testBrowserBackendRestartsOnEnd() {
  const FakeSpeechRecognition = makeFakeSpeechRecognition();
  const { deps } = makeDeps({ models: makeModels({ defaultId: MODEL_BROWSER.id }), SpeechRecognition: FakeSpeechRecognition });
  const service = createDictationService(deps);

  await service.start(makeTarget());
  const instance = FakeSpeechRecognition.instances[0];
  check("started once", instance.startCalls === 1, instance.startCalls);

  // Chrome ends a continuous recognition on its own well before stop() is
  // called; the session must restart rather than leave dictation listening
  // with a dead recognizer.
  instance.fire("end");
  await flush();

  check("recognition restarted after an unrequested end", instance.startCalls === 2, instance.startCalls);
  check("still listening after the restart", service.state === "listening", service.state);

  await service.stop();
  check("stop() after restart still ends cleanly", service.state === "idle", service.state);
}

async function testDownloadConfirmation() {
  {
    const { deps } = makeDeps({ confirmDownload: () => Promise.resolve(false) });
    const service = createDictationService(deps);
    await service.start(makeTarget());
    check("declined download stays idle", service.state === "idle", service.state);
  }
  {
    let calls = 0;
    const storage = makeStorage();
    const { deps } = makeDeps({
      storage,
      confirmDownload: () => {
        calls += 1;
        return Promise.resolve(true);
      },
    });
    const service = createDictationService(deps);
    await service.start(makeTarget());
    check("accepted download reaches listening", service.state === "listening", service.state);
    await service.stop();
    await service.start(makeTarget());
    check("confirmation is remembered", calls === 1, `confirmDownload called ${calls} times`);
    await service.stop();
  }
}

async function testLoadingThenListening() {
  const events = [];
  const worker = makeFakeWorker();
  worker.handlers.load = (msg) => {
    worker.emit({ id: msg.id, kind: "progress", file: "model.onnx", loaded: 1, total: 4 });
    worker.emit({ id: msg.id, kind: "progress", file: "model.onnx", loaded: 4, total: 4 });
    worker.emit({ id: msg.id, kind: "loaded", device: "webgpu" });
  };
  const mic = makeMicFactory();
  const realOpen = mic;
  const openMicrophone = async (args) => {
    events.push("mic-open");
    return realOpen(args);
  };
  const { deps } = makeDeps({ worker, openMicrophone, mic });
  const service = createDictationService(deps);
  const seenStates = [];
  service.subscribe((snap) => seenStates.push(snap.state));

  const original = worker.handlers.load;
  worker.handlers.load = (msg) => {
    events.push("loaded-sent");
    original(msg);
  };

  await service.start(makeTarget());

  check("state passed through loading", seenStates.includes("loading"), seenStates.join(","));
  check("state reached listening", service.state === "listening", service.state);
  check("device reported from loaded reply", service.device === "webgpu", service.device);
  check(
    "progress mirrors the last progress message",
    service.progress && service.progress.loaded === 4 && service.progress.total === 4,
    JSON.stringify(service.progress),
  );
  check("microphone opened only after loaded was sent", events.indexOf("mic-open") > events.indexOf("loaded-sent"));
  await service.stop();
}

async function testTextInsertionAndTranscribing() {
  const worker = makeFakeWorker();
  const { deps } = makeDeps({ worker });
  const service = createDictationService(deps);
  const target = makeTarget("Hello");
  const states = [];
  service.subscribe((s) => states.push(s.state));

  await service.start(target);
  check("listening after start", service.state === "listening", service.state);

  worker.emit({ id: null, kind: "speech", speaking: true });
  check("speaking flag set", service.speaking === true);
  worker.emit({ id: null, kind: "speech", speaking: false });
  check("falling edge -> transcribing", service.state === "transcribing", service.state);

  worker.emit({ id: null, kind: "text", text: " world", reason: "pause", language: "en" });
  await flush();
  check("back to listening after text", service.state === "listening", service.state);
  check("first segment inserted", target.inserted[0] === " world", JSON.stringify(target.inserted));

  worker.emit({ id: null, kind: "speech", speaking: true });
  worker.emit({ id: null, kind: "speech", speaking: false });
  worker.emit({ id: null, kind: "text", text: " again", reason: "pause", language: "en" });
  await flush();
  check("second segment inserted after the first", target.inserted[1] === " again", JSON.stringify(target.inserted));
  check("segments inserted in order", target.inserted.length === 2);

  await service.stop();
}

async function testStopClosesMicBeforeSendingStopAndFlushesFinalText() {
  const worker = makeFakeWorker();
  const order = [];
  worker.handlers.stop = (msg) => {
    order.push("worker-received-stop");
    worker.emit({ id: null, kind: "text", text: "flushed", reason: "flush", language: "en" });
    worker.emit({ id: msg.id, kind: "stopped" });
  };
  const mic = makeMicFactory();
  const { deps } = makeDeps({ worker, mic });
  const service = createDictationService(deps);
  const target = makeTarget();
  await service.start(target);
  const instance = mic.instances[0];
  const realClose = instance.close;
  instance.close = async () => {
    order.push("mic-closed");
    await realClose();
  };

  await service.stop();

  check("mic closed before stop message reached worker", order.indexOf("mic-closed") < order.indexOf("worker-received-stop"), order.join(","));
  check("flushed text inserted", target.inserted.includes("flushed"), JSON.stringify(target.inserted));
  check("state idle after stop", service.state === "idle", service.state);
  check("microphone instance reports closed", instance.closed === true);
}

async function testTargetDiesMidSegment() {
  const worker = makeFakeWorker();
  const { deps, notifications } = makeDeps({ worker });
  const service = createDictationService(deps);
  const target = makeTarget();
  await service.start(target);
  target.kill();

  worker.emit({ id: null, kind: "text", text: "lost", reason: "pause", language: "en" });
  await flush();

  check("dead target stops dictation", service.state === "idle", service.state);
  check("dead target drops the text", target.inserted.length === 0, JSON.stringify(target.inserted));
  check("dead target toasts", notifications.some((n) => n.level === "error"));
}

async function testVimNormalMode() {
  const worker = makeFakeWorker();
  const { deps, notifications } = makeDeps({ worker });
  const service = createDictationService(deps);
  const target = makeVimTarget();
  await service.start(target);

  worker.emit({ id: null, kind: "text", text: "hola", reason: "pause", language: "en" });
  await flush();

  check("vim normal mode stops dictation", service.state === "idle", service.state);
  check("vim normal mode drops the text", target.inserted.length === 0, JSON.stringify(target.inserted));
  check(
    "vim normal mode toasts about insert mode",
    notifications.some((n) => n.level === "error" && /insert mode/i.test(n.message)),
    JSON.stringify(notifications),
  );
}

async function testWorkerError() {
  const worker = makeFakeWorker();
  const { deps, notifications } = makeDeps({ worker });
  const service = createDictationService(deps);
  await service.start(makeTarget());
  check("listening before crash", service.state === "listening", service.state);

  worker.onerror(new Error("boom"));
  await flush();

  check("worker error terminates the worker", worker.terminated === true);
  check("worker error returns to idle", service.state === "idle", service.state);
  check("worker error toasts", notifications.some((n) => n.level === "error"));
}

async function testUnsolicitedErrorMessage() {
  const worker = makeFakeWorker();
  const { deps, notifications } = makeDeps({ worker });
  const service = createDictationService(deps);
  await service.start(makeTarget());

  worker.emit({ id: null, kind: "error", message: "pipeline died" });
  await flush();

  check("unsolicited error terminates the worker", worker.terminated === true);
  check("unsolicited error returns to idle", service.state === "idle", service.state);
  check("unsolicited error toasts", notifications.some((n) => n.level === "error"));
}

async function testStartDuringListeningStopsPreviousTarget() {
  const worker = makeFakeWorker();
  const loadCalls = [];
  const originalLoad = worker.handlers.load;
  worker.handlers.load = (msg) => {
    loadCalls.push(msg);
    originalLoad(msg);
  };
  const mic = makeMicFactory();
  const { deps } = makeDeps({ worker, mic });
  const service = createDictationService(deps);
  const targetA = makeTarget();
  const targetB = makeTarget();

  await service.start(targetA);
  check("first target listening", service.state === "listening", service.state);
  const micA = mic.instances[0];

  await service.start(targetB);
  check("second start reaches listening", service.state === "listening", service.state);
  check("previous microphone closed", micA.closed === true);
  check("worker reused, not reloaded a second time", loadCalls.length === 1, `load called ${loadCalls.length} times`);

  worker.emit({ id: null, kind: "text", text: "for B", reason: "pause", language: "en" });
  await flush();
  check("new text goes to the new target", targetB.inserted.includes("for B"));
  check("old target receives nothing", targetA.inserted.length === 0);

  await service.stop();
}

async function testToggle() {
  const { deps } = makeDeps();
  const service = createDictationService(deps);
  const target = makeTarget();
  await service.toggle(target);
  check("toggle from idle starts", service.state === "listening", service.state);
  await service.toggle(target);
  check("toggle while listening stops", service.state === "idle", service.state);
}

async function testPermissionDenial() {
  const mic = makeMicFactory();
  const deniedError = Object.assign(new Error("denied"), { name: "NotAllowedError" });
  mic.failNext(deniedError);
  const { deps, notifications } = makeDeps({ mic });
  const service = createDictationService(deps);

  await service.start(makeTarget());
  check("permission denial returns to idle", service.state === "idle", service.state);
  check("permission denial toasts", notifications.some((n) => n.level === "error" && /denied/i.test(n.message)), JSON.stringify(notifications));

  const before = mic.instances.length;
  await service.start(makeTarget());
  check("second start does not reprompt for the microphone", mic.instances.length === before, `instances went from ${before} to ${mic.instances.length}`);
  check("second start still toasts and stays idle", service.state === "idle", service.state);
}

async function testLanguageSelection() {
  const models = makeModels();
  const { deps } = makeDeps({ models, navigatorLanguages: () => ["fr-CA", "en-US"] });
  const worker = deps.createWorker();
  const startCalls = [];
  const originalStart = worker.handlers.start;
  worker.handlers.start = (msg) => {
    startCalls.push(msg);
    originalStart(msg);
  };
  const service = createDictationService(deps);
  await service.start(makeTarget());

  check("pickLanguage consulted", models.calls.pickLanguage.length === 1);
  check(
    "pickLanguage saw the navigator languages",
    models.calls.pickLanguage[0].navigatorLanguages[0] === "fr-CA",
  );
  check("worker start request carries the picked language", startCalls[0]?.language === "fr-CA", JSON.stringify(startCalls));
  await service.stop();
}

async function testSubscribeSnapshotSequence() {
  const worker = makeFakeWorker();
  const { deps } = makeDeps({ worker });
  const service = createDictationService(deps);
  const snapshots = [];
  const unsubscribe = service.subscribe((snap) => snapshots.push({ ...snap }));

  check("subscribe calls immediately", snapshots.length === 1);
  check("initial snapshot is idle", snapshots[0].state === "idle");

  await service.start(makeTarget());
  check("snapshot sequence includes loading then listening", snapshots.some((s) => s.state === "loading") && snapshots.at(-1).state === "listening");

  unsubscribe();
  const countAfterUnsubscribe = snapshots.length;
  await service.stop();
  check("no more snapshots after unsubscribe", snapshots.length === countAfterUnsubscribe, `${snapshots.length} vs ${countAfterUnsubscribe}`);
}

async function testTestingReset() {
  const worker = makeFakeWorker();
  const { deps } = makeDeps({ worker });
  const service = createDictationService(deps);
  await service.start(makeTarget());
  check("listening before reset", service.state === "listening", service.state);

  service._testing.reset();

  check("reset returns to idle", service.state === "idle", service.state);
  check("reset terminates the worker", worker.terminated === true);
}

/* --------------------------------------------------------- run */

const tests = [
  testPreconditions,
  testBrowserBackendMissingSpeechRecognition,
  testBrowserBackendSetting,
  testBrowserBackendFinalResultInserted,
  testBrowserBackendNotAllowedRemembered,
  testBrowserBackendRestartsOnEnd,
  testDownloadConfirmation,
  testLoadingThenListening,
  testTextInsertionAndTranscribing,
  testStopClosesMicBeforeSendingStopAndFlushesFinalText,
  testTargetDiesMidSegment,
  testVimNormalMode,
  testWorkerError,
  testUnsolicitedErrorMessage,
  testStartDuringListeningStopsPreviousTarget,
  testToggle,
  testPermissionDenial,
  testLanguageSelection,
  testSubscribeSnapshotSequence,
  testTestingReset,
];

for (const test of tests) {
  await test();
}

if (failures) {
  console.error(`dictation-service: ${failures} check(s) failed`);
  process.exit(1);
}
console.log(`dictation-service: ${tests.length} scenario(s) passed`);
