# Org syntax tree

The Org syntax tree comes from `org.wasm`, built from
[LibrePaper/wasm-orgmode](https://github.com/LibrePaper/wasm-orgmode): its
`src/lezer.rs` maps the parser onto the node vocabulary below. `module.js`
loads the module and exposes `parse(text, fragments)`; `source-trees.js`
(`orgTree`, `orgTreeReady`, `onOrgSyntax`) hands the tree to the editor and the
Org features. Every consumer reads this vocabulary, so a change to a node name,
or to what a node holds, is a change to all of them.

Offsets are UTF-16 offsets into the text. "text" below means the node covers
only that piece, not its delimiters. Siblings never overlap.

One intended difference from the JavaScript parser this tree replaced: a
multi-line `$$…$$` or `\[…\]` is one `DisplayMath` node. The old parser read its
inside as text with `Entity` and `LatexCommand` children.

## Elements

Node names are the contract. The full list, in the order the tree defines them:

```
Document Section Headline Stars TodoKeyword Priority Title Tags Tag
Planning PlanningKeyword Clock ClockKeyword ClockDuration
PropertyDrawer Property PropertyKey PropertyValue Drawer DrawerName DrawerEnd
Keyword KeywordKey KeywordValue
Block BlockBegin BlockName BlockParameters BlockLanguage VerbatimBody CommentBody BlockBody BlockEnd
LatexEnvironment LatexEnvBegin LatexEnvName LatexEnvEnd
Comment FixedWidth HorizontalRule
Table TableRow TableCell TableDelimiter TableRule TableFormula
List ListItem ListBullet Checkbox ListTag FootnoteDefinition FootnoteLabel Paragraph
Bold Italic Underline StrikeThrough Verbatim Code EmphasisMark
Link LinkTarget LinkDescription FootnoteReference Citation CitationKey
TimestampActive TimestampInactive Macro MacroName Target RadioTarget
InlineMath DisplayMath Entity LatexCommand InlineSrc InlineSrcLanguage InlineSrcBody
InlineBabelCall InlineBabelName LineBreak
```

### Document and sections

- **`Document`**: the whole text. Its children are the elements before the
  first headline, then `Section`s.
- **`Section`**: a headline and everything up to the next headline of the same
  or a higher level. Children: `Headline`, element..., `Section`...
- **`Headline`**: the headline line. Children: `Stars`, `TodoKeyword?`,
  `Priority?`, `Title?`, `Tags?`.
  - **`Stars`**: the leading `*`s. The level is its length.
  - **`TodoKeyword`**: `TODO`, `DONE`, or a state declared by `#+TODO:`.
  - **`Priority`**: `[#A]`.
  - **`Title`**: the headline text. Holds inline nodes.
  - **`Tags`**: `:a:b:`, colons included. Holds `Tag`.
  - **`Tag`**: one tag name, without colons.

### Planning, clocks and drawers

- **`Planning`**: a `SCHEDULED:`, `DEADLINE:` or `CLOSED:` line. Children:
  `PlanningKeyword`, then timestamps.
  - **`PlanningKeyword`**: the keyword, such as `SCHEDULED:`.
- **`Clock`**: a `CLOCK:` line. Children: `ClockKeyword`, timestamps,
  `ClockDuration?`.
  - **`ClockKeyword`**: `CLOCK:`.
  - **`ClockDuration`**: `=> 1:30`.
- **`PropertyDrawer`**: `:PROPERTIES:` to `:END:`. Children: `DrawerName`,
  `Property`..., `DrawerEnd`.
  - **`Property`**: `PropertyKey`, `PropertyValue?`.
  - **`PropertyKey`**: the key between the colons, including a `+`.
  - **`PropertyValue`**: the raw value, trimmed.
- **`Drawer`**: any other `:NAME:` to `:END:`. Children: `DrawerName`,
  element..., `DrawerEnd`.
  - **`DrawerName`**: the name between the colons.
  - **`DrawerEnd`**: `:END:`.

### Keywords

- **`Keyword`**: `#+KEY: value`, including `#+NAME`, `#+CAPTION`, `#+ATTR_*`,
  `#+INCLUDE`, `#+BIBLIOGRAPHY`, `#+TITLE` and `#+TODO`. Children:
  `KeywordKey`, `KeywordValue?`. Affiliated keywords are siblings of the
  element they precede.
  - **`KeywordKey`**: the name, plus an optional `[option]`.
  - **`KeywordValue`**: the value, trimmed. Holds inline nodes for `TITLE`,
    `SUBTITLE`, `AUTHOR`, `DATE` and `CAPTION`. Raw text otherwise.

### Blocks

- **`Block`**: `#+begin_NAME` to `#+end_NAME`. Children: `BlockBegin`,
  body?, `BlockEnd?`. While typing, a block may lack its `BlockEnd`: it then
  runs to the end of the section (a headline still ends it) or of the document.
  A stray end line with no open block is a `BlockEnd` alone, outside any
  `Block`.
  - **`BlockBegin`**: the begin line. Children: `BlockName`,
    `BlockParameters?`.
    - **`BlockName`**: the name as written. Compare case-insensitively.
    - **`BlockParameters`**: the rest of the line, trimmed. For `src` and
      `export` it starts with `BlockLanguage`.
    - **`BlockLanguage`**: the first word of the parameters (`src`, `export`).
  - **`VerbatimBody`**: the lines of a `src`, `example` or `export` block. No
    children.
  - **`CommentBody`**: the lines of a comment block. No children.
  - **`BlockBody`**: the lines of any other block. Holds elements (quote, verse,
    center, dynamic blocks, custom names).
  - **`BlockEnd`**: the end line.
- **`LatexEnvironment`**: `\begin{x}` to `\end{x}` at the start of a line.
  Children: `LatexEnvBegin`, `LatexEnvEnd?`.
  - **`LatexEnvBegin`**: `\begin{x}`. Children: `LatexEnvName`.
  - **`LatexEnvName`**: `x`.
  - **`LatexEnvEnd`**: `\end{x}`. Absent for a single-line environment.

### Lines

- **`Comment`**: one `# text` line.
- **`FixedWidth`**: one `: text` line.
- **`HorizontalRule`**: five or more dashes.

### Tables

- **`Table`**: consecutive `|` lines. Children: `TableRow`, `TableRule` or
  `TableFormula`, in order.
  - **`TableRow`**: `TableDelimiter`, `TableCell`, `TableDelimiter`, and so on.
    - **`TableCell`**: the trimmed cell text. Holds inline nodes. Empty cells
      are zero-width.
    - **`TableDelimiter`**: one `|`.
  - **`TableRule`**: a `|---+---|` line.
  - **`TableFormula`**: a `#+TBLFM:` line after the rows. Children:
    `KeywordKey`, `KeywordValue?`.

### Lists

- **`List`**: `ListItem`s at one indentation.
  - **`ListItem`**: `ListBullet`, `Checkbox?`, `ListTag?`, element.... The text
    after the bullet is the first `Paragraph`.
  - **`ListBullet`**: `-`, `+`, `*` (indented), `1.` or `1)`.
  - **`Checkbox`**: `[ ]`, `[X]` or `[-]`.
  - **`ListTag`**: the term of `- term :: description`. Holds inline nodes.

### Footnotes and paragraphs

- **`FootnoteDefinition`**: `[fn:label] text` at column 0. Children:
  `FootnoteLabel`, element....
  - **`FootnoteLabel`**: the label. Also used in `FootnoteReference`.
- **`Paragraph`**: everything else, up to a blank line or the next element.
  Holds inline nodes.

## Inline nodes

These sit inside `Title`, `Paragraph`, `TableCell`, `ListTag`, `KeywordValue`,
`LinkDescription` and `Planning`.

- **`Bold`** `*x*`, **`Italic`** `/x/`, **`Underline`** `_x_`,
  **`StrikeThrough`** `+x+`: `EmphasisMark`, inline..., `EmphasisMark`.
- **`Verbatim`** `=x=`, **`Code`** `~x~`: two `EmphasisMark`s. The text between
  is not parsed.
- **`Link`**: `[[target][desc]]`, `[[target]]`, `<scheme:target>` or a bare
  `https://...`. Children: `LinkTarget`, `LinkDescription?`. The description
  holds inline nodes.
- **`FootnoteReference`**: `[fn:label]`, `[fn:label:definition]` or
  `[fn::definition]`. Children: `FootnoteLabel?`, inline....
- **`Citation`**: `[cite/style:...@key;...]`. Children: `CitationKey`....
  - **`CitationKey`**: the key, without its `@`.
- **`TimestampActive`**: `<2024-01-31 Wed 10:00>`.
- **`TimestampInactive`**: `[2024-01-31 Wed]`.
- **`Macro`**: `{{{name(args)}}}`. Child: `MacroName?`.
- **`Target`**: `<<x>>`. **`RadioTarget`**: `<<<x>>>`.
- **`InlineMath`**: `$x$` or `\(x\)`.
- **`DisplayMath`**: `$$x$$` or `\[x\]`. Can span several lines.
- **`Entity`**: `\alpha`, `\to`, from a built-in subset of Org's entity table.
- **`LatexCommand`**: any other `\name`.
- **`InlineSrc`**: `src_lang[header]{code}`. Children: `InlineSrcLanguage`,
  `InlineSrcBody`.
- **`InlineBabelCall`**: `call_name[h](args)[h]`. Child: `InlineBabelName`.
- **`LineBreak`**: `\\` at the end of a line.

## Not recognised

These read as plain text: sub- and superscripts, statistics cookies, `[@n]`
list counters, table.el tables, inline tasks, diary sexps, and plain links other
than a short list of schemes.
