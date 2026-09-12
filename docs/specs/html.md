# SPEC: HTML

Work nobody has asked for yet, and limitations kept on purpose. Each item
says which.

## Known ways work can be lost, kept deliberately

- A caret in a heavily tagged region of an HTML source -- syntax-highlighted
  code, where every token is its own `<span>` -- still finds the document
  about a fifth of the time less often than prose does. The window for HTML is
  already six times the width; the rest is the nature of the markup.
- A reader looking at a document whose format is `html` sees an edit when the
  frame reloads, about a second after the typing stops, rather than as each
  word lands. The frame is served the page itself so that the scripts a
  notebook or a Quarto page carries actually run; sending it over the
  `preview` channel instead would set `innerHTML`, which runs nothing. Markdown
  and typst readers see each render.

## As a source

- A megabyte-long `data:` URI in an HTML source is a megabyte-long line in
  CodeMirror, and slow. The answer is presentation only -- a replacing
  decoration that shows an inert chip saying what the URI is and how big,
  editable around and not inside -- and it is a day's work nobody has asked
  for yet. The renderer is `crates/librepaper/src/document/html.rs`.

## References

- [HTML renderer](../../crates/librepaper/src/document/html.rs) -- the renderer the `data:` URI decoration would sit beside.
