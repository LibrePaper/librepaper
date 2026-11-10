#set document(title: "Edge cases", author: "A. Author")
#set page(paper: "a4", margin: (x: 2cm, y: 2.5cm))
#set text(font: "New Computer Modern", size: 11pt, lang: "en")
#set par(justify: true, leading: 0.65em)
#set heading(numbering: "1.1")
#show heading.where(level: 1): it => block(above: 2em)[#it]
#show link: set text(fill: blue)
#let note(body, title: "Note") = block(fill: luma(240), inset: 8pt)[*#title* #body]
#let big = 2em

// A line comment with *strong* and $math$
/* A block comment
   spanning #lines */

= Introduction <intro>
== Background
=== Deeper

Some *strong text*, _emphasis_, *_both_*, and `inline raw`.
Escapes: \# hash, \* star, \_ underscore, \@ at, \\ backslash, \$ dollar.
Shorthands: a~b, em---dash, en--dash, ellipsis..., ``curly'' quotes, 'single'.
Shorthand glyphs: -? soft hyphen and a\ hard break above.

```python
def f(x):
    return x  # not #highlighted
```

```typ
#let inside = "raw"
```

Inline math $x^2 + y_i$ and display:
$ sum_(k=1)^n k = (n(n+1))/2 $
$ integral_0^1 f(x) dif x \ "text in math" $

- Bullet one
- Bullet two
  - Nested bullet
+ Numbered one
+ Numbered two
  + Nested numbered
/ Term: Description of the term
/ Other: Other description

#note[Body with *markup* and #big.][title: "Warning"]
#note(title: "Tip")[Trailing content block.]
#text(fill: red, weight: "bold")[Colored text]
#rect(width: 3cm, height: 1cm, stroke: 0.5pt, radius: 2pt)[Box]

#figure(
  image("img/plot.svg", width: 80%),
  caption: [A figure with _markup_ and $alpha$ in the caption.],
) <fig-plot>

#figure(
  table(
    columns: (auto, 1fr),
    align: (left, right),
    [Name], [Value],
    [alpha], [1],
    [beta], [2],
  ),
  caption: [A table as figure.],
  kind: table,
) <tab-vals>

#table(
  columns: 3,
  table.header([*A*], [*B*], [*C*]),
  [1], [2], [3],
  table.cell(colspan: 2)[Merged],
  [4],
)

See @intro, @fig-plot, @tab-vals and @sec:detail for details. Cite @smith2020 and [@doe2019; @roe2021].
Labels: <lonely-label> and @lonely-label.

#let f(a, b: 2, ..rest) = a + b
#f(1, b: 3)
#set text(size: 9pt) if true
#show "TODO": highlight[TODO]
#show math.equation: set block(spacing: 0.8em)
#let counter-value = counter(heading).display()
#context counter(page).get()

Links: #link("https://example.org/a_b?c=d")[the site], https://example.com/bare, and #link("mailto:a@b.c")[mail].
Unclosed #emph[call and [bracket
Unclosed string: #text("broken
