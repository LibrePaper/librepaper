// A course handout: learning goals, colored boxes, a worked example,
// a theorem with proof, exercises and a summary.
// Edit the course name and the metadata below.
#import "@preview/showybox:2.0.4": showybox

#let title = "{{title}}"
#let author = "{{author}}"
#let course = "Course Name, Term Year"
#let issued = datetime.today().display("[month repr:long] [day], [year]")

#let navy = rgb("#1f4e79")
#let forest = rgb("#2e6b3a")
#let plum = rgb("#6a3d8f")
#let amber = rgb("#8a5a00")

#set document(title: title, author: author)
#set page(
  paper: "a4",
  margin: 2.2cm,
  header: context [
    #set text(size: 9pt, fill: luma(90))
    #course #h(1fr) Page #counter(page).display()
  ],
)
#set text(font: "Libertinus Serif", size: 11pt, lang: "en")
#set par(justify: true, leading: 0.65em)
#set heading(numbering: "1.1")
#show heading: set block(above: 1.4em, below: 0.8em)

// Colored boxes. Each helper wraps showybox with one color scheme.
#let definition(name, body) = showybox(
  title: "Definition: " + name,
  frame: (border-color: navy, title-color: navy, body-color: navy.lighten(92%), radius: 3pt),
  title-style: (color: white, weight: "bold"),
  body,
)

#let example(name, body) = showybox(
  title: "Example: " + name,
  frame: (border-color: forest, title-color: forest, body-color: forest.lighten(92%), radius: 3pt),
  title-style: (color: white, weight: "bold"),
  body,
)

#let theorem(name, body) = showybox(
  title: "Theorem: " + name,
  frame: (border-color: plum, title-color: plum, body-color: plum.lighten(92%), radius: 3pt),
  title-style: (color: white, weight: "bold"),
  body,
)

#let summary(body) = showybox(
  title: "Summary",
  frame: (border-color: amber, title-color: amber, body-color: amber.lighten(92%), radius: 3pt),
  title-style: (color: white, weight: "bold"),
  body,
)

#align(center)[
  #text(size: 10pt, fill: luma(90))[#course]
  #v(0.6em)
  #text(size: 20pt, weight: "bold")[#title]
  #v(0.3em)
  #text(size: 11pt)[#author | #issued]
]

#v(0.8em)

= Learning goals

By the end of this handout you should be able to:

- state the definition of a convergent sequence;
- prove that a given sequence converges, using the $epsilon$-$N$ definition;
- explain why every convergent sequence is bounded.

= Key definitions

#definition("Convergent sequence")[
  A sequence $(a_n)$ converges to $L$ when, for every $epsilon > 0$,
  there is an $N$ such that
  $ |a_n - L| < epsilon quad "for all" n >= N. $
]

#definition("Bounded sequence")[
  A sequence $(a_n)$ is bounded when there is a constant $M$ with
  $|a_n| <= M$ for every $n$.
]

= Worked example

#example("A rational sequence")[
  Show that $a_n = (2n + 1)/(n + 3)$ converges to $2$.

  Compute the distance to the limit:
  $ |a_n - 2| = |(2n + 1 - 2(n + 3))/(n + 3)| = 5/(n + 3). $

  Given $epsilon > 0$, the inequality $5/(n + 3) < epsilon$ holds as soon as
  $n > 5/epsilon - 3$. Choose $N = ceil(5/epsilon)$. Then $n >= N$ implies
  $|a_n - 2| < epsilon$, so $a_n -> 2$.
]

= A theorem with proof

#theorem("Convergent sequences are bounded")[
  If $(a_n)$ converges, then $(a_n)$ is bounded.
]

_Proof._ Let $L$ be the limit. Taking $epsilon = 1$ in the definition gives an
index $N$ with $|a_n - L| < 1$ for all $n >= N$. Hence $|a_n| < |L| + 1$
for those $n$. The finitely many terms $a_1, dots, a_(N-1)$ are bounded by
their largest absolute value. Taking $M$ to be the larger of that value and
$|L| + 1$ bounds the whole sequence. #h(1fr) _QED_

= Exercises

+ Show that $1/n$ converges to $0$.
+ Show that if $a_n -> L$ and $b_n -> K$, then $a_n + b_n -> L + K$.
+ Find the limit of $(3n^2 + 1)/(n^2 + 5)$ and justify your answer.

#summary[
  - A sequence converges when its terms eventually stay within any distance
    $epsilon$ of the limit.
  - Convergence implies boundedness, but a bounded sequence need not converge.
  - Every example starts from the definition, so practice with
    $epsilon$-$N$ arguments before moving on.
]
