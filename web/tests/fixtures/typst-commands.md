# Typst Markup and Function Priorities for a WYSIWYG Editor

A prioritized implementation checklist for non-mathematical Typst markup, built-in functions, scripting constructs, and symbols. Intended for an academic-document WYSIWYG layer that must recognize input, render it, and preserve source. Based on Typst's official reference (October 2026). Not an empirically measured frequency ranking.

**Priority:** P0 = essential; P1 = very common / important; P2 = useful; P3 = advanced / specialist. Priorities indicate *recognition and preservation* first; full interactive editing may follow later.

**Scope:** Math expressions and math-library functions are excluded. Recognize `$...$` boundaries and delegate math rendering separately. Typst math is not LaTeX syntax, so KaTeX cannot process arbitrary Typst math source directly without conversion.

**Notation:** `#name(...)` denotes a Typst function call; `#name[...]` is a content-body form; `set` and `show` are language constructs, not ordinary functions. Where entries are field names or parameters rather than callable functions, they are explicitly marked.

## 1. Markup syntax and lexical constructs

| Priority | Syntax | Meaning |
|---|---|---|
| P0 | Blank line | Paragraph break |
| P0 | `*strong*` | Strong emphasis |
| P0 | `_emphasis_` | Emphasis |
| P0 | `` `code` `` | Inline raw text |
| P0 | Triple-backtick fenced block | Raw/code block, optional language |
| P0 | `= Heading`, `== Heading`, `=== Heading`, etc. | Heading levels |
| P0 | `- item` | Bullet-list item |
| P0 | `+ item` | Numbered-list item |
| P1 | `1. item`, `2. item` | Explicitly numbered items |
| P1 | `/ Term: definition` | Term-list item |
| P0 | `[content]` | Content block |
| P0 | `#name(...)` | Function call in markup |
| P0 | `#name[...]` | Function with content body |
| P0 | `#{ ... }` | Code block in markup |
| P0 | `#(expression)` | Embedded code expression |
| P0 | `@target`, `@bib-key` | Label or bibliography reference |
| P0 | `<target>` | Label |
| P0 | `https://example.com` | Recognized URL / link |
| P0 | `\` at line end | Explicit line break |
| P0 | `\*`, `\_`, `\#`, `\@`, `\<`, etc. | Escaped punctuation |
| P0 | `// comment` | Line comment in code context |
| P0 | `/* comment */` | Block comment |
| P1 | `'single'`, `"double"` | Smart quotation marks |
| P1 | `~` | Nonbreaking space |
| P1 | `--`, `---` | Dash shorthand |
| P1 | `...` | Ellipsis shorthand |
| P1 | `#sym.name` | Named symbol insertion |
| P1 | `#emoji.name` | Emoji namespace |
| P0 | `$...$` | Math boundary only; defer math content |
| P1 | `_` and `^` inside math | Math-mode tokens only; defer |
| P1 | `;` separating math lines | Math mode only; defer |
| P1 | `#let x = ...` | Variable binding / user macro |
| P1 | `#let f(x) = ...` | User-defined function |
| P1 | `#import "file.typ": name` | Module import |
| P1 | `#include "file.typ"` | Include document content |
| P1 | `#set ...` | Style rule |
| P1 | `#show ...` | Show/transformation rule |
| P1 | `#context ...` | Contextual content |
| P2 | `#if`, `else if`, `else` | Conditional content |
| P2 | `#for ... in ...` | Loop-generated content |
| P2 | `#while ...` | Loop-generated content |
| P2 | `#return`, `break`, `continue` | Control flow in code |

## 2. Document structure and model

| Priority | Function / construct | Meaning |
|---|---|---|
| P0 | `#heading(...)`, `#heading[...]` | Section heading |
| P0 | `#par[...]`, `#par(...)` | Paragraph |
| P0 | `#parbreak()` | Paragraph break |
| P0 | `#strong[...]` | Bold / strong emphasis |
| P0 | `#emph[...]` | Emphasis |
| P0 | `#list(...)`, `#list[...]` | Bulleted list |
| P0 | `#enum(...)`, `#enum[...]` | Numbered list |
| P1 | `#terms(...)` | Term/definition list |
| P1 | `#list.item[...]` | Explicit list item |
| P1 | `#enum.item(...)` | Numbered-list item with specified number |
| P1 | `#terms.item(...)` | Term-list entry |
| P0 | `#figure(...)` | Float-like figure/table with caption |
| P0 | `#figure.caption(...)` | Figure caption element |
| P0 | `#footnote[...]`, `#footnote(...)` | Footnote |
| P1 | `#footnote.entry(...)` | Footnote entry customization |
| P0 | `#link(...)`, `#link(...)[...]` | Hyperlink |
| P0 | `#ref(...)` | Cross-reference |
| P0 | `#cite(...)` | Bibliography citation |
| P0 | `#bibliography(...)` | Bibliography / references |
| P1 | `#outline(...)` | Table of contents / figures |
| P1 | `#outline.entry(...)` | Outline entry formatting |
| P1 | `#numbering(...)` | Number formatting |
| P1 | `#quote(...)`, `#quote[...]` | Block or inline quotation |
| P1 | `#title(...)` | Document title element |
| P1 | `#document(...)`, `#set document(...)` | Document metadata |
| P2 | `#divider(...)` | Thematic divider |
| P3 | `#asset(...)` | Asset inclusion in bundles |

## 3. Text appearance and inline typography

| Priority | Function | Meaning |
|---|---|---|
| P0 | `#text(...)`, `#text(...)[...]` | Font, size, color, language |
| P0 | `#raw(...)`, `` `...` `` | Verbatim / code text |
| P0 | `#strong[...]` | Strong emphasis |
| P0 | `#emph[...]` | Emphasis |
| P1 | `#underline[...]` | Underline |
| P1 | `#strike[...]` | Strikethrough |
| P1 | `#highlight[...]` | Highlighted text |
| P1 | `#smallcaps[...]` | Small capitals |
| P1 | `#super[...]` | Superscript in text |
| P1 | `#sub[...]` | Subscript in text |
| P1 | `#overline[...]` | Overline |
| P1 | `#smartquote(...)` | Smart quote |
| P1 | `#linebreak()` | Line break |
| P2 | `#upper(...)` | Uppercase transformation |
| P2 | `#lower(...)` | Lowercase transformation |
| P3 | `#lorem(...)` | Placeholder text |

### `text` arguments to recognize and preserve

| Priority | Named argument(s) | Function |
|---|---|---|
| P0 | `font`, `size`, `weight`, `style` | Basic font styling |
| P0 | `fill` | Text color/fill |
| P1 | `lang`, `region`, `dir` | Language, region, direction |
| P1 | `tracking`, `spacing`, `baseline` | Character spacing and baseline |
| P1 | `stroke`, `stretch` | Outline and width |
| P2 | `hyphenate`, `kerning`, `ligatures` | Text shaping |
| P2 | `fallback`, `features`, `variations` | OpenType and font fallback |
| P2 | `number-type`, `number-width`, `fractions` | Numeric typography |
| P3 | `alternates`, `stylistic-set` | Stylistic alternatives |
| P3 | `top-edge`, `bottom-edge`, `overhang` | Text metrics |
| P3 | `script`, `cjk-latin-spacing` | Script-specific behavior |

## 4. Links, references, and bibliographies

| Priority | Function / syntax | Meaning |
|---|---|---|
| P0 | `<label>` | Define target label |
| P0 | `@label` | Reference labeled element |
| P0 | `@citation-key` | Bibliography reference |
| P0 | `#label("name")` | Label value |
| P0 | `#ref(<label>)` | Programmatic cross-reference |
| P0 | `#cite(<key>)` | Programmatic citation |
| P0 | `#bibliography("refs.bib")` | Reference list from BibTeX |
| P1 | `#bibliography("refs.yml")` | Reference list from Hayagriva |
| P1 | `#link("https://...")[text]` | External link with custom text |
| P1 | `#link(<target>)[text]` | Internal link |
| P1 | `#ref(..., supplement: ...)` | Custom reference supplement |
| P1 | `#cite(..., form: ...)` | Citation form |
| P1 | `#cite(..., supplement: ...)` | Citation postnote / supplement |
| P1 | `#cite(..., style: ...)` | Citation style override |
| P1 | `#bibliography(..., style: ...)` | Bibliography style |
| P1 | `#bibliography(..., title: ...)` | Reference section title |
| P1 | `#bibliography(..., full: ...)` | Include uncited entries |
| P1 | `#outline(target: heading)` | Table of contents |
| P2 | `#outline(target: figure)` | List of figures |
| P2 | `#outline(depth: ..., indent: ...)` | Outline settings |
| P2 | `#numbering("1.1", ...)` | Formatted numbering |

## 5. Figures, images, and media

| Priority | Function / argument | Meaning |
|---|---|---|
| P0 | `#image("path.png")` | Insert image |
| P0 | `#image("path.svg")` | SVG image |
| P0 | `#image("path.pdf")` | PDF image |
| P0 | `#figure(image(...), caption: [...])` | Figure with caption |
| P0 | `#figure(table(...), caption: [...])` | Table with caption |
| P1 | `#figure(..., kind: ...)` | Figure type |
| P1 | `#figure(..., placement: ...)` | Figure positioning |
| P1 | `#figure(..., numbering: ...)` | Figure numbering |
| P1 | `#figure(..., supplement: ...)` | Figure name/label |
| P1 | `#figure(..., outlined: ...)` | Include in outline |
| P1 | `#image(..., width: ..., height: ...)` | Image sizing |
| P1 | `#image(..., fit: ...)` | Image fit/crop behavior |
| P1 | `#image(..., alt: ...)` | Accessibility alternative text |
| P2 | `#image(..., format: ...)` | Image format hint |
| P2 | `#image.decode(...)` | Decode image bytes |
| P2 | `#rotate(...)`, `#scale(...)` | Transformed graphics |

## 6. Tables and cells

| Priority | Function / argument | Meaning |
|---|---|---|
| P0 | `#table(...)` | Grid table |
| P0 | `#table(columns: ..., ...)` | Column specification |
| P0 | `#table.cell(...)` | Cell with metadata |
| P0 | `#table.cell(colspan: ...)` | Merged columns |
| P0 | `#table.cell(rowspan: ...)` | Merged rows |
| P0 | `#table.header(...)` | Repeating header |
| P1 | `#table.footer(...)` | Table footer |
| P1 | `#table.hline(...)` | Horizontal rule |
| P1 | `#table.vline(...)` | Vertical rule |
| P1 | `#table(columns: (auto, 1fr, ...))` | Flexible column widths |
| P1 | `#table(align: ...)` | Cell alignment |
| P1 | `#table(stroke: ...)` | Borders |
| P1 | `#table(fill: ...)` | Background fill |
| P1 | `#table(inset: ...)` | Cell padding |
| P1 | `#table(column-gutter: ..., row-gutter: ...)` | Cell spacing |
| P1 | `#table(rows: ...)` | Row heights |
| P2 | `#table(cell: ...)` | Cell customization |
| P2 | `#table.hline(y: ..., start: ..., end: ...)` | Partial horizontal rule |
| P2 | `#table.vline(x: ..., start: ..., end: ...)` | Partial vertical rule |
| P2 | `#table.cell(breakable: ...)` | Splitting cell content |
| P2 | `#table.cell(fill: ..., stroke: ..., inset: ...)` | Per-cell styling |
| P2 | `#table.header(repeat: ...)` | Repeat on subsequent pages |
| P2 | `#grid(...)` | General layout grid, not semantic table |
| P2 | `#grid.cell(...)` | Grid cell with spans |
| P2 | `#grid.hline(...)`, `#grid.vline(...)` | Grid rules |

## 7. Paragraph and page layout

| Priority | Function / construct | Meaning |
|---|---|---|
| P0 | `#set page(...)` | Page style and geometry |
| P0 | `#set text(...)` | Default typography |
| P0 | `#set par(...)` | Paragraph formatting |
| P1 | `#pagebreak()` | Page break |
| P1 | `#colbreak()` | Column break |
| P1 | `#columns(...)` | Multicolumn layout |
| P1 | `#align(...)` | Content alignment |
| P1 | `#block(...)` | Block-level container |
| P1 | `#box(...)` | Inline-level container |
| P1 | `#h(...)`, `#v(...)` | Horizontal / vertical spacing |
| P1 | `#pad(...)` | Insets / padding |
| P1 | `#stack(...)` | Stacked content |
| P1 | `#grid(...)` | Grid layout |
| P1 | `#place(...)` | Place / float content |
| P2 | `#move(...)` | Position offset |
| P2 | `#hide(...)` | Invisible content preserving space |
| P2 | `#repeat(...)` | Repeated leader content |
| P2 | `#rotate(...)` | Rotate element |
| P2 | `#scale(...)` | Scale element |
| P2 | `#skew(...)` | Skew element |
| P2 | `#layout(...)` | Layout-dependent evaluation |
| P2 | `#measure(...)` | Content measurement |

### Page and paragraph arguments

| Priority | Parameter(s) | Used with |
|---|---|---|
| P0 | `paper`, `width`, `height`, `margin` | `page` |
| P1 | `columns`, `header`, `footer` | `page` |
| P1 | `numbering`, `number-align` | `page` |
| P1 | `header-ascent`, `footer-descent` | `page` |
| P1 | `fill`, `flipped` | `page` |
| P2 | `binding`, `two-sided` | `page` |
| P2 | `background`, `foreground` | `page` |
| P1 | `justify`, `leading`, `first-line-indent` | `par` |
| P1 | `spacing`, `linebreaks` | `par` |
| P2 | `hanging-indent` | `par` |
| P2 | `widow-orphan` | `par` |
| P1 | `above`, `below`, `breakable` | `block` |
| P1 | `width`, `height`, `fill`, `stroke`, `inset`, `radius` | `block`, `box` |

## 8. Drawing, fills, shapes, and colors

| Priority | Function / value | Meaning |
|---|---|---|
| P1 | `#rect(...)` | Rectangle |
| P1 | `#line(...)` | Straight line |
| P1 | `#circle(...)` | Circle |
| P1 | `#square(...)` | Square |
| P1 | `#ellipse(...)` | Ellipse |
| P2 | `#polygon(...)` | Polygon |
| P2 | `#curve(...)` | Custom path |
| P1 | `#rgb(...)` | RGB color |
| P1 | `#luma(...)` | Grayscale |
| P2 | `#cmyk(...)` | CMYK color |
| P2 | `#hsl(...)`, `#hsv(...)` | Color models |
| P2 | `#color.mix(...)` | Color mixing |
| P2 | `#gradient.linear(...)` | Linear gradient |
| P2 | `#gradient.radial(...)` | Radial gradient |
| P2 | `#gradient.conic(...)` | Conic gradient |
| P2 | `#tiling(...)` | Repeated fill |
| P1 | `stroke: ...`, `fill: ...` | Common shape style arguments |
| P1 | `radius: ...` | Corners |
| P2 | `dash: ...`, `thickness: ...` | Stroke properties |
| P2 | `red`, `blue`, `black`, `white`, `gray`, `navy`, etc. | Named colors |

## 9. Styling rules and selectors

| Priority | Construct | Meaning |
|---|---|---|
| P0 | `#set text(...)` | Global/local text default |
| P0 | `#set heading(...)` | Heading numbering and options |
| P0 | `#set par(...)` | Paragraph default |
| P0 | `#set page(...)` | Page configuration |
| P1 | `#set list(...)`, `#set enum(...)` | List styles |
| P1 | `#set table(...)`, `#set figure(...)` | Table/figure styles |
| P1 | `#show heading: ...` | Heading styling |
| P1 | `#show "literal": ...` | Literal text replacement |
| P1 | `#show heading.where(level: 1): ...` | Filtered element selector |
| P1 | `#show: template.with(...)` | Apply document template |
| P1 | `#show selector: it => ...` | Transform matching content |
| P1 | `#set ... if condition` | Conditional set rule |
| P1 | `element.where(field: value)` | Selector constrained by fields |
| P2 | `selector(...)` | Selector construction |
| P2 | `.or(...)`, `.and(...)` on selectors | Selector combinations |
| P2 | `.before(...)`, `.after(...)` on selectors | Positional selectors |
| P2 | `#context ...` | Evaluate in layout context |

## 10. User functions, modules, and scripting

| Priority | Construct / function | Meaning |
|---|---|---|
| P0 | `#let name = value` | Binding |
| P0 | `#let name(args) = body` | User-defined macro/function |
| P0 | `#import "file.typ": name` | Import definitions |
| P0 | `#include "file.typ"` | Include content |
| P1 | `#import "file.typ": *` | Import all exported definitions |
| P1 | `#import "@preview/package:version": ...` | Package import |
| P1 | `#name(...)` | User-defined function invocation |
| P1 | `#name.with(...)` | Partial application |
| P1 | `#if ... { ... } else { ... }` | Conditional content |
| P1 | `#for item in items { ... }` | Loop |
| P1 | `#while condition { ... }` | Loop |
| P1 | `#context { ... }` | Contextual evaluation |
| P1 | `it => ...` | Anonymous function |
| P1 | `(..args)` / `args.pos()` / `args.named()` | Argument sinks and introspection |
| P1 | `dictionary.key`, `object.field` | Field access |
| P1 | `array.at(index)`, `dictionary.at(key)` | Indexed access |
| P1 | `none`, `auto`, `true`, `false` | Common value literals |
| P1 | `(a, b, c)` | Array literal |
| P1 | `(key: value, ...)` | Dictionary literal |
| P2 | `#eval(...)` | Evaluate Typst code string |
| P2 | `#type(...)`, `#repr(...)` | Type / representation |
| P2 | `#assert(...)`, `#panic(...)` | Validation/errors |
| P2 | `#sys.inputs` | CLI inputs |
| P2 | `#sys.version` | Typst version |
| P3 | `#plugin(...)` | WASM plugin integration |

## 11. Introspection, state, and document counters

| Priority | Function | Meaning |
|---|---|---|
| P1 | `#counter(heading)` | Heading counter |
| P1 | `#counter(page)` | Page counter |
| P1 | `#counter(...).display()` | Display current counter |
| P1 | `#counter(...).get()` | Read current counter (contextual) |
| P1 | `#counter(...).at(location)` | Counter at a location |
| P2 | `#counter(...).update(...)` | Update counter |
| P2 | `#counter(...).step()` | Step counter |
| P1 | `#here()` | Current location |
| P1 | `#locate(...)` | Evaluate content with location |
| P1 | `#query(selector)` | Find document elements |
| P1 | `#metadata(...)` | Attach/query metadata |
| P2 | `#state("key", initial)` | Stateful values |
| P2 | `#state(...).get()` | Read state |
| P2 | `#state(...).update(...)` | Modify state |
| P2 | `#location(...)` | Location value |

## 12. Data files and external resources

| Priority | Function | Meaning |
|---|---|---|
| P0 | `#read("file.txt")` | Read external text |
| P1 | `#read("file", encoding: ...)` | Read with encoding / bytes |
| P1 | `#csv("file.csv")` | Parse CSV |
| P1 | `#json("file.json")` | Parse JSON |
| P2 | `#yaml("file.yaml")` | Parse YAML |
| P2 | `#toml("file.toml")` | Parse TOML |
| P2 | `#xml("file.xml")` | Parse XML |
| P3 | `#cbor("file.cbor")` | Parse CBOR |
| P1 | `#image(...)` | Load image asset |
| P1 | `#bibliography(...)` | Load bibliography |
| P1 | `#import ...`, `#include ...` | Project dependency |

## 13. Symbols and special text

Typst's `sym` namespace is extensive; prioritize a generic resolver for `#sym.*` over hand-maintaining a full symbol table. These are examples of high-value families to recognize, not a complete enumeration of symbol names.

| Priority | Syntax / family | Meaning |
|---|---|---|
| P0 | `#sym.*` | General named symbols |
| P1 | `#sym.dash.*` | Dashes |
| P1 | `#sym.quote.*` | Quotation marks |
| P1 | `#sym.bullet` | Bullet |
| P1 | `#sym.ellipsis` | Ellipsis |
| P1 | `#sym.copyright` | Copyright |
| P1 | `#sym.registered` | Registered trademark |
| P1 | `#sym.trademark` | Trademark |
| P1 | `#sym.degree` | Degree sign |
| P1 | `#sym.section` | Section sign |
| P1 | `#sym.paragraph` | Paragraph sign |
| P2 | `#sym.currency.*` | Currency signs |
| P2 | `#sym.arrow.*` | Arrows outside math |
| P2 | `#sym.checkmark`, `#sym.ballot.*` | Check and ballot marks |
| P2 | `#sym.star.*` | Stars |
| P2 | `#sym.music.*` | Musical marks |
| P2 | `#emoji.*` | Emoji symbols |
| P0 | Literal Unicode characters | Preserve as source text |
| P1 | Nonbreaking space `~` | Special spacing |
| P1 | `--`, `---`, `...` | Markup symbol shorthands |

## 14. Low-level types and utility functions (recognition first)

These are common in templates and data-generated manuscripts; most do not need a WYSIWYG inspector.

| Priority | Function / family | Meaning |
|---|---|---|
| P1 | `str(...)`, `int(...)`, `float(...)` | Type conversions |
| P1 | `array(...)`, `dictionary(...)` | Collection conversion/construction |
| P1 | `calc.min(...)`, `calc.max(...)` | Basic calculations |
| P1 | `calc.round(...)`, `calc.abs(...)` | Numeric utilities |
| P2 | `calc.clamp(...)`, `calc.rem(...)` | Numeric helpers |
| P1 | `array.map`, `.filter`, `.join`, `.len` | Array transformations |
| P2 | `array.zip`, `.enumerate`, `.flatten`, `.sorted` | Collection transformations |
| P1 | `str.split`, `.replace`, `.contains` | String utilities |
| P2 | `str.trim`, `.starts-with`, `.ends-with`, `.len` | String utilities |
| P2 | `dictionary.keys`, `.values`, `.pairs`, `.insert`, `.remove` | Dictionary utilities |
| P2 | `regex(...)` | Regular expression |
| P2 | `datetime.today()` | Current date |
| P2 | `datetime(...)`, `datetime.display(...)` | Date values / formatting |
| P2 | `duration(...)` | Duration value |
| P2 | `bytes(...)` | Byte sequence |
| P2 | `path(...)` | Path value |
| P3 | `target(...)`, `version(...)` | Target/version data types |

## 15. Specialized output and extensibility (preserve source)

| Priority | Function / family | Meaning |
|---|---|---|
| P2 | `#pdf.*` | PDF-specific helpers and metadata |
| P2 | `#html.*` | HTML-specific output helpers |
| P3 | `#html.elem(...)` | Explicit HTML element |
| P3 | `#html.frame(...)` | HTML frame-related output |
| P3 | `#pdf.attach(...)` | PDF attachment |
| P3 | `#pdf.artifact(...)` | PDF artifact |
| P3 | `#pdf.header-cell(...)`, `#pdf.data-cell(...)` | PDF table accessibility helpers |
| P3 | `#plugin(...)` | WebAssembly integration |
| P3 | `#eval(...)` | Dynamic Typst program evaluation |
| P3 | Custom package-provided functions | Arbitrary third-party macros |

## 16. Recommended implementation sequence

- [ ] **P0 — Lexer and modes:** escaped characters, comments, content blocks, code interpolation, markup boundaries, raw/verbatim, labels, references, URL autolinking, math-region recognition.
- [ ] **P0 — Core editing:** paragraphs, headings, bold/emphasis, lists, inline code, links, citations, footnotes, figures/images, basic tables.
- [ ] **P0 — Lossless representation:** retain `#set`/`#show`, function arguments, user-defined functions, named arguments, imports, and unknown calls verbatim.
- [ ] **P1 — Academic documents:** bibliography and styles, captions, cross-references, numbered headings, explicit enum items, term lists, quotes, outlines, custom title and metadata.
- [ ] **P1 — Rich formatting:** color and text styles, line breaks, table spans, figure placement, page/paragraph layout, grids and columns.
- [ ] **P1 — Templates:** `let`, `import`, `include`, `show`, `set`, `.where`, `.with`, contextual constructs.
- [ ] **P2 — Advanced render:** shapes, gradients, specialized table styling, generated content, counters, state, data-driven content.
- [ ] **P3 — Opaque fallback:** plugin code, dynamic evaluation, arbitrary packages, output-specific constructs, advanced scripting.

## 17. Key WYSIWYG compatibility caveats

1. **Typst has three modes:** markup, code, math. Correct mode transitions are necessary even if math itself is delegated.
2. **A named function is not always an element:** e.g., `#text(...)` is a content-producing element; `#let` is a language construct; `#read` is a data function. A simplistic `#word(...)` regex parser is insufficient.
3. **Style is contextual:** `#set` and `#show` can affect later content, nested scopes, and custom transformations; editing a node should not silently flatten these rules.
4. **Many elements have both shorthand and function forms:** `= Heading` and `#heading[...]`; `- Item` and `#list[...]` should retain their original syntax on round-trip.
5. **User-defined functions are unrestricted document constructors:** treat unknown calls as first-class, source-preserving nodes instead of discarding them.
6. **Tables are not purely textual:** table/grid cells, spans, arguments and content ordering require structural parsing.
7. **Citations and references are distinct:** `@key` is resolved by context as a bibliography citation or a document label reference.
8. **Do not assume `#show` is presentation-only:** it can replace and restructure document content. Exact visual layout can require the Typst compiler.
9. **Raw blocks are lexical exceptions:** parse fences and language tags without interpreting their contents as Typst markup.
10. **Math caveat:** Typst math source and KaTeX/LaTeX math are different languages; rendering through KaTeX requires conversion or a different math-source choice.

## Official references

- Syntax: https://typst.app/docs/reference/syntax/
- Full reference: https://typst.app/docs/reference/
- Styling: https://typst.app/docs/reference/styling/
- Model: https://typst.app/docs/reference/model/
- Text: https://typst.app/docs/reference/text/
- Layout: https://typst.app/docs/reference/layout/
- Visualize: https://typst.app/docs/reference/visualize/
- Introspection: https://typst.app/docs/reference/introspection/
- Foundations: https://typst.app/docs/reference/foundations/
