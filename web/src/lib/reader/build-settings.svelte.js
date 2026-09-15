// Which tool builds this document, and with what.
//
// Build selection is deliberately browser-local: the collaborative session
// supplies source files and never this preference, so two people reading the
// same project can compile it with different tools without arguing about it.
// The LaTeX settings beside it are the opposite -- they live in the project's
// Yjs `meta` map, shared, and are mirrored into state here because that map
// is not itself reactive.
//
// This owns both, and the bookkeeping that goes with them: which reader's
// preferences are loaded, and which session's LaTeX configuration has been
// installed. What it takes are the effects it cannot perform: interrupting
// whatever is being rendered, telling the page which output and preview modes
// the new preference implies, and asking for a repaint.

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
  // The output and preview modes the preference implies. Told how the
  // preference arrived -- "loaded" for the one this reader already had,
  // "chosen" for one picked by hand -- because what each says about a format
  // other than the one on screen is not the same.
  apply = () => {},
  paint = () => {},
}) {
  const state = $state({
    preferences: { selection: "automatic", backend: "auto", format: "" },
    // What the project says about its LaTeX engine. Redrawn when a change
    // arrives from another collaborator, not only when this browser writes.
    latex: { engine: "auto" },
  });

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
