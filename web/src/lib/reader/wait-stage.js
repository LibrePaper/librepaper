import { m } from "../paraglide/messages.js";

// The words on the preview's waiting display. One stage line stands for
// whatever the preview is doing before the first page is on the screen, and
// this is the whole of how that line is chosen.

const ENGINE_NAMES = { typst: "Typst", markdown: "Markdown", quarto: "Markdown" };

/**
 * @param {{ pageDelivered?: boolean, latexPhase?: string, latexScope?: string, engineLoading?: string, compiling?: boolean }} state
 * `latexPhase` and `latexScope` are the LaTeX browser compiler's status and
 * are empty for any other format.
 */
export function waitStageOf({ pageDelivered = false, latexPhase = "", latexScope = "", engineLoading = "", compiling = false } = {}) {
  // A page the viewer has been handed and has not drawn yet.
  if (pageDelivered) return m.wait_stage_drawing_pages();
  const latexBusy = latexPhase === "loading" || latexPhase === "compiling" || latexPhase === "browser-biber";
  if (latexBusy) {
    if (latexPhase === "browser-biber" || latexScope === "biber") return m.wait_stage_bibliography();
    if (latexScope === "resources") return m.wait_stage_packages();
    if (latexPhase === "loading") return m.wait_stage_latex_engine();
    return m.common_compiling();
  }
  // The renderer for Typst or Markdown is downloaded on first use, and the
  // first render waits for it.
  if (engineLoading) return m.wait_stage_loading_engine({ engine: ENGINE_NAMES[engineLoading] || engineLoading });
  if (compiling) return m.common_compiling();
  return m.wait_stage_preparing();
}
