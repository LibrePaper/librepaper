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
  if (!it) return "a file or folder needs a name";
  if (bytes > max) return `${it}: a path may be at most ${max} bytes`;
  if (it.startsWith("/")) {
    return `${it}: a path is relative, so it cannot begin with /`;
  }
  if (it.includes("\\")) return `${it}: paths are separated by /, not by \\`;
  // Anything a terminal would obey rather than print, written as escapes so
  // the rule survives being read and copied about.
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(it)) {
    return `${it}: a path cannot carry control characters`;
  }
  const segments = it.split("/");
  if (segments.length > MAX_SEGMENTS) {
    return `${it}: a path may be at most ${MAX_SEGMENTS} segments deep`;
  }
  for (const segment of segments) {
    if (!segment) return `${it}: a path cannot have an empty segment`;
    if (segment === "." || segment === "..") {
      return `${it}: a path cannot climb with . or ..`;
    }
    if (segment.startsWith(".")) {
      return `${it}: a dotfile is not part of a document`;
    }
  }
  return "";
}

/// The kind an extension names, or the reason there is none.
export function kindOf(rules, path) {
  const lower = path.toLowerCase();
  const ends = (list) => (list || []).some((end) => lower.endsWith(end));
  // Derived first, because `.log` and `.out` are plausible-looking names and
  // the reason they are refused is worth saying rather than "not a file type
  // this holds".
  if (ends(rules?.derived_extensions)) {
    return {
      error: `${path}: this is a file a compiler writes, and the document keeps what a person wrote`,
    };
  }
  if (ends(rules?.text_extensions)) return { kind: "text" };
  if (ends(rules?.asset_extensions)) return { kind: "asset" };
  return { error: `${path}: a document holds texts and figures, and this is neither` };
}
