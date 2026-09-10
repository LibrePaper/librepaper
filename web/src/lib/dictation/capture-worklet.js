// The AudioWorkletProcessor that turns the microphone's native sample rate
// into the fixed 16 kHz / 512-sample frames every recognizer model expects
// (docs/dictation-interfaces.md "capture.js and capture-worklet.js";
// SPEC-dictation.md 4.2, 4.3).
//
// This file runs in the AudioWorkletGlobalScope, not the main thread or a
// module graph Vite can rewrite imports for -- there is no `deps` seam here
// and no way to unit-test it under Node, so it is kept intentionally free of
// dependencies and of anything beyond arithmetic on typed arrays. The
// resampling and framing logic is simple enough to read directly; the
// correctness of the pipeline it feeds is covered by
// `checks/dictation-service.mjs` via a fake worklet output.
//
// `sampleRate` here is the AudioWorkletGlobalScope global (the context's
// rate), not an import -- eslint/no-undef would flag it in a normal module,
// but this file is never linted as one.

const TARGET_RATE = 16000;
const FRAME_SAMPLES = 512;

class LibrepaperDictationCapture extends AudioWorkletProcessor {
  constructor() {
    super();
    // Fractional read position into the resampled stream, carried across
    // process() calls so linear interpolation is continuous at block
    // boundaries rather than restarting every 128 samples.
    this._ratio = TARGET_RATE / sampleRate;
    this._position = 0; // in units of *input* samples
    this._prevInputSample = 0;
    this._outBuffer = new Float32Array(FRAME_SAMPLES);
    this._outFilled = 0;
  }

  process(inputs) {
    const input = inputs[0];
    const channel = input && input[0];
    if (!channel || channel.length === 0) return true;

    // Resample this block by walking the *output* clock and pulling a
    // linearly-interpolated input sample for each output tick. `_position`
    // stays in input-sample units so it can span across process() calls
    // without drift.
    let pos = this._position;
    const step = 1 / this._ratio; // input samples per output sample
    while (true) {
      const i0 = Math.floor(pos);
      if (i0 + 1 >= channel.length) break;
      const frac = pos - i0;
      // A fractional position carried over from the last block sits between
      // that block's final sample and this block's first: without the carried
      // sample, channel[-1] is undefined and the frame fills with NaN at
      // every rate that is not an integer multiple of 16 kHz, such as 44.1.
      const s0 = i0 < 0 ? this._prevInputSample : channel[i0];
      const s1 = channel[i0 + 1];
      const sample = s0 + (s1 - s0) * frac;
      this._pushSample(sample);
      pos += step;
    }
    // Carry the leftover fractional position into the next block, shifted
    // back by the number of input samples this block consumed.
    this._position = pos - channel.length;
    this._prevInputSample = channel[channel.length - 1];
    return true;
  }

  _pushSample(sample) {
    this._outBuffer[this._outFilled] = sample;
    this._outFilled += 1;
    if (this._outFilled === FRAME_SAMPLES) {
      const frame = this._outBuffer;
      this._outBuffer = new Float32Array(FRAME_SAMPLES);
      this._outFilled = 0;
      this.port.postMessage(frame, [frame.buffer]);
    }
  }
}

registerProcessor("librepaper-dictation-capture", LibrepaperDictationCapture);
