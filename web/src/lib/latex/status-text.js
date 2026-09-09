// Pure text and mapping helpers for the compile status line.
//
// `LatexStatus.svelte`, `Diagnostics.svelte` and `Reader.svelte`'s
// `renderedNote` all need to turn a `Status`, a `Provenance` or an
// `Attempt[]` (docs/specs/latex-interfaces.md section 2.1) into a sentence
// a reader can act on. None of that needs a DOM, a worker or `latex.js`
// itself, so it lives here, in plain functions a Node check can call
// directly (see checks/latex-reader.mjs) -- the same reason `latex/status.js`
// keeps the store apart from `latex.js`.
//
// Every wording below either quotes docs/specs/latex-compiler.md's "Failure presentation"
// list verbatim (the `status.message` strings, which this file never
// invents) or fills in the one line SPEC asks for beside them: why a
// fallback happened, and what a given failure kind needs from the reader.

/// The chip beside the status message: which backend is producing pages, or
/// naming the VM-bibliography case the spec calls out on its own ("browser +
/// VM bibliography" is not just "browser" -- a reader who sees only "browser"
/// has no way to know Biber ran outside it).
export function backendChip(status) {
  if (!status) return "";
  if (status.phase === "vm-biber" || status.phase === "vm-preparing") return "browser + VM bibliography";
  if (status.backend === "local") return "local";
  if (status.backend === "browser") return "browser";
  return "";
}

/// Which contextual actions the status line offers right now, in the order
/// they should be drawn. Driven by the local connection state and, once a
/// job has actually failed, by its `failure.kind` -- never by `phase` alone,
/// since a phase like "failed" says nothing about what a reader can do about
/// it and a failure kind does.
export function actionsFor(status) {
  if (!status) return [];
  const local = status.local || {};
  const actions = [];
  if (status.phase === "local-needed" || local.state === "unreachable" || local.state === "denied") {
    actions.push("connect", "retry");
  }
  if (status.phase === "failed" && status.lastResult?.failure) {
    actions.push(...actionsForFailure(status.lastResult.failure));
  }
  if (local.state === "unauthorized" || local.state === "incompatible") actions.push("open-app");
  if (status.route === "native") actions.push("try-browser");
  return [...new Set(actions)];
}

function actionsForFailure(failure) {
  switch (failure?.kind) {
    case "local-unavailable":
      return ["connect", "retry"];
    case "tool-missing":
      return ["doctor"];
    case "incompatible":
      return ["connect"];
    case "vm":
      return ["retry", "connect"];
    case "native":
      return ["diagnostics"];
    default:
      return [];
  }
}

/// The one-line hint under a failed status: SPEC "Failure presentation"
/// asks that a connection error "not erase TeX diagnostics" and that the
/// interface "explain whether the local app was unreachable, a required
/// tool was missing, versions were incompatible, or the native build itself
/// failed" -- this is that explanation, keyed on `failure.kind`.
export function failureHint(failure) {
  if (!failure) return "";
  switch (failure.kind) {
    case "local-unavailable":
      return "Local LibrePaper is unavailable. Connect it or retry the connection.";
    case "tool-missing":
      return `${failure.message || "A required tool is missing"}. Run \`librepaper local doctor\` for setup help.`;
    case "incompatible":
      return `${failure.message || "The local and browser versions are incompatible"}.`;
    case "vm":
      return `${failure.message || "The browser bibliography VM could not finish this"}. Retry, or connect local LibrePaper.`;
    case "native":
      return "The local build failed too; see Diagnostics.";
    default:
      return failure.message || "";
  }
}

/// One matter-of-fact line explaining a successful compile that fell back to
/// another backend -- SPEC: "A successful local fallback should explain why
/// it was used without interrupting editing" and "without leaving an error
/// state". Empty unless there actually was more than one attempt with a
/// reason attached; a single successful browser attempt says nothing.
export function fallbackExplanation(attempts) {
  if (!Array.isArray(attempts) || attempts.length < 2) return "";
  const reasons = attempts.filter((attempt) => !attempt.ok && attempt.reason).map((attempt) => attempt.reason);
  if (!reasons.length) return "";
  const last = attempts[attempts.length - 1];
  return `Used ${last.backend} because ${reasons.join("; ")}.`;
}

const ENGINE_NAMES = { pdflatex: "pdfLaTeX", xelatex: "XeLaTeX", lualatex: "LuaLaTeX" };

function engineName(engine) {
  return ENGINE_NAMES[engine] || engine || "";
}

const BIBLIOGRAPHY_NAMES = {
  bibtex: "BibTeX",
  "local-biber": "local Biber",
  "vm-biber": "browser Biber (VM)",
  native: "native Biber",
};

function bibliographyName(kind) {
  return BIBLIOGRAPHY_NAMES[kind] || kind;
}

/// The "Compiled with" sentence Diagnostics shows: backend, engine, release
/// or tool versions, and the bibliography backend when one ran. e.g.
/// "Compiled locally with pdfLaTeX, pdfTeX 1.40.27 (TeX Live 2025);
/// bibliography via local Biber."
export function provenanceSentence(provenance) {
  if (!provenance) return "";
  const where = provenance.backend === "local" ? "Compiled locally" : "Compiled in the browser";
  const engine = engineName(provenance.engine);
  const tools = provenance.tools || {};
  const detail = provenance.backend === "local"
    ? tools.tex
      ? `, ${tools.tex}`
      : ""
    : provenance.release
      ? ` (release ${provenance.release})`
      : "";
  const bibliography = provenance.bibliography
    ? `; bibliography via ${bibliographyName(provenance.bibliography)}`
    : "";
  return `${where}${engine ? ` with ${engine}` : ""}${detail}${bibliography}.`;
}

/// Where a rendering came from, in the parenthetical `renderedNoteText` adds
/// to its date: "(locally, pdfLaTeX)" or "(in the browser, XeLaTeX)".
function provenanceDetail(provenance) {
  if (!provenance) return "";
  const where = provenance.backend === "local" ? "locally" : "in the browser";
  const engine = engineName(provenance.engine);
  return [where, engine].filter(Boolean).join(", ");
}

/// The note a reader sees under the toolbar for a stored rendering that is
/// not the current text: SPEC's own example is "rendered from an earlier
/// version, 2026-09-07 (locally, pdfLaTeX)". `rendering` is what
/// `/renderings/latest` (or a checkpoint) answered -- `{ sha, at, current,
/// missing?, provenance? }`. Callers still say "not yet rendered" themselves
/// when there is no rendering at all; this only covers the three states a
/// rendering object itself can be in.
export function renderedNoteText(rendering) {
  if (!rendering) return "";
  if (rendering.missing) return "this version was never rendered";
  if (rendering.current) return "";
  const date = (rendering.at || "").slice(0, 10);
  const detail = provenanceDetail(rendering.provenance);
  return `rendered from an earlier version, ${date}${detail ? ` (${detail})` : ""}`;
}
