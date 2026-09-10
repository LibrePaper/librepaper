// Main-thread microphone capture for dictation (SPEC-dictation.md 4.2;
// docs/dictation-interfaces.md "capture.js and capture-worklet.js").
//
// Everything that touches the browser -- getUserMedia, AudioContext, and the
// worklet module's URL -- arrives through arguments rather than the global,
// so `web/checks/dictation-service.mjs` can drive this exact module under
// Node with fakes, following the seam in `web/src/lib/latex/local.js`.
export async function openMicrophone({ onFrame, getUserMedia, AudioContext, workletUrl }) {
  const stream = await getUserMedia({
    audio: {
      channelCount: 1,
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    },
  });

  const context = new AudioContext();
  try {
    await context.audioWorklet.addModule(workletUrl);
    const source = context.createMediaStreamSource(stream);
    const node = new AudioWorkletNode(context, "librepaper-dictation-capture");
    source.connect(node);
    // The worklet has no output the graph needs to hear, but Chrome only
    // pulls a node's process() calls while it is part of a live render
    // graph reaching the destination -- SPEC 4.2's frames would silently
    // stop the moment a tab is backgrounded otherwise. connect(0)/gain 0
    // would work too; a plain connect to the destination is simplest and
    // costs nothing because the node emits no channels.
    node.connect(context.destination);

    // openMicrophone() resolves once frames are actually flowing (the
    // contract in docs/dictation-interfaces.md), not merely once the graph
    // is wired -- a worklet can take a render quantum or two to produce its
    // first 512-sample frame.
    await new Promise((resolve) => {
      node.port.onmessage = (event) => {
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
