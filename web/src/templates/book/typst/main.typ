#let title = "{{title}}"
#let author = "{{author}}"

#set document(title: title, author: author)
#set page(numbering: "1")
#set heading(numbering: "1.1")

#align(center, text(size: 28pt, weight: "bold", [#title]))

#align(center, text(size: 14pt, [by #author]))

#pagebreak()

#outline(
  title: [Contents],
  indent: auto,
)

#pagebreak()

#include "chapters/01-introduction.typ"

#pagebreak()

#include "chapters/02-main-content.typ"

#pagebreak()

#include "chapters/03-conclusion.typ"

#pagebreak()

#bibliography("references.bib", style: "american-psychological-association")
