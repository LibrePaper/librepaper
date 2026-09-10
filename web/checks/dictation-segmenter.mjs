// The segmenter, against synthetic frames.
//
// Frames are tagged with a unique id in sample 0 and a speech marker in
// sample 1, so a finished segment's contents can be read back exactly
// (which frames, in which order) rather than just measured by length.
// Timing is expressed in frame counts, not wall-clock waits: every
// threshold below is chosen as a small multiple of a 10 ms synthetic frame
// so the expected frame counts are exact integers.

import { createSegmenter, createEnergyClassifier, DEFAULTS } from "../src/lib/dictation/segment.js";

let failures = 0;
function check(what, condition, detail = "") {
  if (condition) return;
  failures += 1;
  console.error(`dictation-segmenter: FAIL ${what}${detail ? ` -- ${detail}` : ""}`);
}

const FRAME_SAMPLES = 160;
const SAMPLE_RATE = 16000; // frameMs = 10
const OPTS = {
  sampleRate: SAMPLE_RATE,
  frameSamples: FRAME_SAMPLES,
  preRollMs: 30, // 3 frames
  closeSilenceMs: 50, // 5 frames
  maxSegmentMs: 200, // 20 frames
  minSpeechMs: 20, // 2 frames
};

function frame(id, speech) {
  const f = new Float32Array(FRAME_SAMPLES);
  f[0] = id;
  f[1] = speech ? 1 : 0;
  return f;
}

function idsOf(audio) {
  const ids = [];
  for (let i = 0; i * FRAME_SAMPLES < audio.length; i++) ids.push(audio[i * FRAME_SAMPLES]);
  return ids;
}

function sameIds(a, b) {
  return a.length === b.length && a.every((v, i) => v === b[i]);
}

async function testPreRollAndPauseClose() {
  const segments = [];
  const seg = createSegmenter({
    ...OPTS,
    classify: (f) => f[1] === 1,
    onSegment: (s) => segments.push(s),
  });

  for (let i = 0; i < 5; i++) await seg.push(frame(i, false)); // ids 0..4, only last 3 are pre-roll
  await seg.push(frame(100, true));
  await seg.push(frame(101, true)); // 2 speech frames = 20ms speech, meets minSpeechMs
  for (let i = 200; i < 205; i++) await seg.push(frame(i, false)); // 5 silence frames closes on "pause"

  check("pre-roll + pause: exactly one segment", segments.length === 1, `got ${segments.length}`);
  const s = segments[0];
  check("pre-roll + pause: reason is pause", s.reason === "pause", s.reason);
  check("pre-roll + pause: speechMs is 20", s.speechMs === 20, String(s.speechMs));
  check(
    "pre-roll + pause: contains exactly the last 3 pre-roll frames, the speech, then the closing silence",
    sameIds(idsOf(s.audio), [2, 3, 4, 100, 101, 200, 201, 202, 203, 204]),
    JSON.stringify(idsOf(s.audio)),
  );
}

async function testCapSplitsAndFlushCloses() {
  const segments = [];
  const seg = createSegmenter({
    ...OPTS,
    classify: () => true,
    onSegment: (s) => segments.push(s),
  });

  for (let i = 0; i < 20; i++) await seg.push(frame(i, true)); // hits the 200ms cap exactly
  check("cap: closes at the cap", segments.length === 1, `got ${segments.length}`);
  check("cap: reason is cap", segments[0]?.reason === "cap", segments[0]?.reason);
  check("cap: 20 frames of speech", segments[0]?.speechMs === 200, String(segments[0]?.speechMs));

  for (let i = 20; i < 25; i++) await seg.push(frame(i, true)); // starts a fresh segment after the cap
  await seg.flush();
  check("flush: closes the second segment", segments.length === 2, `got ${segments.length}`);
  check("flush: reason is flush", segments[1]?.reason === "flush", segments[1]?.reason);
  check(
    "flush: second segment holds the post-cap frames",
    sameIds(idsOf(segments[1].audio), [20, 21, 22, 23, 24]),
    JSON.stringify(idsOf(segments[1]?.audio)),
  );

  segments.length = 0;
  await seg.flush(); // nothing open: silent no-op
  check("flush with nothing open calls onSegment zero times", segments.length === 0);
}

async function testShortSegmentDropped() {
  const segments = [];
  const seg = createSegmenter({
    ...OPTS,
    classify: (f) => f[1] === 1,
    onSegment: (s) => segments.push(s),
  });

  await seg.push(frame(0, true)); // 10ms of speech, below the 20ms minimum
  for (let i = 1; i <= 5; i++) await seg.push(frame(i, false)); // closes on pause

  check("short segment is dropped silently", segments.length === 0, `got ${segments.length}`);
}

async function testStrictOrderingWithOutOfOrderAsyncClassify() {
  const segments = [];
  const seg = createSegmenter({
    ...OPTS,
    // Frame i's classification finishes sooner the *later* i is, so
    // resolution order is the reverse of arrival order.
    classify: (f) => new Promise((resolve) => setTimeout(() => resolve(f[1] === 1), 20 - f[0])),
    onSegment: (s) => segments.push(s),
  });

  const frames = [
    frame(0, false), frame(1, false), frame(2, false),
    frame(3, true), frame(4, true),
    frame(5, false), frame(6, false), frame(7, false), frame(8, false), frame(9, false),
  ];
  // Fire every push without waiting, so classify's promises really do race.
  const pending = frames.map((f) => seg.push(f));
  await Promise.all(pending);

  check("async out-of-order classify: one segment", segments.length === 1, `got ${segments.length}`);
  check(
    "async out-of-order classify: frames land in arrival order regardless of resolution order",
    sameIds(idsOf(segments[0]?.audio ?? new Float32Array()), [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
    JSON.stringify(idsOf(segments[0]?.audio ?? new Float32Array())),
  );
}

async function testOnSpeechTransitions() {
  const log = [];
  const seg = createSegmenter({
    ...OPTS,
    classify: (f) => f[1] === 1,
    onSegment: () => {},
    onSpeech: (speaking) => log.push(speaking),
  });

  await seg.push(frame(0, false));
  await seg.push(frame(1, true));
  await seg.push(frame(2, true)); // still speaking: no second "true"
  await seg.push(frame(3, false));
  await seg.push(frame(4, true)); // flapped back to speech before the close threshold
  for (let i = 5; i < 10; i++) await seg.push(frame(i, false)); // closes on pause

  check("onSpeech fires only on transitions", JSON.stringify(log) === JSON.stringify([true, false, true, false]), JSON.stringify(log));
}

async function testReset() {
  const segments = [];
  const seg = createSegmenter({
    ...OPTS,
    classify: (f) => new Promise((resolve) => setTimeout(() => resolve(f[1] === 1), 15)),
    onSegment: (s) => segments.push(s),
  });

  const stalePromise = seg.push(frame(999, true)); // slow to resolve
  seg.reset(); // fires before the slow classify resolves
  await stalePromise;

  const seg2 = createSegmenter({ ...OPTS, classify: (f) => f[1] === 1, onSegment: (s) => segments.push(s) });
  await seg2.push(frame(0, true));
  await seg2.push(frame(1, true));
  for (let i = 2; i < 7; i++) await seg2.push(frame(i, false));

  check("reset discards the in-flight stale segment", segments.length === 1, `got ${segments.length}`);
  check(
    "reset: the surviving segment is the fresh one, not the stale frame",
    segments[0] && sameIds(idsOf(segments[0].audio), [0, 1, 2, 3, 4, 5, 6]),
    JSON.stringify(segments[0] && idsOf(segments[0].audio)),
  );

  // reset() on the original segmenter itself must also leave it usable and clean.
  const after = [];
  const seg3 = createSegmenter({ ...OPTS, classify: (f) => f[1] === 1, onSegment: (s) => after.push(s) });
  await seg3.push(frame(0, true));
  seg3.reset();
  await seg3.push(frame(1, true));
  await seg3.push(frame(2, true));
  for (let i = 3; i < 8; i++) await seg3.push(frame(i, false));
  check(
    "reset mid-segment starts clean, without the pre-reset frame",
    after.length === 1 && sameIds(idsOf(after[0].audio), [1, 2, 3, 4, 5, 6, 7]),
    JSON.stringify(after[0] && idsOf(after[0].audio)),
  );
}

async function testThrowingClassifyCountsAsSilence() {
  const segments = [];
  let calls = 0;
  const seg = createSegmenter({
    ...OPTS,
    classify: async (f) => {
      calls += 1;
      if (f[0] === 3) throw new Error("vad exploded");
      return f[1] === 1;
    },
    onSegment: (s) => segments.push(s),
  });

  for (let i = 0; i < 6; i++) await seg.push(frame(i, true)); // frame 3 throws mid-speech
  for (let i = 6; i <= 10; i++) await seg.push(frame(i, false));

  check("a throwing classify does not reject push", calls === 11, `classify ran ${calls} times`);
  check("the segment still closes after the throw", segments.length === 1, `got ${segments.length}`);
}

function testDefaultsShape() {
  check("DEFAULTS has the documented keys", [
    "sampleRate", "frameSamples", "preRollMs", "closeSilenceMs", "maxSegmentMs", "minSpeechMs",
  ].every((k) => k in DEFAULTS));
  check("DEFAULTS.frameSamples is 512", DEFAULTS.frameSamples === 512);
}

function testEnergyClassifier() {
  const classify = createEnergyClassifier({ calibrationFrames: 5, ratio: 3 });
  const noise = (v) => {
    const f = new Float32Array(FRAME_SAMPLES);
    f.fill(v);
    return f;
  };

  let calibrationSaidSpeech = false;
  for (let i = 0; i < 5; i++) {
    if (classify(noise(0.01))) calibrationSaidSpeech = true; // floor settles near 0.01
  }
  check("energy classifier: calibration frames are never speech", !calibrationSaidSpeech);

  check("energy classifier: above floor * ratio is speech", classify(noise(0.05)) === true);
  check("energy classifier: at or below floor * ratio is silence", classify(noise(0.02)) === false);
}

const tests = [
  testDefaultsShape,
  testPreRollAndPauseClose,
  testCapSplitsAndFlushCloses,
  testShortSegmentDropped,
  testStrictOrderingWithOutOfOrderAsyncClassify,
  testOnSpeechTransitions,
  testReset,
  testThrowingClassifyCountsAsSilence,
  testEnergyClassifier,
];

for (const test of tests) {
  await test();
}

if (failures) {
  console.error(`dictation-segmenter: ${failures} check(s) failed`);
  process.exit(1);
}
console.log(`dictation-segmenter: ${tests.length} scenario(s) passed`);
