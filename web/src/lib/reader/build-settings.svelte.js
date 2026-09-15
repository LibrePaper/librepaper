// Which tool builds this document, and with what.
//
// Build selection is deliberately browser-local: the collaborative session
// supplies source files and never this preference, so two people reading the
// same project can compile it with different tools without arguing about it.
// The LaTeX settings beside it are the opposite -- they live in the project's
// Yjs `meta` map, shared, and are mirrored into state here because that map
// is not itself reactive.
//
// This owns both, the outputs and preview modes that follow from them, and
// the bookkeeping that goes with all of it: which reader's preferences are
// loaded, and which session's LaTeX configuration has been installed. What it
// takes are the two effects it cannot perform -- interrupting whatever is
// being rendered, and asking for a repaint.

import { read as readPreferences } from "../build-preferences.js";
import * as defaultLatex from "../latex.js";
import * as defaultRenderers from "../renderers.js";

export function createBuildSettings({
  slug,
  origin,
  read = readPreferences,
  latex = defaultLatex,
  renderers = defaultRenderers,
  // Everything in flight stops when the preference or the reader changes.
  // Takes the new preference, because which local preview may survive
  // depends on which tool was chosen.
  interrupt = () => {},
  paint = () => {},
}) {
  const state = $state({
    preferences: { selection: "automatic", backend: "auto", format: "" },
    // What the project says about its LaTeX engine. Redrawn when a change
    // arrives from another collaborator, not only when this browser writes.
    latex: { engine: "auto" },
    // What each format is currently producing, and which engine is drawing
    // it. These follow from the preference and are kept beside it: they used
    // to live in the page, which meant this module had to hand them back
    // through a callback to set values it had just worked out.
    latexOutput: "pdf",
    typstOutput: "pdf",
    quartoPreviewMode: "quarto",
    typstPreviewMode: "typst",
  });

  const outputOf = (preference) => (preference.output === "html" ? "html" : "pdf");

  /// What a preference says about the outputs and preview modes.
  ///
  /// The two paths differ, and deliberately. A preference *loaded* for a
  /// format says what that format's preview mode is whether or not this
  /// document is in it -- the Typst mode is read when a Typst file is opened
  /// later. A preference *chosen* by hand only speaks for the document on
  /// screen, because that is what the reader was looking at when they chose.
  function apply(preference, format, how) {
    if (format === "latex") state.latexOutput = outputOf(preference);
    if (format === "typst") state.typstOutput = outputOf(preference);
    if (how === "loaded") {
      state.typstPreviewMode = preference.backend === "local" && preference.tool === "calepin" ? "calepin" : "typst";
      if (preference.selection === "tool") {
        state.quartoPreviewMode = preference.backend === "local" && preference.tool === "quarto" ? "quarto" : "markdown";
      }
      return;
    }
    const local = preference.selection === "tool" && preference.backend === "local";
    if (format === "quarto") state.quartoPreviewMode = local && preference.tool === "quarto" ? "quarto" : "markdown";
    if (format === "typst") state.typstPreviewMode = local && preference.tool === "calepin" ? "calepin" : "typst";
  }

  // Which reader's preferences are loaded, and which session's LaTeX
  // configuration is installed. Neither is state: nothing draws them.
  let previousUser = "";
  let observedSession = null;

  const scope = (user) => ({ origin, user, document: slug });

  const engineSettings = (preference) => ({
    ...preference,
    engine: preference.engine || "auto",
    backend: preference.backend || "auto",
    tool: preference.tool || "tex",
    output: preference.output || "pdf",
    preset: preference.preset || "",
  });

  /// The format or the reader changed, so the preference for that pair is
  /// loaded. A reader signing in or out is a different person with different
  /// preferences, and whatever the last one had running is interrupted;
  /// loading an identical preference for the same reader is not, because
  /// source hydration and later source changes own the rendering.
  function follow({ format, user }) {
    if (!format) return;
    if (previousUser && previousUser !== user) interrupt(null);
    previousUser = user;
    state.preferences = read(scope(user), format);
    if (format === "latex") latex.configure({ project: slug, settings: state.preferences });
    apply(state.preferences, format, "loaded");
  }

  /// A preference chosen by hand, from the settings dialog or a menu.
  function choose(next, format) {
    state.preferences = next;
    interrupt(next);
    apply(next, format, "chosen");
    if (format === "latex") {
      state.latex = engineSettings(next);
      latex.configure({ project: slug, settings: state.latex, mayCompile: renderers.available("latex") });
      latex.setSettings(state.latex);
    }
    paint();
  }

  /// The scope a preference update is written against. Exposed because the
  /// menus build their updates from it before handing them back to `choose`.
  function scopeFor(user) {
    return scope(user);
  }

  /// Stop watching LaTeX for the session that was being watched.
  function stopLatex() {
    if (observedSession) latex.cancel();
    observedSession = null;
  }

  /// Install this project's LaTeX configuration, once per session. The
  /// session is passed rather than read: the caller has the one it means, and
  /// a configuration installed against a session that has since been replaced
  /// is the bug this guards against.
  function configureLatex(format, session, user) {
    if (format !== "latex" || !session) {
      stopLatex();
      return null;
    }
    if (observedSession === session) return null;
    stopLatex();
    state.preferences = read(scope(user), format);
    state.latexOutput = outputOf(state.preferences);
    state.latex = engineSettings(state.preferences);
    latex.configure({
      project: slug,
      settings: state.latex,
      mayCompile: renderers.available("latex"),
    });
    observedSession = session;
    return state.preferences;
  }

  return { state, follow, choose, scopeFor, stopLatex, configureLatex };
}
