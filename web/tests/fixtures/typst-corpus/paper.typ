#import "lib.typ": section-note

#set page(
  paper: "a4",
  margin: 2cm,
  header: [LibrePaper corpus · #context counter(page).display()],
  footer: [Typst PDF fixture],
  columns: 2,
)
#set text(size: 10pt)

= A paged Typst corpus

This fixture exercises imported definitions, Unicode (naïve café — 日本語 — 🙂),
tables, columns, headers and footers, bibliography, and an embedded image.
#section-note

#table(
  columns: 3,
  [*Input*], [*Estimate*], [*Error*],
  [one], [1.20], [0.03],
  [two], [2.40], [0.08],
)

#figure(
  image("asset.svg", width: 24pt),
  caption: [An embedded SVG asset.],
)

As shown by @doe2020, a small reproducible example can still span pages.
#bibliography("refs.bib")
