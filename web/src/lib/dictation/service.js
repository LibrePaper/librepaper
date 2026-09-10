// The dictation state machine (SPEC-dictation.md 4.1, 4.9; 6;
// docs/dictation-interfaces.md "service.js"). Every component that offers
// dictation -- the composer button, the comment/reply buttons, the
// shortcut, the status pill -- talks to the single object this module
// builds, never to the worker, the microphone, or a target directly.
//
// Everything ambient (creating the worker, opening the microphone, reading
// storage, showing a toast, the wall clock) arrives through `deps`, in the
// style of `web/src/lib/latex/local.js`, so `web/checks/dictation-service.mjs`
// can drive the real state machine under Node with fakes.
//
// `assemble.js` and `models.js` are being written in parallel on other
// branches (docs/dictation-interfaces.md's parallel-authorship note) and do
// not exist in this tree yet. `createDictationService` never imports them:
// it only calls `deps.assemble(...)` and the `deps.models` methods below,
// which the check supplies as small fakes. `getDictation()` -- the one
// caller that needs the real thing -- resolves them with a *dynamic*
// `import()`, and only inside `start()`, the first time a session actually
// needs them. A static `import ... from "./models.js"` at the top of this
// file would fail to resolve the moment Node loaded this module at all,
// long before any test got a chance to inject a fake; a lazy dynamic import
// is invisible to Node until the code path that needs the real file
// actually runs, which happens only from a real page, after every module in
// docs/dictation-interfaces.md has been merged.
//
// deps.models is the models.js module namespace shape, called as methods
// rather than read as `DEFAULT_MODEL`/`VAD` constants, so the lazily-loaded
// real module and a synchronous fake look identical to this file:
//   { modelById(id), defaultModel(), vad(), pickLanguage(model, setting, navigatorLanguages) }
// Each may return its value directly or a Promise of it; this module always
// `await`s them, which is a no-op on a plain value.

import { readSettings, confirm as confirmModel, isConfirmed } from "./settings.js";

const STATES = Object.freeze({
  IDLE: "idle",
  LOADING: "loading",
  LISTENING: "listening",
  TRANSCRIBING: "transcribing",
  UNAVAILABLE: "unavailable",
});

// SPEC-dictation.md section 6, "Preconditions and failure modes".
const REASON_INSECURE = "Dictation needs HTTPS or localhost";
const REASON_NO_WASM = "Dictation needs WebAssembly, which this browser does not support";
const REASON_NO_MIC = "No microphone found";
const REASON_BROWSER_BACKEND = "Browser dictation is not available yet";

const TOAST_PERMISSION_DENIED = "Microphone access was denied. Check this site's microphone permission in your browser settings.";
const TOAST_TARGET_GONE = "Dictation stopped: the field it was typing into went away.";
const TOAST_VIM_NORMAL_MODE = "Dictation stopped: enter insert mode to keep dictating into the editor.";
const TOAST_WORKER_ERROR = "Dictation stopped after an internal error.";
const TOAST_DOWNLOAD_FAILED = "Dictation could not download its model.";

let nextRequestId = 1;

export function createDictationService(deps) {
  let state = STATES.IDLE;
  let progress = null;
  let model = null;
  let device = null;
  let reason = null;
  let speaking = false;

  let worker = null;
  let workerModelId = null; // the model id `worker` currently has loaded, or null
  let pending = new Map(); // request id -> { resolve, reject }
  let microphone = null;
  let target = null;
  let permissionDeniedThisSession = false;
  let persisted = false;

  const listeners = new Set();

  function snapshot() {
    return { state, progress, model, device, reason, speaking };
  }

  function notifyListeners() {
    const value = snapshot();
    for (const fn of listeners) fn(value);
  }

  function setState(patch) {
    if ("state" in patch) state = patch.state;
    if ("progress" in patch) progress = patch.progress;
    if ("model" in patch) model = patch.model;
    if ("device" in patch) device = patch.device;
    if ("reason" in patch) reason = patch.reason;
    if ("speaking" in patch) speaking = patch.speaking;
    notifyListeners();
  }

  function toIdle() {
    setState({ state: STATES.IDLE, progress: null, model: null, device: null, reason: null, speaking: false });
  }

  function toUnavailable(text) {
    setState({ state: STATES.UNAVAILABLE, progress: null, model: null, device: null, reason: text, speaking: false });
  }

  // ---------------------------------------------------------- worker wiring

  function send(kind, payload = {}) {
    const id = nextRequestId++;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      try {
        worker.postMessage({ id, kind, ...payload });
      } catch (error) {
        pending.delete(id);
        reject(error);
      }
    });
  }

  function postFrame(audio) {
    if (!worker) return;
    worker.postMessage({ id: null, kind: "frame", audio }, [audio.buffer]);
  }

  // A flush's `text` message and the `stopped` reply to the `stop` request
  // that provoked it arrive in that order (worker.js protocol table), but
  // the flush is handled asynchronously (it awaits `deps.assemble`) while
  // `stopped` resolves `stop()`'s own await synchronously. `stop()` awaits
  // this after `stopped` so the flushed text is actually inserted -- SPEC
  // 6's "insert any last text, then idle" -- before the target is dropped.
  let lastTextHandling = Promise.resolve();

  async function closeMicrophone() {
    if (!microphone) return;
    const mic = microphone;
    microphone = null;
    await mic.close();
  }

  function clearTarget() {
    target = null;
  }

  /// The microphone is released and the target forgotten, in that order (the
  /// recording indicator must go out even if nothing else about the session
  /// can be salvaged) -- SPEC 3 and 6. The worker is told to stop too, the
  /// same as a graceful `stop()`, so it does not sit "started" waiting for
  /// frames that a closed microphone will never send again.
  async function abortSession(toastMessage) {
    await closeMicrophone();
    if (worker) {
      try {
        await send("stop", {});
      } catch {
        /* the worker is already gone */
      }
    }
    clearTarget();
    toIdle();
    if (toastMessage) deps.notify(toastMessage, "error");
  }

  async function handleTextMessage(msg) {
    const wasTranscribing = state === STATES.TRANSCRIBING;
    if (!target || !target.alive()) {
      await abortSession(TOAST_TARGET_GONE);
      return;
    }
    const before = target.before();
    const text = await deps.assemble(before, msg.text, model);
    if (text) {
      try {
        target.insert(text);
      } catch (error) {
        if (error?.name === "VimNormalMode") {
          await abortSession(TOAST_VIM_NORMAL_MODE);
          return;
        }
        throw error;
      }
    }
    if (wasTranscribing && state === STATES.TRANSCRIBING) {
      setState({ state: STATES.LISTENING });
    }
  }

  function resolvePending(id, msg) {
    const call = pending.get(id);
    if (!call) return false;
    pending.delete(id);
    call.resolve(msg);
    return true;
  }

  function rejectPending(id, error) {
    const call = pending.get(id);
    if (!call) return false;
    pending.delete(id);
    call.reject(error);
    return true;
  }

  function handleWorkerMessage(msg) {
    if (msg == null) return;
    switch (msg.kind) {
      case "progress":
        setState({ progress: { loaded: msg.loaded, total: msg.total, file: msg.file } });
        return;
      case "speech": {
        const was = speaking;
        setState({ speaking: !!msg.speaking });
        if (was && !msg.speaking && state === STATES.LISTENING) {
          setState({ state: STATES.TRANSCRIBING });
        }
        return;
      }
      case "text":
        // A `transcribe` request's own reply carries a matching id and its
        // `reason: "direct"`; a segment from the running session is
        // unsolicited (`id: null`) and goes through insertion instead.
        if (msg.id != null && resolvePending(msg.id, msg)) return;
        // Segments are transcribed and inserted in order (SPEC 4.3); the
        // worker only sends the next `text` once its own segment is ready,
        // so no reordering guard is needed on this side.
        lastTextHandling = handleTextMessage(msg).catch((error) => failSession(error));
        return;
      case "loaded":
      case "started":
      case "stopped":
      case "unloaded":
        resolvePending(msg.id, msg);
        return;
      case "error":
        if (msg.id != null && rejectPending(msg.id, workerError(msg.message))) return;
        // Unsolicited: the pipeline broke mid-session rather than in reply
        // to a specific request. Treated like a crash (SPEC 6 "Worker
        // crashes").
        failSession(workerError(msg.message));
        return;
      default:
        return;
    }
  }

  function workerError(message) {
    return Object.assign(new Error(message), { name: "WorkerError" });
  }

  function rejectAllPending(error) {
    for (const { reject } of pending.values()) reject(error);
    pending.clear();
  }

  function failSession(error) {
    const dead = worker;
    worker = null;
    workerModelId = null;
    rejectAllPending(error);
    closeMicrophone().then(() => {
      clearTarget();
      try {
        dead?.terminate();
      } catch {
        /* already gone */
      }
      toIdle();
      deps.notify(TOAST_WORKER_ERROR, "error");
    });
  }

  function attachWorker(w) {
    w.onmessage = (event) => handleWorkerMessage(event.data);
    w.onerror = () => failSession(new Error("worker crashed"));
    return w;
  }

  // -------------------------------------------------------------- lifecycle

  function preconditionFailure() {
    if (!deps.isSecureContext()) return REASON_INSECURE;
    if (!deps.hasWebAssembly()) return REASON_NO_WASM;
    if (!deps.hasMediaDevices()) return REASON_NO_MIC;
    return null;
  }

  async function resolveModel(requested) {
    const settings = readSettings(deps.storage);
    const id = requested || settings.model || (await deps.models.defaultModel());
    const entry = await deps.models.modelById(id);
    return entry || (await deps.models.modelById(await deps.models.defaultModel()));
  }

  async function resolveLanguage(requested, entry) {
    if (requested !== undefined) return requested;
    const settings = readSettings(deps.storage);
    return deps.models.pickLanguage(entry, settings.language, deps.navigatorLanguages());
  }

  async function ensureWorkerLoaded(entry) {
    if (worker && workerModelId === entry.id) return;
    if (worker) {
      try {
        worker.terminate();
      } catch {
        /* already gone */
      }
      rejectAllPending(new Error("worker replaced"));
    }
    worker = attachWorker(deps.createWorker());
    setState({ state: STATES.LOADING, progress: null, model: entry, device: null, reason: null });
    const vad = await deps.models.vad();
    const loaded = await send("load", { model: entry, vad, device: "auto" });
    workerModelId = entry.id;
    setState({ device: loaded.device });
    if (!persisted) {
      persisted = true;
      await deps.persist();
    }
  }

  async function start(newTarget, { model: requestedModel, language: requestedLanguage } = {}) {
    if (state !== STATES.IDLE && state !== STATES.UNAVAILABLE) {
      await stop();
    }

    const failure = preconditionFailure();
    if (failure) {
      toUnavailable(failure);
      deps.notify(failure, "error");
      return;
    }

    if (permissionDeniedThisSession) {
      deps.notify(TOAST_PERMISSION_DENIED, "error");
      return;
    }

    const entry = await resolveModel(requestedModel);
    if (entry.kind === "browser") {
      toUnavailable(REASON_BROWSER_BACKEND);
      deps.notify(REASON_BROWSER_BACKEND, "error");
      return;
    }

    if (!isConfirmed(deps.storage, entry.id)) {
      const confirmed = await deps.confirmDownload(entry);
      if (!confirmed) return; // stays idle, not remembered as declined
      confirmModel(deps.storage, entry.id);
    }

    const language = await resolveLanguage(requestedLanguage, entry);

    try {
      await ensureWorkerLoaded(entry);
    } catch (error) {
      deps.notify(TOAST_DOWNLOAD_FAILED, "error");
      try {
        worker?.terminate();
      } catch {
        /* already gone */
      }
      worker = null;
      workerModelId = null;
      toIdle();
      return;
    }

    await send("start", { language });

    try {
      microphone = await deps.openMicrophone({ onFrame: postFrame });
    } catch (error) {
      if (error?.name === "NotAllowedError") {
        permissionDeniedThisSession = true;
        deps.notify(TOAST_PERMISSION_DENIED, "error");
      } else {
        deps.notify(REASON_NO_MIC, "error");
      }
      try {
        await send("stop", {});
      } catch {
        /* worker already unreachable */
      }
      toIdle();
      return;
    }

    target = newTarget;
    setState({ state: STATES.LISTENING, model: entry });
  }

  async function stop() {
    if (state === STATES.IDLE || state === STATES.UNAVAILABLE) return;
    // The microphone is released as soon as dictation stops (SPEC 3), before
    // the worker has even acknowledged the stop -- the recording indicator
    // must go out right away, not after a round trip.
    await closeMicrophone();
    if (worker) {
      try {
        await send("stop", {});
      } catch {
        /* the worker is already gone; nothing left to flush */
      }
    }
    // The `stopped` reply follows its flushed segment's `text` message, but
    // that message is handled asynchronously -- wait for it so the final
    // text actually lands before the target is let go (SPEC 6).
    await lastTextHandling;
    clearTarget();
    toIdle();
  }

  async function toggle(newTarget) {
    if (state === STATES.IDLE || state === STATES.UNAVAILABLE) {
      await start(newTarget);
    } else {
      await stop();
    }
  }

  function subscribe(fn) {
    listeners.add(fn);
    fn(snapshot());
    return () => listeners.delete(fn);
  }

  return {
    start,
    stop,
    toggle,
    get state() {
      return state;
    },
    get progress() {
      return progress;
    },
    get model() {
      return model;
    },
    get device() {
      return device;
    },
    get reason() {
      return reason;
    },
    get speaking() {
      return speaking;
    },
    subscribe,
    _testing: {
      reset() {
        try {
          worker?.terminate();
        } catch {
          /* already gone */
        }
        rejectAllPending(new Error("reset"));
        worker = null;
        workerModelId = null;
        microphone = null;
        target = null;
        permissionDeniedThisSession = false;
        persisted = false;
        lastTextHandling = Promise.resolve();
        state = STATES.IDLE;
        progress = null;
        model = null;
        device = null;
        reason = null;
        speaking = false;
      },
    },
  };
}

// -------------------------------------------------------------- real deps

let singleton = null;

function defaultDeps() {
  return {
    createWorker: () => new Worker(new URL("./worker.js", import.meta.url), { type: "module" }),
    openMicrophone: ({ onFrame }) =>
      import("./capture.js").then(({ openMicrophone }) =>
        openMicrophone({
          onFrame,
          getUserMedia: (constraints) => navigator.mediaDevices.getUserMedia(constraints),
          AudioContext: window.AudioContext || window.webkitAudioContext,
          workletUrl: new URL("./capture-worklet.js", import.meta.url),
        }),
      ),
    storage: (() => {
      try {
        return typeof localStorage !== "undefined" ? localStorage : null;
      } catch {
        return null;
      }
    })(),
    isSecureContext: () => typeof window !== "undefined" && !!window.isSecureContext,
    hasWebAssembly: () => typeof WebAssembly !== "undefined",
    hasMediaDevices: () => typeof navigator !== "undefined" && !!navigator.mediaDevices?.getUserMedia,
    confirmDownload: () => Promise.resolve(true), // placeholder; a later slice adds the SPEC 5 modal
    notify: (message, level) => {
      import("../toast.svelte.js").then(({ problem, said }) => {
        (level === "error" ? problem : said)(message);
      });
    },
    persist: async () => {
      try {
        await navigator.storage?.persist?.();
      } catch {
        /* not fatal either way */
      }
    },
    navigatorLanguages: () => (typeof navigator !== "undefined" ? Array.from(navigator.languages || []) : []),
    now: () => Date.now(),
    assemble: (before, raw, entry) => import("./assemble.js").then(({ assemble }) => assemble(before, raw, entry)),
    models: {
      modelById: (id) => import("./models.js").then(({ modelById }) => modelById(id)),
      defaultModel: () => import("./models.js").then(({ DEFAULT_MODEL }) => DEFAULT_MODEL),
      vad: () => import("./models.js").then(({ VAD }) => VAD),
      pickLanguage: (entry, setting, navigatorLanguages) =>
        import("./models.js").then(({ pickLanguage }) => pickLanguage(entry, setting, navigatorLanguages)),
    },
  };
}

/// The one dictation service for the page. Built lazily so importing this
/// module never touches `Worker`, `localStorage`, or any other browser
/// global at load time.
export function getDictation() {
  if (!singleton) singleton = createDictationService(defaultDeps());
  return singleton;
}
