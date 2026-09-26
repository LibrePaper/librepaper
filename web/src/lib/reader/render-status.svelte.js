// Reactive status associated with the render coordinator. FramePreview owns
// the replayable HTML/PDF payload; this controller owns the UI-facing state
// around attempts and the one-shot DOCX artifact.
export function createRenderStatus() {
  const state = $state({
    compiling: false,
    lastCompile: 0,
    failure: false,
    failureReason: "",
    lastLatexResult: null,
    provenance: null,
    docxArtifact: null,
  });

  return {
    state,
    begin({ clearFailure = false } = {}) {
      state.compiling = true;
      if (clearFailure) state.failure = false;
    },
    finish() { state.compiling = false; },
    succeeded() {
      state.failure = false;
      state.failureReason = "";
    },
    failed(reason) {
      state.failure = true;
      state.failureReason = String(reason || "could not render");
    },
    resetFailure() {
      state.failure = false;
      state.failureReason = "";
    },
    recordDuration(seconds) {
      if (seconds) state.lastCompile = seconds;
    },
    recordLatex(result) { state.lastLatexResult = result; },
    recordProvenance(value) { state.provenance = value; },
    recordDocx(value) { state.docxArtifact = value; },
    clearDocx() { state.docxArtifact = null; },
  };
}
