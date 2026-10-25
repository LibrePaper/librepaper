// The words on the preview's waiting display. One stage line stands for
// whatever the preview is doing before the first page is on the screen, and
// this is the whole of how that line is chosen.

/**
 * @param {{ pageDelivered?: boolean, latexPhase?: string, latexScope?: string, quartoStarting?: boolean, compiling?: boolean }} state
 * `latexPhase` and `latexScope` are the LaTeX browser compiler's status and
 * are empty for any other format.
 */
export function waitStageOf({ pageDelivered = false, latexPhase = "", latexScope = "", quartoStarting = false, compiling = false } = {}) {
  // A page the viewer has been handed and has not drawn yet.
  if (pageDelivered) return "Drawing the pages";
  const latexBusy = latexPhase === "loading" || latexPhase === "compiling" || latexPhase === "browser-biber";
  if (latexBusy) {
    if (latexPhase === "browser-biber" || latexScope === "biber") return "Updating the bibliography";
    if (latexScope === "resources") return "Fetching LaTeX packages";
    if (latexPhase === "loading") return "Loading the LaTeX engine";
    return "Compiling";
  }
  if (quartoStarting) return "Starting Quarto";
  if (compiling) return "Compiling";
  return "Preparing the preview";
}
