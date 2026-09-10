// Frames in, segments out.
//
// The recognizer models in the catalog transcribe a finished clip, not a
// stream, so something has to decide where one clip ends and the next
// begins. That is all this module does: it does not know about the
// microphone, the worker, or any browser API, so it is exercised directly
// under Node in web/checks/dictation-segmenter.mjs rather than through an
// injected `deps` object like the browser-facing modules.
//
// `classify` may be sync or async (the real VAD runs in the same worker and
// is async; the energy fallback below is sync). Frames arrive one at a time
// through push() and must be judged in the order they arrived even though
// classify() may resolve out of order -- a fast frame must never jump ahead
// of a slow one and flip the segment boundary early. push() gets this by
// chaining every frame's work onto one promise, so frame N's classification
// and bookkeping fully finish before frame N+1's classification even starts.
//
// reset() is synchronous (SPEC: the caller does not await it, e.g. on
// worker "start") but frames already queued in that chain may still be
// mid-flight. A generation counter, bumped on reset, lets those stale
// continuations notice they no longer own the current state and bail out
// instead of resurrecting a segment nobody asked for.

export const DEFAULTS = Object.freeze({
  sampleRate: 16000,
  frameSamples: 512, // 32 ms at 16 kHz
  preRollMs: 300,
  closeSilenceMs: 600,
  maxSegmentMs: 20000,
  minSpeechMs: 250,
});

function concat(frames, totalSamples) {
  const audio = new Float32Array(totalSamples);
  let offset = 0;
  for (const frame of frames) {
    audio.set(frame, offset);
    offset += frame.length;
  }
  return audio;
}

export function createSegmenter({ classify, onSegment, onSpeech, ...overrides }) {
  const opts = { ...DEFAULTS, ...overrides };
  const frameMs = (opts.frameSamples / opts.sampleRate) * 1000;
  const preRollFrames = Math.round(opts.preRollMs / frameMs);

  let generation = 0;
  let chain = Promise.resolve();

  let ring = []; // pre-roll: most recent frames while not inside a segment
  let inSegment = false;
  let chunks = [];
  let totalSamples = 0;
  let durationMs = 0;
  let speechMs = 0;
  let silenceRunMs = 0;
  let speaking = false;

  function pushRing(frame) {
    ring.push(frame);
    if (ring.length > preRollFrames) ring.shift();
  }

  function closeSegment(reason) {
    const finishedSpeechMs = speechMs;
    const audio = concat(chunks, totalSamples);
    chunks = [];
    totalSamples = 0;
    durationMs = 0;
    speechMs = 0;
    silenceRunMs = 0;
    inSegment = false;
    ring = []; // the next pre-roll starts fresh right after this segment
    if (finishedSpeechMs >= opts.minSpeechMs) {
      onSegment({ audio, speechMs: finishedSpeechMs, reason });
    }
  }

  function addFrame(frame) {
    chunks.push(frame);
    totalSamples += frame.length;
    durationMs += frameMs;
  }

  function judge(frame, isSpeech) {
    if (isSpeech !== speaking) {
      speaking = isSpeech;
      onSpeech?.(isSpeech);
    }

    if (!inSegment) {
      if (!isSpeech) {
        pushRing(frame);
        return;
      }
      chunks = [...ring];
      totalSamples = chunks.reduce((sum, f) => sum + f.length, 0);
      durationMs = chunks.length * frameMs;
      ring = [];
      inSegment = true;
      addFrame(frame);
      speechMs = frameMs;
      silenceRunMs = 0;
      return;
    }

    addFrame(frame);
    if (isSpeech) {
      speechMs += frameMs;
      silenceRunMs = 0;
    } else {
      silenceRunMs += frameMs;
    }

    if (silenceRunMs >= opts.closeSilenceMs) {
      closeSegment("pause");
    } else if (durationMs >= opts.maxSegmentMs) {
      closeSegment("cap");
    }
  }

  function push(frame) {
    const gen = generation;
    chain = chain.then(() => classify(frame)).then((isSpeech) => {
      if (gen !== generation) return;
      judge(frame, Boolean(isSpeech));
    });
    return chain;
  }

  function flush() {
    const gen = generation;
    chain = chain.then(() => {
      if (gen !== generation) return;
      if (inSegment) closeSegment("flush");
    });
    return chain;
  }

  function reset() {
    generation += 1;
    chain = Promise.resolve();
    ring = [];
    inSegment = false;
    chunks = [];
    totalSamples = 0;
    durationMs = 0;
    speechMs = 0;
    silenceRunMs = 0;
    speaking = false;
  }

  return { push, flush, reset };
}

// Silero VAD is the primary classifier; this is the fallback when it fails
// to load. It calibrates against whatever room noise is present at start
// rather than a fixed threshold, because a fixed number is wrong for either
// a quiet room or a noisy one.
export function createEnergyClassifier({ calibrationFrames = 62, ratio = 3 } = {}) {
  let seen = 0;
  let sumRms = 0;
  let floor = 0;

  return function classify(frame) {
    let sumSquares = 0;
    for (let i = 0; i < frame.length; i++) sumSquares += frame[i] * frame[i];
    const rms = Math.sqrt(sumSquares / frame.length);

    if (seen < calibrationFrames) {
      sumRms += rms;
      seen += 1;
      floor = sumRms / seen;
      return false;
    }
    return rms > floor * ratio;
  };
}
