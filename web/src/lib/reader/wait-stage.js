// The words on the preview's waiting display. One stage line stands for
// whatever the preview is doing before the first page is on the screen, and
// this is the whole of how that line is chosen.

const ENGINE_NAMES = { typst: "Typst", markdown: "Markdown", quarto: "Markdown" };

/**
 * @param {{ pageDelivered?: boolean, latexPhase?: string, latexScope?: string, liveStarting?: string, engineLoading?: string, compiling?: boolean }} state
 * `latexPhase` and `latexScope` are the LaTeX browser compiler's status and
 * are empty for any other format.
 */
export function waitStageOf({ pageDelivered = false, latexPhase = "", latexScope = "", liveStarting = "", engineLoading = "", compiling = false } = {}) {
  // A page the viewer has been handed and has not drawn yet.
  if (pageDelivered) return "Drawing the pages";
  const latexBusy = latexPhase === "loading" || latexPhase === "compiling" || latexPhase === "browser-biber";
  if (latexBusy) {
    if (latexPhase === "browser-biber" || latexScope === "biber") return "Updating the bibliography";
    if (latexScope === "resources") return "Fetching LaTeX packages";
    if (latexPhase === "loading") return "Loading the LaTeX engine";
    return "Compiling";
  }
  // A live preview run by the desktop app is starting for this file.
  if (liveStarting === "quarto") return "Starting Quarto";
  if (liveStarting === "calepin") return "Starting Calepin";
  // The renderer for Typst or Markdown is downloaded on first use, and the
  // first render waits for it.
  if (engineLoading) return `Loading the ${ENGINE_NAMES[engineLoading] || engineLoading} engine`;
  if (compiling) return "Compiling";
  return "Preparing the preview";
}
