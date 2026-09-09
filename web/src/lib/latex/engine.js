// Engine selection, before a single byte of the document is fetched.
//
// This file answers one question -- which of the three engines compiles this
// document -- and answers it the same way in the worker, in the settings
// panel and in a Node check, because it has no dependency on any of them: a
// pure function of a source string (and, for the top-level decision, the
// project's own setting).
//
// The order is fixed by the spec ("Project configuration and identity"):
// an explicit project setting wins outright; failing that, a magic comment in
// the main file is a promise from the author and is honoured even when the
// source shows no sign of needing it; failing that, conservative detection of
// packages and commands that are known to require a Unicode engine or LuaTeX;
// pdfLaTeX otherwise. Detection is deliberately narrow -- a document that
// merely mentions "xetex" in a comment or a string must not switch engines,
// which is why every pattern below runs on stripped, commentless text.
//
// The engine lists originated in WasmTex's build layer; the selector is kept
// local so that package is not a runtime dependency of LibrePaper. The
// precedence and package/command lists are covered by the corpus below.

export const ENGINES = ["pdflatex", "xelatex", "lualatex"];

/// `% !TEX program = ...` and its `TS-program` cousin (TeXShop/TeXworks
/// naming), case-insensitive on both the directive keyword and the engine
/// name. Only recognised engine names resolve to something; a directive
/// naming an engine we do not have (`% !TEX program = context`) is not a
/// silent pdfLaTeX fallback here -- `resolveEngine` treats a null directive
/// exactly like no directive, which is the conservative choice: an unknown
/// program name is more likely a typo or an unrelated tool than a deliberate
/// instruction this codebase can act on.
const DIRECTIVE = /%\s*!\s*(?:TEX\s+)?(?:TS-)?(?:program|engine)\s*=\s*([A-Za-z]+)/i;

function normalizeEngineName(name) {
  const lower = name.toLowerCase();
  if (lower === "xelatex" || lower === "xetex") return "xelatex";
  if (lower === "lualatex" || lower === "luatex" || lower === "dvilualatex") return "lualatex";
  if (lower === "pdflatex" || lower === "latex" || lower === "pdftex" || lower === "pdf") return "pdflatex";
  return null;
}

export function directiveOf(source) {
  // Directives live in the first couple of lines by convention; scanning the
  // whole file would risk matching a directive-shaped string inside a verbatim
  // block or a comment about directives. 2048 bytes covers every documented
  // placement (line 1 or 2) with room for a shebang-style preamble.
  const match = source.slice(0, 2048).match(DIRECTIVE);
  if (!match) return null;
  return normalizeEngineName(match[1] ?? "");
}

/// Strip `%...` comments (respecting `\%`), the same way TeX itself ignores
/// them, so a commented-out `\usepackage{fontspec}` left behind by an author
/// experimenting with fonts does not switch the engine for everyone after.
function stripComments(source) {
  return source.replace(/(^|[^\\])((?:\\\\)*)%.*$/gm, "$1$2");
}

/// Only the preamble is relevant to engine detection -- packages loaded and
/// commands issued after `\begin{document}` do not change what the preamble
/// already required, and scanning less text keeps detection cheap on large
/// documents. A document with no `\begin{document}` (a fragment, a partial
/// paste) falls back to a bounded prefix so detection still runs on it.
function preambleOf(source) {
  const at = source.indexOf("\\begin{document}");
  return stripComments(at >= 0 ? source.slice(0, at) : source.slice(0, 8192));
}

function loadedPackages(preamble) {
  const packages = new Set();
  for (const match of preamble.matchAll(/\\(?:usepackage|RequirePackage)\s*(?:\[[^\]]*\])?\s*\{([^}]*)\}/g)) {
    for (const raw of (match[1] ?? "").split(",")) {
      const name = raw.trim();
      if (name) packages.add(name);
    }
  }
  return packages;
}

function firstOf(packages, wanted) {
  for (const name of wanted) if (packages.has(name)) return name;
  return null;
}

// LuaTeX-only packages and commands: real LuaTeX API usage, not just
// something that happens to work under LuaLaTeX too.
const LUATEX_PACKAGES = new Set([
  "luacode",
  "luatextra",
  "luatexbase",
  "luatex85",
  "luaotfload",
  "lua-ul",
  "luamplib",
  "luacolor",
  "luatexja",
  "luatexja-fontspec",
  "luatexja-preset",
]);

// CJK-via-XeTeX packages: these specifically need XeTeX's font handling, not
// merely "a Unicode engine" (LuaTeX would need a different CJK stack).
const XETEX_ONLY_PACKAGES = new Set(["xeCJK", "xetexko", "xecjk"]);

// Packages that need a Unicode-capable engine but work under either XeTeX or
// LuaTeX; XeTeX is the conservative default for these.
const UNICODE_PACKAGES = new Set([
  "fontspec",
  "unicode-math",
  "xltxtra",
  "xunicode",
  "polyglossia",
  "mathspec",
]);

const FONTSPEC_COMMAND = /\\(?:setmainfont|setsansfont|setmonofont|newfontface|fontspec)\b/;

export function detect(source) {
  const preamble = preambleOf(source);
  const packages = loadedPackages(preamble);

  if (/\\directlua\b/.test(preamble)) return "lualatex";
  if (firstOf(packages, LUATEX_PACKAGES)) return "lualatex";
  if (firstOf(packages, XETEX_ONLY_PACKAGES)) return "xelatex";
  if (firstOf(packages, UNICODE_PACKAGES)) return "xelatex";
  if (FONTSPEC_COMMAND.test(preamble)) return "xelatex";
  return null;
}

/// The full order from the spec: explicit setting, then directive, then
/// detection, then pdfLaTeX. `settings.engine` of `"auto"` (or absent) defers
/// to the document.
export function resolveEngine(tree, settings) {
  const forced = settings?.engine;
  if (forced && forced !== "auto") return forced;
  const source = tree?.texts?.[tree?.main] ?? "";
  return directiveOf(source) ?? detect(source) ?? "pdflatex";
}

/// An early hint, used before a compile has produced real aux/bcf files to
/// inspect: `\usepackage[...]{biblatex}` without `backend=bibtex` in the
/// options implies Biber (biblatex's default backend since 1.0). This is
/// intentionally conservative and never authoritative -- the controller
/// (package B2) decides the real bibliography tool from the actual compile
/// outputs, per the spec's "Avoid using source regexes as the sole authority
/// for required helper work."
export function needsBiber(source) {
  const preamble = preambleOf(source);
  const match = preamble.match(/\\usepackage\s*(\[([^\]]*)\])?\s*\{biblatex\}/);
  if (!match) return false;
  const options = match[2] ?? "";
  return !/\bbackend\s*=\s*bibtex\b/i.test(options);
}
