# SPEC: Quarto in LibrePaper

Quarto is Pandoc Markdown with executable cells. The cells are the only part
that needs a kernel; the rest is a dialect the Markdown engine can be taught.
This page specifies a `.qmd` as a LibrePaper source format that no machine ever
executes: the prose, the divs, the citations and the cross-references render
in the browser through the same engine crate as Markdown, and the results of
code cells arrive from the author's last local render, keyed to the cells
they belong to. Nothing here compiles on the server, which is the rule every
format already keeps.

The route the README describes -- render to self-contained HTML locally,
publish the HTML -- stays and is not changed by this. Its limit is the reason
for this page: the HTML is not the source, so what is typed in the browser
never reaches the `.qmd`, and `librepaper sync` has nothing to write back.

## What happens when a Quarto document opens

`librepaper publish paper.qmd` stores the file as `quarto`; `librepaper publish
paper/` takes the project directory with its figures, its `.bib` and any
frozen results. The reader renders the source in the browser with the
Markdown module and shows a flow HTML page, as it does for `.md`. Editing,
the word diff, sync to a file on disk, history and comments all apply as
they do to Markdown, because the format is Markdown to every layer except
the renderer. The document's title comes from the YAML front matter, or from
the first heading when there is none.

A code cell is shown as a code block and never run. Under each cell, the
reader shows the output the author's machine produced for that exact cell
text, when it has one; otherwise a placeholder that says the cell has not
been rendered since it changed. Editing a paragraph disturbs no output.
Editing a cell drops that cell's output until the next `quarto render` on
the author's machine, which `librepaper sync` or a second `publish` carries
up. This is the rule native Typst publishing already follows: an artifact
travels with the inputs it was produced from and is shown only while they
match.

## The format

`formatOf` in `web/src/lib/renderers.js` and `is_markdown` in the engine
crate map `.qmd` to a `quarto` format whose renderer is the Markdown
module. The Markdown module grows by the Quarto layer and stays the small
one; nothing in it fetches the Typst module. A deployment that offers
Markdown editing offers Quarto editing.

The engine renders a `.qmd` in five passes over one comrak parse:

1. **Source pre-pass.** Rewrites the few constructs comrak cannot parse into
   ones it can, skipping code spans and fenced blocks: bracketed spans
   `[text]{.class}` become raw inline spans; every closing div fence is
   brought to the length of the fence it closes; the `#|` option lines at
   the top of a cell are lifted out of the fence and attached to it.
2. **Parse.** Comrak with this project's existing options plus
   `front_matter_delimiter`, `math_dollars`, `math_latex`,
   `block_directive`, `header_attributes` and `fenced_code_attributes`.
3. **Structure pass.** Walks the tree once: reads the front matter; parses
   each directive's info string as a Pandoc attribute block
   `{#id .class key="value"}`; classifies each div by family (below);
   evaluates conditional content; finds the cells.
4. **Numbering pass.** Assigns numbers to figures, tables, equations,
   sections and theorem-like blocks by their id prefix, then resolves
   `@fig-x` and its siblings in the text, and `[@key]` citations against
   the bibliography.
5. **Render.** Comrak's HTML writer, with the directive, code block and
   front-matter nodes rendered by this crate rather than by comrak: a
   callout as an aside with a header, a figure div as a `<figure>` with its
   caption, a cell as code followed by its output.

The output is the standalone page `page::page` builds for every Markdown
document, with the Quarto styles added to the shell and a math renderer
included only when the document has math.

### Front matter

The YAML block at the top of the file. Read: `title`, `subtitle`, `author`
(a string or a list of names or of objects with `name`), `date`,
`abstract`, `bibliography` (a path or list of paths), `csl`,
`number-sections`, `toc`, and under `execute:` the `echo`, `eval`,
`include` and `freeze` defaults. Everything else is kept and ignored. The
title block renders above the body; `toc: true` renders a table of contents
from the headings. The `format:` key is not consulted: the output is HTML,
and conditional content is evaluated as such.

### Fenced divs

Comrak's `block_directive` extension parses the block: a line of three or
more colons opens a container, Markdown inside it parses normally, blocks
nest, and a matching fence closes it. It emits a div whose class is the raw
info string, which is why the structure pass owns the attribute string and
the render pass owns the element.

Two differences from Pandoc are handled in the pre-pass. Comrak closes a
fence only with a run at least as long as the opening one, while Pandoc
closes the innermost div with any run of three or more; the pre-pass
equalises the lengths. A bare `:::` with nothing after it at top level opens
a container in comrak; the pre-pass drops it, since it closes nothing.

The families, and what each renders as:

- **Callouts.** `.callout-note`, `.callout-tip`, `.callout-warning`,
  `.callout-caution`, `.callout-important`. The title is the `title`
  attribute, else a leading level-two heading inside the block, else the
  type's name. `collapse`, `appearance` and `icon` are read and applied as
  classes. Renders as `<aside class="callout note">` with a header and a
  body, so the markup matches what the README already writes by hand.
- **Cross-referenceable containers.** A div whose id starts with `fig-`,
  `tbl-`, `lst-`, `thm-`, `lem-`, `cor-`, `prp-`, `def-`, `exm-`, `exr-`
  or `rem-`. For `fig-` and `tbl-` the last paragraph is the caption, the
  rest is the content, and `layout-ncol`, `layout-nrow` or `layout` arrange
  the children as subfigures with letters. Theorem-like blocks take an
  optional heading as their name and render as "Theorem 1 (Name)" followed
  by the body. A bare image with a `#fig-` id and a bare table with a
  `#tbl-` caption line are the same thing without the fence and are
  numbered in the same sequence.
- **Layout.** `.columns` renders as a flex row and each `.column` takes its
  `width`. `.column-margin` renders as a margin note beside the paragraph
  it follows; `.column-body-outset`, `.column-page` and `.column-screen`
  widen the block past the text column. In a reader too narrow for a
  margin, the margin content follows the text as an aside.
- **Conditional content.** `.content-visible` and `.content-hidden` with
  `when-format`, `unless-format`, `when-profile`, `unless-profile`. The
  format is `html` and the profile is none. A visible block whose condition
  names html is kept; a hidden block whose condition names html is dropped;
  a block visible only for `pdf`, `latex`, `docx` or `revealjs` is dropped;
  a block hidden for those is kept. A dropped block leaves nothing in the
  page, so a comment cannot be anchored to it.
- **Tabsets.** `.panel-tabset` turns each level-two heading into a tab. The
  first version renders the sections stacked under their headings, which
  loses nothing a reader needs to comment on; tabs with a few lines of
  script come later.
- **Pass-through.** `.hidden` is dropped. `.aside` renders as a margin
  note. `.text-center`, `.small`, `.smaller`, `.border` are emitted as
  classes and styled. Reveal.js classes -- `.incremental`, `.fragment`,
  `.nonincremental` -- pass through unstyled; `.notes` is speaker notes and
  is dropped. Any class this list does not name is emitted as written.

### Bracketed spans

`[text]{#id .class key="value"}` is the inline twin of the fenced div. The
pre-pass turns it into a raw `<span>` with the attributes, so that emphasis
inside the brackets still parses. `.smallcaps`, `.underline` and `.aside`
are styled; `.aside` renders as a margin note like the div. A bracket
followed by a parenthesis is a link and is left to comrak.

### Cells

A cell is a fenced block whose info string is `{r}`, `{python}`, `{julia}`,
`{ojs}`, `{mermaid}`, `{dot}` or another language in braces, with an
optional label and with `#|` option lines at its top. The pre-pass lifts the
option lines out and attaches them to the block; the structure pass merges
them over the front matter's `execute:` defaults.

The code is shown unless `echo: false` or `include: false`. The output is
shown unless `output: false` or `include: false`. `eval: false` marks a cell
that has no output by design and gets no placeholder. `label: fig-x` or
`label: tbl-x` makes the cell's output a numbered figure or table with
`fig-cap` or `tbl-cap` as its caption, and `fig-subcap` with `layout-ncol`
makes subfigures. `code-fold: true` renders the code inside a `<details>`.
`{mermaid}` and `{dot}` cells are code and no diagram until a diagram
renderer is added; their frozen output, when there is one, is a figure like
any other.

Inline code, `` `{r} x` ``, renders as code and never as its value. It is
replaced by its frozen value when the freeze carries one for it.

### Cell outputs from the author's machine

Quarto writes a post-execution intermediate that needs no kernel to read.
With `execute: freeze: true` in the front matter or `_quarto.yml`, a render
writes `_freeze/<document>/execute-results/html.json`: the executed Markdown
with outputs in place, and a list of supporting files, the figures. With
`format: html: keep-md: true`, the same Markdown lands beside the document
as a `.md` file, with figures under `<document>_files/figure-html/`.

`librepaper publish paper.qmd` and `librepaper publish paper/` look for either,
in that order. From the executed Markdown, the CLI cuts each cell's output:
the blocks that follow a cell's fence up to the next cell or the next
paragraph of the original source. It keys the output by the SHA-256 of the
cell's source text after the option lines are removed, with whitespace at
the ends of lines trimmed. Outputs and their figures are uploaded as assets
of the document; the map from cell digest to output is one asset named
`.librepaper/quarto-outputs.json`. The source that is stored is the `.qmd`
itself, never the executed Markdown, so the author's file and the document
are the same text.

The renderer, given the tree, reads the map, digests each cell the same way
and splices the output whose digest matches under the cell, with its figure
paths rewritten through the asset resolver like any Markdown image. A cell
with no match shows the placeholder. Two cells with the same text share an
output, which is what they would have produced anyway.

When there is no freeze and no kept Markdown, `publish` says so in one line
and publishes the source; every cell shows the placeholder. When the freeze
is older than the source, `publish` still uploads it: the digests decide,
cell by cell, which outputs still apply.

`librepaper sync c9k paper.qmd` beside a Makefile that runs `quarto render`
re-reads the freeze on every save and uploads the outputs whose digests the
document does not yet have, so a render is a checkpoint with fresh outputs.

### Citations and cross-references

`[@key]`, `[@key, p. 12]`, `@key` and `-@key` resolve against the
bibliographies the front matter names, read from the project tree. The
engine crate already depends on hayagriva through Typst; the Markdown
module uses it to read BibTeX and render citations and a reference list in
the style the `csl` key names when the style is bundled, and in
author-year otherwise. A key no bibliography has renders as the key in
brackets and produces a diagnostic. The reference list renders where a
`::: {#refs}` div stands, else at the end of the document.

`@fig-x`, `@tbl-x`, `@sec-x`, `@eq-x` and the theorem prefixes resolve to
"Figure 1", "Table 2", "Section 3", "Equation 4" as links to the target.
`[-@fig-x]` gives the number alone, `[@fig-x]` in brackets is the same as
bare. A reference to an id the document does not define renders as written
and produces a diagnostic. Sections are numbered only under
`number-sections: true`, but `@sec-` references resolve regardless.

`$$ ... $$ {#eq-x}` numbers an equation. Math itself is emitted as comrak's
math spans and typeset in the page by a bundled math renderer, which the
shell includes only when the document has math.

### Shortcodes

`{{< include file.qmd >}}` splices the file from the tree in the pre-pass,
before parsing, so its headings and cells are numbered and keyed as if they
were in the main file. `{{< pagebreak >}}` is dropped. `{{< video >}}`
renders as a link. `{{< meta key >}}` reads the front matter. Other
shortcodes render as written, in a span the styles mute.

## Diagnostics

The Quarto layer reports through the same channel as Markdown: an unknown
citation key, an unresolved cross-reference, a cell whose output is missing
or stale, a front matter that does not parse, an include that names a file
the tree lacks. None of them stops the render; the page is produced with
the fault marked in place, as Markdown does for a missing figure.

## What is not done here

- No execution anywhere. Not in the browser, not on the server, not in the
  local app. A cell's output is what the author's render produced, or a
  placeholder.
- No Pandoc. A WebAssembly Pandoc exists and is tens of megabytes, and it
  would still not give callouts, cross-references or freeze handling, which
  live in Quarto's Lua filters. Covering the dialect papers use keeps the
  one-engine promise: the command line, the server and the browser render
  the same bytes from the same crate.
- No Quarto extensions and no filters. A document that depends on a Lua
  filter for its appearance renders without it.
- Project features -- `_quarto.yml` books, websites, dashboards, `.qmd`
  files that reference each other's cross-references -- are out of scope. A
  `_quarto.yml` is read only for its `execute:` defaults and its
  `bibliography`.
- `.ipynb` is a natural second source format under the same rule, with the
  outputs already embedded in the file. It is not specified here.

## Order of work

1. The format and the structure passes with callouts, layout, conditional
   content and cells as static code. This alone gives round-trip editing
   and comments on the real source for any `.qmd` whose cells are cosmetic
   or hidden.
2. Outputs from the freeze, in `publish`, `sync` and the renderer.
3. Numbering, cross-references and citations.
4. Tabsets with tabs, `code-fold`, margin notes, a diagram renderer.

## Open questions

- The renderer adds words that are not in the source: callout titles,
  "Figure 1", "Theorem 2 (Name)", the reference list. The Markdown path
  already adds footnote markers, so the comment anchoring presumably
  tolerates injected text; confirm before the numbering pass lands that a
  comment on a caption anchors to the caption's words and not to the label.
- Hayagriva's size in the Markdown module. If it moves the module far from
  its ~130 KB, citations may live behind a flag the way Typst does, and a
  build without it renders keys in brackets.
- Whether the executed Markdown's figures should be stored under the
  document as ordinary assets, addressable by path, or only through the
  output map. Ordinary assets mean an author can also reference a rendered
  figure from prose; the map alone is simpler to retire when a cell
  changes.
