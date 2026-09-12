import { openMicrophone } from "../../src/lib/dictation/capture.js";

let failures = 0;
function check(what, condition, detail = "") {
  if (condition) return;
  failures += 1;
  console.error(`dictation-capture: FAIL ${what}${detail ? ` -- ${detail}` : ""}`);
}

const tracks = [{ stopped: false, stop() { this.stopped = true; } }];
let context;
class FakeAudioContext {
  constructor() {
    context = this;
    this.state = "suspended";
    this.destination = {};
    this.audioWorklet = { addModule: async () => {} };
    this.resumed = false;
    this.closed = false;
  }
  async resume() {
    if (FakeAudioContext.rejectResume) throw new Error("resume denied");
    this.resumed = true;
    this.state = "running";
  }
  createMediaStreamSource() { return { connect() {}, disconnect() {} }; }
  async close() { this.closed = true; }
}

class FakeAudioWorkletNode {
  constructor() {
    this.port = { onmessage: null };
    this.disconnect = () => {};
    queueMicrotask(() => this.port.onmessage?.({ data: new Float32Array(512) }));
  }
  connect() {}
}

const previous = globalThis.AudioWorkletNode;
globalThis.AudioWorkletNode = FakeAudioWorkletNode;
try {
  const frames = [];
  const mic = await openMicrophone({
    onFrame: (frame) => frames.push(frame),
    getUserMedia: async () => ({ getTracks: () => tracks }),
    AudioContext: FakeAudioContext,
    workletUrl: "fake-worklet.js",
  });
  check("resumes a suspended AudioContext", context.resumed);
  check("waits for the first frame", frames.length === 1);
  await mic.close();
  check("stops the input track on close", tracks[0].stopped);
  check("closes the AudioContext", context.closed);

  tracks[0].stopped = false;
  FakeAudioContext.rejectResume = true;
  let rejected = false;
  try {
    await openMicrophone({
      onFrame() {},
      getUserMedia: async () => ({ getTracks: () => tracks }),
      AudioContext: FakeAudioContext,
      workletUrl: "fake-worklet.js",
    });
  } catch {
    rejected = true;
  }
  check("propagates a rejected AudioContext resume", rejected);
  check("cleans up the stream when resume fails", tracks[0].stopped);
  check("closes the context when resume fails", context.closed);
} finally {
  if (previous === undefined) delete globalThis.AudioWorkletNode;
  else globalThis.AudioWorkletNode = previous;
}

if (failures) process.exit(1);
console.log("dictation-capture: suspended-context resume and cleanup checked");
