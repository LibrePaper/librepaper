# Dictation: module interfaces

The contract between the modules that implement [SPEC-dictation.md](../SPEC-dictation.md).
Each module is written against this page rather than against the others'
source, so they can be written in parallel. Where this page and the SPEC
differ, this page wins; the one deliberate difference is that the voice
activity detector and the segmenter run inside the recognizer worker
(SPEC 4.3 and 4.4 describe them from the service's point of view).

All modules live in `web/src/lib/dictation/`. Every module that touches the
browser reaches it only through an injected `deps` object, following
`web/src/lib/latex/local.js`, so `web/checks/*.mjs` can run it under Node.

```
models.js           catalog of models, revisions, per-model options
segment.js          pure: frames in, segments out
assemble.js         pure: raw segment text -> text to insert at the caret
worker.js           recognizer worker: VAD, segmenter, Transformers.js pipeline
capture.js          microphone -> 16 kHz mono frames (main thread side)
capture-worklet.js  AudioWorkletProcessor: resample and frame
targets.js          textarea and editor insertion targets
settings.js         localStorage keys
service.js          the state machine every component talks to
```

## segment.js

```js
export const DEFAULTS = Object.freeze({
  sampleRate: 16000,
  frameSamples: 512,      // 32 ms at 16 kHz
  preRollMs: 300,
  closeSilenceMs: 600,
  maxSegmentMs: 20000,
  minSpeechMs: 250,
});

// classify(frame: Float32Array) -> boolean | Promise<boolean>; true = speech.
// onSegment({ audio: Float32Array, speechMs: number, reason: "pause" | "cap" | "flush" })
// onSpeech(speaking: boolean) is optional and fires on transitions.
export function createSegmenter({ classify, onSegment, onSpeech, ...overridesOfDEFAULTS });
// -> { push(frame: Float32Array): Promise<void>, flush(): Promise<void>, reset(): void }
// push() handles frames strictly in order even when classify is async: it
// queues internally, and a call never observes a later frame's decision.
// A segment includes preRollMs of audio before its first speech frame.
// A closed segment with less than minSpeechMs of speech is dropped silently.

// Energy fallback. The first calibrationFrames set the noise floor from RMS;
// after that a frame is speech when its RMS exceeds floor * ratio.
export function createEnergyClassifier({ calibrationFrames = 62, ratio = 3 } = {});
// -> classify(frame) synchronous
```

## assemble.js

```js
// before: the text before the caret (only its tail matters; may be "").
// raw: the recognizer's text for one segment.
// model: { capitalizes: boolean, punctuates: boolean } (from the catalog).
// Returns the exact string to insert, "" when the segment is dropped.
export function assemble(before, raw, model);

// Exported for checks.
export function cleanSegment(raw);   // trim; drop [BLANK_AUDIO], (music), ♪, and similar noise tokens
export function needsSpace(before);  // false at "" or after whitespace, newline, or an opening bracket/quote
export function startsSentence(before); // true at "" or after . ! ? (optionally followed by closing quotes/brackets and whitespace)
```

Rules: see SPEC 4.6. When `model.capitalizes` is false and `startsSentence(before)`
is true, the first letter is upper-cased. When `model.punctuates` is false no
punctuation is invented. Nothing is escaped.

## models.js

```js
export const MODELS = Object.freeze([
  {
    id: "whisper-base",
    label: "Whisper base",
    kind: "local",                       // "local" | "browser"
    repo: "onnx-community/whisper-base",
    revision: "1846881b6b3a3024392c1eea3ad983695bc23925",
    dtype: "q8",
    sizeBytes: 60_000_000,               // approximate, shown before download
    languages: [...99 BCP 47 tags...],   // or "any" for the browser backend
    acceptsLanguage: true,
    punctuates: true,
    capitalizes: true,
    description: "The light option. 99 languages.",
  },
  // whisper-small: repo onnx-community/whisper-small, revision 36050c46d777d46dc4b5f43f6d90574fc38f8732, ~250 MB
  // parakeet-ctc: repo onnx-community/parakeet-ctc-0.6b-ONNX, revision 7df2cab7aed886b8b7f80d68a8214007e4847601, ~600 MB,
  //   25 European languages, acceptsLanguage false
  // browser: kind "browser", sizeBytes 0, languages "any", label "Browser built-in",
  //   description says audio may be sent to the browser vendor
]);
export const DEFAULT_MODEL = "whisper-small";
export const VAD = Object.freeze({
  repo: "onnx-community/silero-vad",
  revision: "e71cae966052b992a7eca6b17738916ce0eca4ec",
});
export function modelById(id);                                  // -> entry or undefined
export function supportsLanguage(model, tag);                   // BCP 47 primary subtag match; "any" matches all
export function pickLanguage(model, setting, navigatorLanguages); // -> tag or null (auto). SPEC 4.9.
```

## worker.js protocol

Every request carries an `id`; replies to it carry the same `id`. Unsolicited
messages carry `id: null`. Any failure is `{ id, kind: "error", message }`.

| Request | Replies |
| --- | --- |
| `{ id, kind: "load", model, vad, device: "auto" \| "webgpu" \| "wasm" }` | zero or more `{ id, kind: "progress", file, loaded, total }`, then `{ id, kind: "loaded", device: "webgpu" \| "wasm" }` |
| `{ id, kind: "start", language: string \| null }` | `{ id, kind: "started" }`. Resets the segmenter. |
| `{ id: null, kind: "frame", audio: Float32Array }` | none directly. The buffer is transferred. |
| `{ id, kind: "stop" }` | `{ id, kind: "stopped" }` after the flushed segment's text, if any. |
| `{ id, kind: "transcribe", audio: Float32Array, language }` | `{ id, kind: "text", text, language, reason: "direct" }`. For checks and the built-in-VAD-less path. |
| `{ id, kind: "unload" }` | `{ id, kind: "unloaded" }` |

Unsolicited, while started:

- `{ id: null, kind: "speech", speaking: boolean }` on VAD transitions.
- `{ id: null, kind: "text", text, language, reason: "pause" \| "cap" \| "flush" }` per segment, in order.

`model` is a catalog entry. The worker keeps a table of pipeline call options
per model (Whisper: `language`, `task: "transcribe"`; others: none). The ONNX
Runtime wasm and its loader are bundled by Vite and served from this origin,
never from a CDN. Transformers.js's browser cache is left on; `revision` is
passed to the pipeline so the cache key names exact bytes.

## capture.js and capture-worklet.js

```js
// Opens the microphone and delivers 512-sample Float32Array frames at 16 kHz
// mono. Resolves once frames are flowing. close() stops every track and
// closes the AudioContext.
export async function openMicrophone({ onFrame, getUserMedia, AudioContext, workletUrl });
// -> { close(): Promise<void>, sampleRate: 16000 }
```

The worklet resamples from the context rate to 16 kHz (linear interpolation
is acceptable) and posts frames of exactly 512 samples.

## targets.js

```js
// A target: { kind, insert(text): void, before(limit = 200): string, alive(): boolean, focus(): void }
export function textareaTarget(element);   // also for <input type="text">
export function editorTarget(editor);      // editor exposes insertAtCaret(text), textBeforeCaret(limit), vimMode(), focus()
export function targetForActiveElement(doc, editorFor); // editorFor(element) -> editor or null
```

`textareaTarget.insert` replaces the selection, moves the caret after the
inserted text, and dispatches a bubbling `input` event. `editorTarget.insert`
throws `VimNormalMode` when `editor.vimMode() === "normal"`; the service turns
that into the SPEC 6 toast.

Editor.svelte gains three exports in a later slice: `insertAtCaret(text)`,
`textBeforeCaret(limit)`, `vimMode()` returning `"insert" | "normal" | null`.

## settings.js

```js
export const KEYS = Object.freeze({
  backend: "librepaper-dictation-backend",     // "local" | "browser"
  model: "librepaper-dictation-model",         // catalog id
  language: "librepaper-dictation-language",   // "auto" | BCP 47
  confirmed: "librepaper-dictation-confirmed", // JSON array of catalog ids
});
export function readSettings(storage);                 // -> { backend, model, language, confirmed: [] } with defaults
export function writeSettings(storage, patch);
export function isConfirmed(storage, modelId);
export function confirm(storage, modelId);
```

## service.js

```js
export function createDictationService(deps);
// deps = {
//   createWorker(): Worker-like { postMessage, onmessage, onerror, terminate },
//   openMicrophone({ onFrame }) -> Promise<{ close }>,   // capture.js bound to real browser APIs
//   storage,                                              // localStorage or null
//   isSecureContext: () => boolean,
//   hasWebAssembly: () => boolean,
//   hasMediaDevices: () => boolean,
//   confirmDownload: (model) => Promise<boolean>,        // the SPEC 5 modal
//   notify: (message, level) => void,                    // toast
//   persist: () => Promise<void>,                        // navigator.storage.persist, once
//   navigatorLanguages: () => string[],
//   now: () => number,
// }
// -> {
//   start(target, { model?, language? } = {}): Promise<void>,
//   stop(): Promise<void>,
//   toggle(target): Promise<void>,
//   get state(): "idle" | "loading" | "listening" | "transcribing" | "unavailable",
//   get progress(): { loaded, total, file } | null,
//   get model(): catalog entry | null,
//   get device(): "webgpu" | "wasm" | null,
//   get reason(): string | null,        // when unavailable
//   get speaking(): boolean,
//   subscribe(fn): unsubscribe,          // fn(snapshot) on every change
//   _testing: { reset() },
// }
export function getDictation();  // page-wide singleton built from real deps
export function setDownloadConfirmation(fn); // fn(entry) -> Promise<boolean>; the SPEC 5 dialog registers here
```

State transitions, from SPEC 4.1 and 6:

- `start` when not `idle`: `stop` first, then proceed with the new target.
- Preconditions fail -> `unavailable` with `reason`, plus a toast, no worker.
- Model not confirmed -> `confirmDownload`; false -> stay `idle`.
- `loading` until `loaded`; the microphone opens only after that.
- `listening`; on each `text` message: `assemble(target.before(), text, model)`,
  then `target.insert`. If `target.alive()` is false: stop, drop, toast.
- `transcribing` while a segment is with the worker (from the first `speech:false`
  after speech until the next `text`), back to `listening` after.
- `stop`: close the microphone first, send `stop`, wait for `stopped`, insert
  any last text, then `idle`. The worker stays loaded for the next start.
- Worker `onerror`: terminate, toast, `idle`.
- The pipeline's `progress` messages are mirrored into `progress`.

## webspeech.js

```js
export function createWebSpeechSession({ SpeechRecognition, language, onText, onSpeech, onError });
// -> { stop(): Promise<void> }
```

The browser built-in backend. The service uses it instead of the worker and
the microphone when the resolved catalog entry has `kind: "browser"`, and
reports `device: "browser"`. Final results go through the same assembly
and insertion path as worker text.

## purge.js

```js
export async function removeCachedModel(entry, { caches }); // -> number of cache entries removed
```
