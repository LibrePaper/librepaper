// Every rule the deployment enforces lives here, and only here. The server
// imports it, the command line imports it, and the shell imports it in place
// of the `/api/config` request it used to make. A limit changed here changes
// everywhere, with no build step and no request between this module and any of
// its readers.
//
// This is the port of `config.rs`. The values are the same values; the shape
// is the JSON that `/api/config` used to answer with, so the shell reads the
// same field names it always read.

/// The defaults, as a fresh object each time: `serve` mutates its own copy
/// from the flags, and two deployments in one process (the tests run several)
/// must not share one.
export function defaultRules() {
  return {
    // The sum of every text in a document and of every key naming one. A paper
    // split into thirty files is allowed exactly what a paper in one file is
    // allowed, which is why this bounds the sum rather than each text. It
    // bounds a rendering too, now that a document may store one, so it is
    // named for the document rather than for the HTML it once was.
    max_document: 4 * 1024 * 1024,
    // How many files one document may hold, across its texts and its assets.
    // A paper has a dozen; a directory of two hundred is somebody using a
    // document as a filesystem.
    max_files: 200,
    // The longest one path may be, in bytes.
    max_path: 200,

    // Which extensions name a file a person edits, which name bytes nobody
    // edits in place, and which name what a compiler wrote -- and a document
    // keeps what a person wrote. Rules rather than constants, so a deployment
    // can widen or narrow them without a build.
    text_extensions: [
      '.tex', '.typ', '.md', '.markdown', '.bib', '.sty', '.cls', '.bst',
      '.txt', '.csv', '.json', '.yml', '.yaml', '.html', '.css',
    ],
    asset_extensions: [
      '.png', '.jpg', '.jpeg', '.gif', '.svg', '.webp', '.pdf', '.otf',
      '.ttf', '.woff', '.woff2',
    ],
    derived_extensions: [
      '.aux', '.bbl', '.blg', '.log', '.out', '.toc', '.fls', '.fdb_latexmk',
      '.synctex.gz', '.lof', '.lot', '.nav', '.snm',
    ],

    max_comments: 500,
    rate_per_hour: 20,

    // Storage is what keeps a deployment's bill bounded no matter who shows
    // up: a ceiling on everything stored, a ceiling per publisher, and a cap
    // on how many documents and how many uploads an hour one publisher gets.
    // Sizes are bytes of stored HTML; every index entry records its own.
    storage: {
      total: 5 * 1024 * 1024 * 1024,
      per_owner: 100 * 1024 * 1024,
      documents_per_owner: 50,
      uploads_per_hour: 30,
    },

    // Caps the serialized seed annotations a reserved example carries, in
    // bytes.
    max_annotations: 256 * 1024,

    // The only file types the reader can frame and anchor comments into. The
    // upload page checks them before sending, and the server checks them
    // again.
    extensions: ['.html', '.htm', '.md', '.markdown'],

    // The markups a document may be kept as, beside the HTML it was rendered
    // to, so it can be reopened and edited. HTML is a source format like the
    // other two, and its renderer is the identity: there is no longer a
    // document without a source.
    source_formats: ['markdown', 'typst', 'html'],

    // The maximum length of each free-text field on an annotation.
    caps: { body: 5000, creator: 80, exact: 1000, context: 64, tag: 24 },

    // The W3C Web Annotation motivations an annotation may carry. Using the
    // standard vocabulary rather than an invented one means an exported
    // annotation says the same thing to any tool that reads the spec.
    motivations: ['commenting', 'highlighting'],
    default_motivation: 'commenting',

    // How many labels one annotation may carry. Tags are what make a long
    // review navigable, but a dozen on one comment is a filing system, not a
    // label.
    max_tags: 6,

    // Caps a document title, in characters. Titles live in the index, which is
    // read on nearly every request, so an unbounded title is a way to sink the
    // whole deployment.
    max_title: 200,
    // Caps replies on one comment, so a thread cannot grow without bound and a
    // room stays small enough to load and rewrite whole.
    max_replies: 100,

    // The shape of a valid slug, as a RegExp source string.
    slug_pattern: '^[a-z0-9]+(?:-[a-z0-9]+)*$',
    slug_max: 80,

    // Documents are unlisted, so the URL is the only way in and the slug has
    // to be unguessable. 10 characters from a 32-symbol alphabet is 50 bits,
    // drawn from a CSPRNG; look-alike characters are left out so a link
    // survives being read aloud or retyped.
    suffix_alphabet: 'abcdefghijkmnpqrstuvwxyz23456789',
    suffix_length: 10,
  }
}

/// What the shell imports. One frozen copy of the defaults, for every reader
/// that is not a server building its own from flags.
export const RULES = Object.freeze(defaultRules())

/// Keeps an unknown motivation out of storage, falling back to the default
/// rather than rejecting the annotation.
export function allowedMotivation(rules, value) {
  return rules.motivations.includes(value) ? value : rules.default_motivation
}

/// Says whether a source in this format is worth keeping beside the document
/// it rendered to. Whether it can be rendered again *here* is a separate
/// question, answered by the renderers.
export function storableSource(rules, format) {
  return rules.source_formats.includes(format)
}

/// Overrides the document size ceiling, in megabytes. Zero leaves the default
/// alone. Returns an error message, or "" when it took.
export function setMaxDocument(rules, megabytes) {
  if (!megabytes) return ''
  if (megabytes < 1 || megabytes > 100) return '--max-size must be between 1 and 100 MB'
  rules.max_document = megabytes * 1024 * 1024
  return ''
}

/// Overrides the storage ceilings, in megabytes: how much one publisher may
/// hold across all their documents, and how much the whole deployment will
/// hold. Zero leaves a default alone.
export function setStorage(rules, quotaMb, totalMb) {
  if (quotaMb < 0 || totalMb < 0) return '--quota and --storage must be positive'
  if (quotaMb > 0) rules.storage.per_owner = quotaMb * 1024 * 1024
  if (totalMb > 0) rules.storage.total = totalMb * 1024 * 1024
  if (rules.storage.per_owner > rules.storage.total) {
    return `--quota (${rules.storage.per_owner >> 20} MB) cannot exceed --storage (${
      rules.storage.total >> 20
    } MB)`
  }
  return ''
}

/// Overrides the per-publisher counts: how many documents one publisher may
/// hold, and how many uploads they may make in an hour. Zero leaves a default
/// alone.
export function setCounts(rules, documents, uploadsPerHour) {
  if (documents < 0 || uploadsPerHour < 0) {
    return '--max-documents and --uploads-per-hour must be positive'
  }
  if (documents > 0) rules.storage.documents_per_owner = documents
  if (uploadsPerHour > 0) rules.storage.uploads_per_hour = uploadsPerHour
  return ''
}

/// The normalised form of a path, which is what is compared and what is
/// stored. NFC because two spellings of the same accented name are the same
/// file to every person who looks at them and to macOS, whatever the bytes say.
export function normalisePath(path) {
  return path.trim().normalize('NFC')
}

/// The key two paths collide on: normalised, then case-folded, because a sync
/// client on macOS or Windows would write `Fig.png` and `fig.png` to one file
/// and silently lose one of them.
export function collisionKey(path) {
  return normalisePath(path).toLowerCase()
}

/// At most eight segments. A paper has a `chapters/` and a `fig/`; a tree
/// deeper than this is a filesystem somebody is trying to store here.
export const MAX_SEGMENTS = 8

/// Whether a path is one the rules accept, and which kind of file it names:
/// `{ kind: 'text' | 'asset' }`, or `{ error }` with the sentence to show
/// whoever offered it.
///
/// This is the port of `paths.rs`, and the server checks the same thing again
/// on every route that takes a path. The shell checks first only so that a
/// refusal is explained where the person is, rather than by a status code.
export function checkPath(rules, path) {
  const it = normalisePath(path)
  const bytes = new TextEncoder().encode(it).length
  if (!it) return { error: 'a file needs a name' }
  if (bytes > rules.max_path) {
    return { error: `${it}: a path may be at most ${rules.max_path} bytes` }
  }
  if (it.startsWith('/')) {
    return { error: `${it}: a path is relative, so it cannot begin with /` }
  }
  if (it.includes('\\')) return { error: `${it}: paths are separated by /, not by \\` }
  // Anything a terminal would obey rather than print. Written as escapes, so
  // the rule survives being read and copied about.
  // eslint-disable-next-line no-control-regex
  if (/[\u0000-\u001f\u007f]/.test(it)) {
    return { error: `${it}: a path cannot carry control characters` }
  }
  const segments = it.split('/')
  if (segments.length > MAX_SEGMENTS) {
    return { error: `${it}: a path may be at most ${MAX_SEGMENTS} segments deep` }
  }
  for (const segment of segments) {
    if (!segment) return { error: `${it}: a path cannot have an empty segment` }
    if (segment === '.' || segment === '..') {
      return { error: `${it}: a path cannot climb with . or ..` }
    }
    if (segment.startsWith('.')) {
      return { error: `${it}: a dotfile is not part of a document` }
    }
  }
  return kindOf(rules, it)
}

/// The kind an extension names, or the reason there is none.
export function kindOf(rules, path) {
  const lower = path.toLowerCase()
  const ends = (list) => list.some((end) => lower.endsWith(end))
  // Derived first, because `.log` and `.out` are plausible-looking names and
  // the reason they are refused is worth saying.
  if (ends(rules.derived_extensions)) {
    return {
      error: `${path}: this is a file a compiler writes, and the document keeps what a person wrote`,
    }
  }
  if (ends(rules.text_extensions)) return { kind: 'text' }
  if (ends(rules.asset_extensions)) return { kind: 'asset' }
  return { error: `${path}: a document holds texts and figures, and this is neither` }
}

/// The next spelling of a path that is already taken: `paper.tex` becomes
/// `paper (2).tex`. Before the extension rather than after it, because a name
/// that stops being a `.tex` stops being a file the compiler will read.
export function suffixedPath(path, nth) {
  const dot = path.lastIndexOf('.')
  const split = dot > 0 && !path.slice(dot).includes('/')
  const stem = split ? path.slice(0, dot) : path
  const extension = split ? path.slice(dot) : ''
  return `${stem} (${nth})${extension}`
}

/// Whether a slug is one the rules will accept. The pattern is compiled here
/// rather than at every call site, so the shell and the server cannot disagree
/// about what a slug is.
export function validSlug(rules, slug) {
  return slug.length > 0 && slug.length <= rules.slug_max && new RegExp(rules.slug_pattern).test(slug)
}
