// The recognizer worker: detection, segmentation and recognition run
// together in one worker rather than split across the service. It owns the
// Transformers.js pipeline, the Silero VAD, and the segmenter (from
// ./segment.js, written in parallel against that module's contract -- this
// file must not implement it).
//
// Protocol follows the LaTeX renderer worker (renderer-worker.js): every
// request carries an `id`, replies carry the same `id`, and unsolicited
// messages carry `id: null`. Every failure becomes `{ id, kind: "error",
// message }` rather than an uncaught rejection, so one bad segment does not
// take the worker down with it.
import { pipeline, AutoModel, Tensor, env } from "@huggingface/transformers";
import { createSegmenter, createEnergyClassifier } from "./segment.js";
import { whisperLanguage } from "./language.js";

// The ONNX Runtime wasm binary and its .mjs loader, served from this origin
// and never a CDN. Transformers.js's own dist only ships the .mjs
// glue for the browser bundle it picks (onnxruntime-web's threaded "asyncify"
// build); the paired .wasm binary lives in onnxruntime-web, a dependency of
// @huggingface/transformers already present in node_modules. Its exports map
// only publishes these two files at its package root (not under "./dist/"),
// which is the specifier Vite must be given. Passing both as an explicit
// { mjs, wasm } object, rather than a directory prefix, is also what lets
// Transformers.js pre-load and cache the wasm binary itself.
env.backends.onnx.wasm.wasmPaths = {
  mjs: new URL("onnxruntime-web/ort-wasm-simd-threaded.asyncify.mjs", import.meta.url).href,
  wasm: new URL("onnxruntime-web/ort-wasm-simd-threaded.asyncify.wasm", import.meta.url).href,
};
// Models come from the Hub, pinned by revision, and stay in the browser's
// Cache Storage across sessions.
env.allowRemoteModels = true;
env.useBrowserCache = true;

// The pipeline call options per catalog id. A table, not a switch, so `tests/unit/dictation-models.mjs`
// can assert every catalog entry is covered by reading this file as text.
const CALL_OPTIONS = {
  "whisper-base": (language) => (language ? { language: whisperLanguage(language), task: "transcribe" } : { task: "transcribe" }),
  "whisper-small": (language) => (language ? { language: whisperLanguage(language), task: "transcribe" } : { task: "transcribe" }),
  "parakeet-ctc": () => ({}),
};

function callOptionsFor(modelId, language) {
  const make = CALL_OPTIONS[modelId];
  return make ? make(language) : {};
}

function extractText(result) {
  if (typeof result?.text === "string") return result.text;
  if (Array.isArray(result) && typeof result[0]?.text === "string") return result[0].text;
  return "";
}

// Silero VAD expects 16 kHz float32 frames of shape [1, 512], a constant
// sample-rate tensor, and a running [2, 1, 128] state it hands back each call.
const VAD_SAMPLE_RATE = new Tensor("int64", [16000n], [1]);
function freshVadState() {
  return new Tensor("float32", new Float32Array(2 * 1 * 128), [2, 1, 128]);
}

// Speech above 0.5, silence below 0.35: hysteresis so a probability dip mid
// word does not chop the segment.
const VAD_ENTER = 0.5;
const VAD_EXIT = 0.35;

let recognizer = null;
let recognizerModel = null; // the catalog entry passed to `load`
let vadModel = null;
let segmenter = null;
let language = null;
// Segments are transcribed off the frame chain so the segmenter keeps judging
// frames while the recognizer works, but strictly one after another so their
// text arrives in the order it was spoken. `stop` waits on this chain before
// replying, which is what lets the service insert the final sentence before
// it goes idle (contract: "stopped after the flushed segment's text").
let transcriptions = Promise.resolve();

function post(message, transfer) {
  self.postMessage(message, transfer);
}

function reply(id, kind, extra) {
  post({ id, kind, ...extra });
}

// classify() for the segmenter: the VAD when it loaded, otherwise the energy
// fallback. A fresh closure per `start` so its hysteresis and
// running state do not leak from one dictation session into the next.
function makeClassifier() {
  if (!vadModel) return createEnergyClassifier();
  let state = freshVadState();
  let speaking = false;
  return async (frame) => {
    const input = new Tensor("float32", frame, [1, frame.length]);
    const { output, stateN } = await vadModel({ input, sr: VAD_SAMPLE_RATE, state });
    state = stateN;
    const probability = output.data[0];
    speaking = speaking ? probability > VAD_EXIT : probability > VAD_ENTER;
    return speaking;
  };
}

async function loadRecognizer(model, device, id) {
  return pipeline("automatic-speech-recognition", model.repo, {
    dtype: model.dtype,
    device,
    revision: model.revision,
    progress_callback: (data) => {
      if (data?.status !== "progress") return;
      reply(id, "progress", { file: data.file, loaded: data.loaded, total: data.total });
    },
  });
}

async function loadVad(vad) {
  try {
    return await AutoModel.from_pretrained(vad.repo, {
      revision: vad.revision,
      config: { model_type: "custom" },
      dtype: "fp32",
    });
  } catch {
    // Falls back to the energy classifier; not fatal to `load`.
    return null;
  }
}

async function handleLoad(id, data) {
  recognizerModel = data.model;
  const requested = data.device || "auto";
  let device = requested === "auto" ? "webgpu" : requested;
  try {
    recognizer = await loadRecognizer(recognizerModel, device, id);
  } catch (error) {
    if (requested !== "auto") throw error;
    device = "wasm";
    recognizer = await loadRecognizer(recognizerModel, device, id);
  }
  vadModel = await loadVad(data.vad);
  reply(id, "loaded", { device });
}

async function handleStart(id, data) {
  language = data.language ?? null;
  segmenter = createSegmenter({
    classify: makeClassifier(),
    onSegment: ({ audio, reason }) => {
      const spokenIn = language;
      transcriptions = transcriptions
        .then(async () => {
          const result = await recognizer(audio, callOptionsFor(recognizerModel.id, spokenIn));
          post({ id: null, kind: "text", text: extractText(result), language: spokenIn, reason });
        })
        .catch((error) => post({ id: null, kind: "error", message: error?.message || String(error) }));
    },
    onSpeech: (speaking) => post({ id: null, kind: "speech", speaking }),
  });
  reply(id, "started");
}

async function handleFrame(data) {
  if (!segmenter) return;
  await segmenter.push(new Float32Array(data.audio));
}

async function handleStop(id) {
  if (segmenter) await segmenter.flush();
  await transcriptions;
  reply(id, "stopped");
}

async function handleTranscribe(id, data) {
  const result = await recognizer(new Float32Array(data.audio), callOptionsFor(recognizerModel.id, data.language ?? null));
  reply(id, "text", { text: extractText(result), language: data.language ?? null, reason: "direct" });
}

async function handleUnload(id) {
  await transcriptions;
  await recognizer?.dispose?.();
  recognizer = null;
  recognizerModel = null;
  vadModel = null;
  segmenter = null;
  language = null;
  reply(id, "unloaded");
}

// Requests are handled strictly in the order they arrive -- including
// `frame`, which has no id of its own -- so a `stop` always flushes audio
// that was already pushed rather than racing it.
let queue = Promise.resolve();

self.onmessage = ({ data }) => {
  const { id = null, kind } = data;
  queue = queue.then(async () => {
    try {
      if (kind === "load") await handleLoad(id, data);
      else if (kind === "start") await handleStart(id, data);
      else if (kind === "frame") await handleFrame(data);
      else if (kind === "stop") await handleStop(id);
      else if (kind === "transcribe") await handleTranscribe(id, data);
      else if (kind === "unload") await handleUnload(id);
      else throw new Error(`Unknown dictation worker request: ${kind}`);
    } catch (error) {
      post({ id, kind: "error", message: error?.message || String(error) });
    }
  });
};
