// The "browser built-in" dictation backend (SPEC-dictation.md 1.1, the
// browser-built-in backend row of 4.5, and section 7's "different product"
// paragraph). It wraps the Web Speech API's `SpeechRecognition` the same way
// `capture.js` wraps `getUserMedia`: the constructor arrives as a parameter
// so this module never touches a browser global directly and
// `web/checks/dictation-webspeech.mjs` can drive it under Node with a fake.
//
// Unlike the local backend there is no worker, no VAD, and no segmenter --
// the browser does its own endpointing and hands back one final result per
// utterance. This module's whole job is translating that event stream into
// the same three callbacks the worker's unsolicited messages produce:
// `onText(transcript)`, `onSpeech(speaking)`, and `onError(error)`.

// Chrome (and some other engines) end a `continuous` recognition on their own
// after a silence timeout or roughly a minute of audio, well before the user
// asked to stop. Restarting on `end` is how "continuous" is actually
// achieved; `stop()` sets `stopped` first so the restart is skipped once the
// session is really over.
export function createWebSpeechSession({ SpeechRecognition, language, onText, onSpeech, onError }) {
  const recognition = new SpeechRecognition();
  recognition.continuous = true;
  recognition.interimResults = false;
  recognition.lang = language || "";
  recognition.maxAlternatives = 1;

  let stopped = false;
  let endWaiters = [];

  function notifyEndWaiters() {
    const waiters = endWaiters;
    endWaiters = [];
    for (const resolve of waiters) resolve();
  }

  // `result.length` only grows across events for a `continuous` recognition
  // (Chrome never revisits an already-final result), so tracking the count
  // already handled is enough to find just the new final ones -- there are
  // no interim results to skip since `interimResults` is off.
  let handledResults = 0;
  function handleResult(event) {
    const results = event.results;
    for (let i = handledResults; i < results.length; i += 1) {
      const result = results[i];
      if (!result.isFinal) continue;
      const transcript = result[0]?.transcript ?? "";
      onText(transcript);
    }
    handledResults = results.length;
  }

  function handleSpeechStart() {
    onSpeech(true);
  }

  function handleSpeechEnd() {
    onSpeech(false);
  }

  function handleError(event) {
    const code = event.error;
    if (code === "not-allowed" || code === "service-not-allowed") {
      onError(Object.assign(new Error(code), { name: "NotAllowedError" }));
      return;
    }
    // "no-speech" (the utterance timed out with nothing heard) and "aborted"
    // (our own stop()) are ordinary, not failures.
    if (code === "no-speech" || code === "aborted") return;
    onError(new Error(event.message || code));
  }

  function handleEnd() {
    handledResults = 0;
    notifyEndWaiters();
    if (stopped) return;
    try {
      recognition.start();
    } catch {
      // The browser refused the restart (e.g. it was already starting);
      // nothing more to do -- a real stop() will still resolve normally.
    }
  }

  recognition.addEventListener("result", handleResult);
  recognition.addEventListener("speechstart", handleSpeechStart);
  recognition.addEventListener("speechend", handleSpeechEnd);
  recognition.addEventListener("error", handleError);
  recognition.addEventListener("end", handleEnd);

  recognition.start();

  async function stop() {
    if (stopped) return;
    stopped = true;
    const ended = new Promise((resolve) => {
      endWaiters.push(resolve);
    });
    try {
      recognition.stop();
    } catch {
      /* already stopped or never started */
    }
    await Promise.race([ended, new Promise((resolve) => setTimeout(resolve, 1000))]);
  }

  return { stop };
}
