// Main-thread microphone capture for dictation.
//
// Everything that touches the browser -- getUserMedia, AudioContext, and the
// worklet module's URL -- arrives through arguments rather than the global,
// so `web/checks/dictation-service.mjs` can drive this exact module under
// Node with fakes, following the seam in `web/src/lib/latex/local.js`.
const FIRST_FRAME_WAIT_MS = 2000;

export async function openMicrophone({ onFrame, getUserMedia, AudioContext, workletUrl }) {
  const stream = await getUserMedia({
    audio: {
      channelCount: 1,
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    },
  });

  let context;
  try {
    // Keep construction inside the cleanup boundary too: an unavailable
    // AudioContext must not strand the stream opened above.
    context = new AudioContext();
    // A context created after an awaited permission/model step may start
    // suspended even though the user clicked the dictation button. Resume it
    // before wiring the worklet, otherwise no render quanta (and no frames)
    // are produced.
    if (context.state === "suspended") await context.resume();
    await context.audioWorklet.addModule(workletUrl);
    const source = context.createMediaStreamSource(stream);
    const node = new AudioWorkletNode(context, "librepaper-dictation-capture");
    source.connect(node);
    // The worklet has no output the graph needs to hear, but Chrome only
    // pulls a node's process() calls while it is part of a live render
    // graph reaching the destination -- the frames would silently
    // stop the moment a tab is backgrounded otherwise. connect(0)/gain 0
    // would work too; a plain connect to the destination is simplest and
    // costs nothing because the node emits no channels.
    node.connect(context.destination);

    // openMicrophone() resolves once frames are actually flowing,
    // not merely once the graph
    // is wired -- a worklet can take a render quantum or two to produce its
    // first 512-sample frame.
    // A device that never delivers a frame (some virtual inputs, a muted
    // hardware switch) must not leave the caller waiting forever with the
    // recording indicator lit: after two seconds the graph is considered
    // live and frames are simply forwarded as they come, if ever.
    await new Promise((resolve) => {
      const timer = setTimeout(() => {
        node.port.onmessage = (inner) => onFrame(inner.data);
        resolve();
      }, FIRST_FRAME_WAIT_MS);
      node.port.onmessage = (event) => {
        clearTimeout(timer);
        node.port.onmessage = (inner) => onFrame(inner.data);
        onFrame(event.data);
        resolve();
      };
    });

    return {
      sampleRate: 16000,
      async close() {
        for (const track of stream.getTracks()) track.stop();
        try {
          node.port.onmessage = null;
          node.disconnect();
          source.disconnect();
        } catch {
          /* already disconnected */
        }
        await context.close();
      },
    };
  } catch (error) {
    for (const track of stream.getTracks()) track.stop();
    try {
      await context.close();
    } catch {
      /* already closed */
    }
    throw error;
  }
}
