import { m } from "./paraglide/messages.js";

// What a path in a document's directory may be.
//
// This is the shell's copy of `paths.rs`, and it is a copy on purpose: the
// server checks every path again on every route that takes one, and this
// exists only so that a name is refused where the person is typing it rather
// than by a status code a moment later. The rules themselves are the
// deployment's -- the extension lists and the length come from `/api/config`,
// so a deployment that widens them widens them here too without a build.
//
// `host/core/rules.js` holds the same check for the JavaScript host. Two
// copies rather than one import, because the shell and the host are separate
// programs that happen to agree, and the shell reaching into the host's
// sources is a dependency no bundler should be asked to follow.

/// At most eight segments. A paper has a `chapters/` and a `fig/`; a tree
/// deeper than this is a filesystem somebody is trying to store here.
export const MAX_SEGMENTS = 8;

/// The normalised form of a path, which is what is compared and what is
/// stored. NFC because two spellings of the same accented name are the same
/// file to every person who looks at them and to macOS, whatever the bytes
/// say.
export function normalisePath(path) {
  return (path || "").trim().normalize("NFC");
}

/// The key two paths collide on: normalised, then case-folded, because a sync
/// client on macOS or Windows would write `Fig.png` and `fig.png` to one file
/// and silently lose one of them.
export function collisionKey(path) {
  return normalisePath(path).toLowerCase();
}

/// Whether a path is one the rules accept, and which kind of file it names:
/// `{ kind: "text" | "asset" }`, or `{ error }` with the sentence to show
/// whoever offered it.
export function checkPath(rules, path) {
  const error = checkDirectoryPath(rules, path);
  if (error) return { error };
  return kindOf(rules, normalisePath(path));
}

/// Folder names obey the same path rules, without requiring a file extension.
export function checkDirectoryPath(rules, path) {
  const it = normalisePath(path);
  const bytes = new TextEncoder().encode(it).length;
  const max = rules?.max_path || 200;
  if (!it) return m.paths_needs_name();
  if (bytes > max) return m.paths_too_long({ path: it, max });
  if (it.startsWith("/")) {
    return m.paths_relative({ path: it });
  }
  if (it.includes("\\")) return m.paths_backslash({ path: it });
  // Anything a terminal would obey rather than print, written as escapes so
  // the rule survives being read and copied about.
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(it)) {
    return m.paths_control({ path: it });
  }
  const segments = it.split("/");
  if (segments.length > MAX_SEGMENTS) {
    return m.paths_too_deep({ path: it, segments: MAX_SEGMENTS });
  }
  for (const segment of segments) {
    if (!segment) return m.paths_empty_segment({ path: it });
    if (segment === "." || segment === "..") {
      return m.paths_climb({ path: it });
    }
    if (segment.startsWith(".")) {
      return m.paths_dotfile({ path: it });
    }
  }
  return "";
}

/// The kind an extension names, or the reason there is none.
export function kindOf(rules, path) {
  const lower = path.toLowerCase();
  const ends = (list) => (list || []).some((end) => lower.endsWith(end));
  const allowed = [...new Set([
    ...(rules?.text_extensions || []),
    ...(rules?.asset_extensions || []),
  ])];
  const error = m.paths_extensions({ path, allowed: allowed.join(", ") || m.paths_none() });
  // Derived extensions are refused even when their names look plausible.
  if (ends(rules?.derived_extensions)) {
    return { error };
  }
  if (ends(rules?.text_extensions)) return { kind: "text" };
  if (ends(rules?.asset_extensions)) return { kind: "asset" };
  return { error };
}

/// The next spelling of a path that is already taken: `paper.tex` becomes
/// `paper (2).tex`, then `paper (3).tex`. Before the extension rather than
/// after it, because a name that stops being a `.tex` stops being a file the
/// compiler will read. The mirror of `paths::suffixed` in
/// `librepaper-document-core`.
export function suffixed(path, nth) {
  const dot = path.lastIndexOf(".");
  // A leading dot is not an extension, and a dot in a directory name is not
  // this file's.
  const usable = dot > 0 && !path.slice(dot).includes("/");
  const stem = usable ? path.slice(0, dot) : path;
  const extension = usable ? path.slice(dot) : "";
  return `${stem} (${nth})${extension}`;
}
