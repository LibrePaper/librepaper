// A handout in the style of Edward Tufte, using the marginalia package.
// #note numbers a sidenote; #note(counter: none) gives an unnumbered margin note.
#import "@preview/marginalia:0.3.1" as marginalia: note, notefigure, wideblock

// Sidenotes get plain superscript numbers, as in Tufte. Unnumbered margin
// notes still use #note(counter: none).
#let note = note.with(numbering: (..i) => super(numbering("1", ..i)))

#let title = "{{title}}"
#let author = "{{author}}"

#set document(title: title, author: author)
#set page(paper: "us-letter")

// A narrow left margin and a wide right margin for sidenotes.
#show: marginalia.setup.with(
  inner: (far: 6mm, width: 30mm, sep: 6mm),
  outer: (far: 6mm, width: 55mm, sep: 8mm),
  top: 2.5cm,
  bottom: 2.5cm,
)

#set text(font: "Libertinus Serif", size: 11pt)
#set par(justify: true, leading: 0.7em, spacing: 1.1em)
#show math.equation: set text(font: "New Computer Modern Math")
// Table cells are ragged, not justified, so short words do not spread out.
#show table: set par(justify: false)
#set heading(numbering: none)
#show heading.where(level: 1): it => {
  v(1.2em)
  text(size: 15pt, weight: "regular", it.body)
  v(0.2em)
}

// Title block
#text(size: 26pt, title)
#linebreak()
#text(style: "italic", author)
#linebreak()
#datetime.today().display("[month repr:long] [day padding:none], [year]")

#v(1em)

#smallcaps[Good graphics] tell the truth about data. They show the most
information with the least ink, and they leave the reader room to
think @tufte1983.#note[A sidenote is numbered and set in the margin beside
the line where it is called.]

= Data ink

Every mark on a page should earn its place. The share of ink that carries
data is a simple measure of how much of a graphic does real work:

$ r = "data ink" / "total ink" $

A high ratio means the reader sees the data first. Redundant gridlines,
heavy frames and decorative fills push the ratio down.#note(counter: none)[
  This note has no number. Use one for an aside that does not need a marker.
]

= Tables

Tables work best when the numbers are easy to compare down a column. Rules
are kept to the top, below the header, and at the bottom, and the rest of
the table stays open.

#figure(
  table(
    columns: (1fr, auto, auto),
    align: (left, right, left),
    stroke: none,
    inset: (x: 6pt, y: 5pt),
    table.hline(stroke: 0.8pt),
    table.header([*Book*], [*Year*], [*Theme*]),
    table.hline(stroke: 0.5pt),
    [The Visual Display of Quantitative Information], [1983], [Graphical excellence],
    [Envisioning Information], [1990], [Visual reasoning],
    [Beautiful Evidence], [2006], [Evidence and presentation],
    table.hline(stroke: 0.8pt),
  ),
  caption: [Three books on information design, in order of publication.],
)

= Small multiples

Repeating one small graphic with a shared scale lets the reader compare
groups at a glance @tufte2006. A margin figure keeps such a graphic close to the text
that discusses it, without breaking the reading line.

#notefigure(
  rect(width: 100%, height: 3.2cm, stroke: 0.5pt + gray)[
    #align(center + horizon)[#text(size: 9pt)[Placeholder for a margin figure]]
  ],
  caption: [A margin figure keeps its caption beside the text it supports.],
)

#wideblock[
  Wide material, such as a long table or a sequence of panels, can run across
  the text column and into the margin. Use the full width when the content
  needs room to be read in one glance.
]

#bibliography("references.bib", title: "References", style: "chicago-author-date")
