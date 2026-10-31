#import "@preview/ilm:2.1.1": ilm

// Edit the title and author below. Chapters live in chapters/.
#show: ilm.with(
  title: "{{title}}",
  authors: "{{author}}",
  abstract: [A short description of the book: the question it asks, the method it uses and who it is for.],
  preface: [
    #heading(level: 1, numbering: none)[Preface]

    This book collects the chapters of a single project in one place. Each chapter lives in its own file under `chapters/`, so you can write and revise them separately. The preface explains the aim, the reader and the way the chapters fit together.
  ],
  raw-text: "use-typst-default",
  appendix: (
    enabled: true,
    title: "Appendix",
    heading-numbering-format: "A.1.1.",
    body: [#include "chapters/appendix.typ"],
  ),
  bibliography: bibliography("references.bib"),
)

#include "chapters/01-introduction.typ"

#include "chapters/02-methods.typ"

#include "chapters/03-results.typ"
